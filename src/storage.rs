use crate::protocol::Host;
#[cfg(unix)]
use std::fs;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

#[cfg(windows)]
mod windows;

pub(crate) struct Originals {
    root: PathBuf,
}

impl Originals {
    pub(crate) fn new(home: &Path) -> Self {
        Self {
            root: home.join(".outrim/outputs"),
        }
    }

    pub(crate) fn reserve(&self, host: Host, session: &str) -> io::Result<NamedTempFile> {
        let host = match host {
            Host::Pi => "pi",
            Host::Opencode => "opencode",
        };
        // Session IDs are opaque and unbounded; never use their bytes as a path
        // component. This fingerprint only groups files, not their identity:
        // random file names prevent overwrites even if two fingerprints collide.
        // Readers retain exact paths, so no hash algorithm is a recovery contract.
        let mut hasher = DefaultHasher::new();
        session.hash(&mut hasher);
        let session = format!("s-{:016x}", hasher.finish());
        let root = std::path::absolute(&self.root)?;
        let base = root
            .parent()
            .ok_or_else(|| io::Error::other("invalid output root"))?;
        for dir in [
            base.to_path_buf(),
            root.clone(),
            root.join(host),
            root.join(host).join(&session),
        ] {
            private_directory(&dir)?;
        }
        tempfile::Builder::new()
            .prefix("o-")
            .suffix(".txt")
            .tempfile_in(root.join(host).join(session))
    }
}

pub(crate) fn commit(mut file: NamedTempFile, text: &str) -> io::Result<PathBuf> {
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("invalid original path"))?,
    )?
    .sync_all()?;
    Ok(path)
}

#[cfg(windows)]
use windows::private_directory;

#[cfg(unix)]
fn private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other("original directory is not a directory"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
