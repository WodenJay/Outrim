#[cfg(unix)]
use std::fs;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

#[cfg(windows)]
mod windows;

pub(crate) fn commit(mut file: NamedTempFile, text: &str) -> io::Result<PathBuf> {
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(
        path.parent()
            .ok_or_else(|| io::Error::other("Invalid original path"))?,
    )?
    .sync_all()?;
    Ok(path)
}

#[cfg(not(windows))]
pub(crate) fn replace(file: NamedTempFile, path: &Path) -> io::Result<()> {
    file.persist(path).map(|_| ()).map_err(|error| error.error)
}

#[cfg(windows)]
pub(crate) fn replace(mut file: NamedTempFile, path: &Path) -> io::Result<()> {
    use std::{thread, time::Duration};

    // Windows can deny replacement while a reader holds the file open; retry for 195ms.
    const RETRY_DELAYS_MS: [u64; 7] = [5, 10, 20, 40, 40, 40, 40];

    for delay_ms in RETRY_DELAYS_MS {
        match file.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) if error.error.kind() == io::ErrorKind::PermissionDenied => {
                file = error.file;
                thread::sleep(Duration::from_millis(delay_ms));
            }
            Err(error) => return Err(error.error),
        }
    }

    file.persist(path).map(|_| ()).map_err(|error| error.error)
}

#[cfg(windows)]
pub(crate) use windows::private_directory;

#[cfg(unix)]
pub(crate) fn private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other(format!(
            "Not a directory: {}",
            path.display()
        )));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
