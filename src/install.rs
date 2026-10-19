use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

const EXTENSION_NAME: &str = "outrim.js";
const EXTENSION_MARKER: &str = "outrim-managed: pi-extension";
const EXECUTABLE_PLACEHOLDER: &str = "__OUTRIM_EXECUTABLE__";
const PI_EXTENSION: &str = include_str!("../assets/outrim-pi.js");

pub fn pi_agent_dir(user_home: &Path, configured_dir: Option<&OsStr>) -> PathBuf {
    configured_dir
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home.join(".pi/agent"))
}

pub fn install_pi_extension(agent_dir: &Path, executable: &Path) -> io::Result<PathBuf> {
    let executable = executable
        .to_str()
        .ok_or_else(|| io::Error::other("executable path is not valid Unicode"))?;
    let executable = serde_json::to_string(executable).map_err(io::Error::other)?;
    let executable_slot = format!("\"{EXECUTABLE_PLACEHOLDER}\"");
    if PI_EXTENSION.matches(&executable_slot).count() != 1 {
        return Err(io::Error::other(
            "embedded Pi extension has an invalid executable slot",
        ));
    }
    let extension = PI_EXTENSION.replace(&executable_slot, &executable);
    let extensions_dir = agent_dir.join("extensions");
    fs::create_dir_all(&extensions_dir)?;
    let path = extensions_dir.join(EXTENSION_NAME);

    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let existing = fs::read_to_string(&path)?;
            if !existing.contains(EXTENSION_MARKER) {
                return Err(unmanaged_extension_error(&path));
            }
            if existing == extension {
                return Ok(path);
            }
        }
        Ok(_) => return Err(unmanaged_extension_error(&path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    fs::write(&path, extension)?;
    Ok(path)
}

pub fn uninstall_pi_extension(agent_dir: &Path) -> io::Result<Option<PathBuf>> {
    let path = agent_dir.join("extensions").join(EXTENSION_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Err(unmanaged_extension_error(&path));
    }
    let existing = fs::read_to_string(&path)?;
    if !existing.contains(EXTENSION_MARKER) {
        return Err(unmanaged_extension_error(&path));
    }
    fs::remove_file(&path)?;
    Ok(Some(path))
}

fn unmanaged_extension_error(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "refusing to replace or remove unmanaged Pi extension at {}",
            path.display()
        ),
    )
}
