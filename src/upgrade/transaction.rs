use super::release::digest;
use outrim::install::Agent;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub(super) struct Target {
    pub path: PathBuf,
    pub agent: Option<Agent>,
    original: String,
    staged: Option<PathBuf>,
    backup: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Default)]
pub(super) struct Plan {
    pub targets: Vec<Target>,
}

impl Target {
    pub fn inspect(path: PathBuf, agent: Option<Agent>) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.permissions().readonly() {
            return Err(io::Error::other(format!(
                "not a writable regular file: {}",
                path.display()
            )));
        }
        // Creating a sibling checks destination-directory access without changing the target.
        let probe = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        probe.as_file().sync_all()?;
        Ok(Self {
            original: digest(&fs::read(&path)?),
            path,
            agent,
            staged: None,
            backup: None,
        })
    }

    pub fn stage(&mut self, bytes: &[u8]) -> io::Result<()> {
        if digest(bytes) == self.original {
            return Ok(());
        }
        let mut backup = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        let mut source = File::open(&self.path)?;
        io::copy(&mut source, &mut backup)?;
        backup.as_file().sync_all()?;
        fs::set_permissions(backup.path(), fs::metadata(&self.path)?.permissions())?;
        self.backup = Some(backup.into_temp_path().keep().map_err(io::Error::other)?);
        let mut staged = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        staged.write_all(bytes)?;
        staged.as_file().sync_all()?;
        fs::set_permissions(staged.path(), fs::metadata(&self.path)?.permissions())?;
        self.staged = Some(staged.into_temp_path().keep().map_err(io::Error::other)?);
        Ok(())
    }

    fn validate(&self) -> io::Result<()> {
        if !fs::symlink_metadata(&self.path)?.is_file() {
            return Err(io::Error::other(format!(
                "target is no longer a regular file: {}",
                self.path.display()
            )));
        }
        if let Some(agent) = self.agent {
            if agent.read(&self.path)?.is_none() {
                return Err(io::Error::other("installed integration was removed"));
            }
        } else if digest(&fs::read(&self.path)?) != self.original {
            return Err(io::Error::other("binary changed while update was prepared"));
        }
        Ok(())
    }
}

impl Plan {
    pub fn changed(&self) -> bool {
        self.targets.iter().any(|target| target.staged.is_some())
    }

    pub fn commit(&mut self) -> io::Result<()> {
        for target in &self.targets {
            target.validate()?;
        }
        // Windows write sharing detects mapped executables and exclusive handles before
        // modifying any target. Do not interpret ordinary permission errors as busy.
        #[cfg(windows)]
        for target in &self.targets {
            if target.staged.is_some() {
                drop(OpenOptions::new().write(true).open(&target.path)?);
            }
        }
        let mut committed: Vec<usize> = Vec::new();
        for index in 0..self.targets.len() {
            let target = &self.targets[index];
            let Some(staged) = &target.staged else {
                continue;
            };
            let result = target
                .validate()
                .and_then(|()| fs::rename(staged, &target.path));
            if let Err(error) = result {
                let mut failures = Vec::new();
                for &previous in committed.iter().rev() {
                    let target = &self.targets[previous];
                    // Preserve the new payload for a possible sharing-violation retry.
                    let recovery = fs::rename(&target.path, target.staged.as_ref().unwrap())
                        .and_then(|()| {
                            fs::copy(target.backup.as_ref().unwrap(), &target.path).map(|_| ())
                        });
                    if let Err(rollback) = recovery {
                        failures.push(format!("{}: {rollback}", target.path.display()));
                    }
                }
                if !failures.is_empty() {
                    return Err(io::Error::other(format!(
                        "{error}; rollback failed: {}",
                        failures.join("; ")
                    )));
                }
                return Err(error);
            }
            committed.push(index);
        }
        Ok(())
    }

    pub fn report(&self) {
        for target in &self.targets {
            if target.staged.is_none() {
                continue;
            }
            println!("Updated {}", target.path.display());
            if let Some(agent) = target.agent {
                println!(
                    "Restart {} to activate; running sessions keep loaded code.",
                    agent.name()
                );
            }
        }
    }

    pub fn cleanup(&self) {
        for target in &self.targets {
            for path in [target.staged.as_ref(), target.backup.as_ref()]
                .into_iter()
                .flatten()
            {
                let _ = fs::remove_file(path);
            }
        }
    }
}

pub(super) fn busy(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        false
    }
}
