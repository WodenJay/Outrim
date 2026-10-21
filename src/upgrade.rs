use outrim::install::Agent;
use serde::Deserialize;
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    prerelease: bool,
    draft: bool,
}

pub fn run(pi: &Path, opencode: &Path) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let mut installed = Vec::new();
    for (agent, directory) in [(Agent::Pi, pi), (Agent::OpenCode, opencode)] {
        let path = agent.path(directory);
        println!("Checking {}: {}", agent.name(), path.display());
        if let Some(current) = agent.read(&path)? {
            installed.push((agent, path, current));
        }
    }
    let api = std::env::var("OUTRIM_RELEASE_API")
        .unwrap_or_else(|_| "https://api.github.com/repos/WodenJay/Outrim/releases/latest".into());
    let mut response = ureq::get(&api)
        .header("User-Agent", "outrim-upgrade")
        .call()
        .map_err(io::Error::other)?;
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(1024 * 1024)
        .read_to_end(&mut body)?;
    let release: Release = serde_json::from_slice(&body).map_err(io::Error::other)?;
    if release.prerelease || release.draft {
        return Err(io::Error::other("release is not stable"));
    }
    let _version = semver::Version::parse(release.tag_name.trim_start_matches('v'))
        .map_err(io::Error::other)?;
    let mut changed = false;
    for (agent, path, current) in installed {
        let rendered = agent.render(&executable)?;
        if current == rendered {
            continue;
        }
        let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        staged.write_all(rendered.as_bytes())?;
        staged.as_file().sync_all()?;
        staged.persist(&path).map_err(|error| error.error)?;
        println!(
            "Updated {}. Restart {} to activate; running sessions keep loaded code.",
            path.display(),
            agent.name()
        );
        changed = true;
    }
    if !changed {
        println!("No changes needed.");
    }
    let _ = fs::metadata(&executable)?;
    Ok(())
}
