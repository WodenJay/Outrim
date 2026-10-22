use crate::storage;
use serde::Deserialize;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};
use tempfile::NamedTempFile;

pub fn set_recording(home: &Path, enabled: bool) -> io::Result<String> {
    let directory = home.join(".outrim");
    storage::private_directory(&directory)?;
    let mut file = NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(&mut file, &serde_json::json!({ "recording": enabled }))?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    storage::replace(file, &directory.join("recording.json"))?;
    Ok(format!(
        "Recording {}. Restart hosts to apply.\n",
        if enabled { "on" } else { "off" }
    ))
}

pub(crate) fn enabled(home: &Path) -> io::Result<bool> {
    #[derive(Deserialize)]
    struct Settings {
        recording: bool,
    }
    match fs::read(home.join(".outrim/recording.json")) {
        Ok(bytes) => Ok(serde_json::from_slice::<Settings>(&bytes)?.recording),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
