use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

const PI_EXTENSION_NAME: &str = "outrim.js";
const PI_EXTENSION_MARKER: &str = "outrim-managed: pi-extension";
const OPENCODE_PLUGIN_NAME: &str = "outrim.js";
const OPENCODE_PLUGIN_MARKER: &str = "outrim-managed: opencode-plugin";
const EXECUTABLE_PLACEHOLDER: &str = "__OUTRIM_EXECUTABLE__";
const PI_EXTENSION: &str = include_str!("../assets/outrim-pi.js");
const OPENCODE_PLUGIN: &str = include_str!("../assets/outrim-opencode.js");

pub fn pi_agent_dir(user_home: &Path, configured_dir: Option<&OsStr>) -> PathBuf {
    configured_dir
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home.join(".pi").join("agent"))
}

pub fn opencode_config_dir(user_home: &Path, configured_home: Option<&OsStr>) -> PathBuf {
    configured_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home.join(".config"))
        .join("opencode")
}

pub fn install_pi_extension(agent_dir: &Path, executable: &Path) -> io::Result<PathBuf> {
    install_plugin(
        &agent_dir.join("extensions"),
        PI_EXTENSION_NAME,
        PI_EXTENSION_MARKER,
        PI_EXTENSION,
        executable,
        "Pi",
    )
}

pub fn uninstall_pi_extension(agent_dir: &Path) -> io::Result<Option<PathBuf>> {
    uninstall_plugin(
        &agent_dir.join("extensions"),
        PI_EXTENSION_NAME,
        PI_EXTENSION_MARKER,
        "Pi",
    )
}

pub fn install_opencode_plugin(config_dir: &Path, executable: &Path) -> io::Result<PathBuf> {
    install_plugin(
        &config_dir.join("plugins"),
        OPENCODE_PLUGIN_NAME,
        OPENCODE_PLUGIN_MARKER,
        OPENCODE_PLUGIN,
        executable,
        "OpenCode",
    )
}

pub fn uninstall_opencode_plugin(config_dir: &Path) -> io::Result<Option<PathBuf>> {
    uninstall_plugin(
        &config_dir.join("plugins"),
        OPENCODE_PLUGIN_NAME,
        OPENCODE_PLUGIN_MARKER,
        "OpenCode",
    )
}

fn install_plugin(
    directory: &Path,
    filename: &str,
    marker: &str,
    template: &str,
    executable: &Path,
    kind: &str,
) -> io::Result<PathBuf> {
    let plugin = render_plugin(template, executable, kind)?;
    fs::create_dir_all(directory)?;
    let path = directory.join(filename);

    if read_managed_plugin(&path, marker, kind)?.as_deref() == Some(plugin.as_str()) {
        return Ok(path);
    }

    fs::write(&path, plugin)?;
    Ok(path)
}

fn render_plugin(template: &str, executable: &Path, kind: &str) -> io::Result<String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid executable path encoding"))?;
    let executable = serde_json::to_string(executable).map_err(io::Error::other)?;
    let executable_slot = format!("\"{EXECUTABLE_PLACEHOLDER}\"");
    if template.matches(&executable_slot).count() != 1 {
        return Err(io::Error::other(format!(
            "Invalid {kind} integration template"
        )));
    }
    Ok(template.replace(&executable_slot, &executable))
}

fn uninstall_plugin(
    directory: &Path,
    filename: &str,
    marker: &str,
    kind: &str,
) -> io::Result<Option<PathBuf>> {
    let path = directory.join(filename);
    if read_managed_plugin(&path, marker, kind)?.is_none() {
        return Ok(None);
    }
    fs::remove_file(&path)?;
    Ok(Some(path))
}

fn read_managed_plugin(path: &Path, marker: &str, kind: &str) -> io::Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Err(unmanaged_plugin_error(path, kind));
    }
    let existing = fs::read_to_string(path)?;
    if !existing.contains(marker) {
        return Err(unmanaged_plugin_error(path, kind));
    }
    Ok(Some(existing))
}

/// The two generated integrations supported by installation and upgrade.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Agent {
    Pi,
    OpenCode,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pi => "Pi",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn path(self, directory: &Path) -> PathBuf {
        directory.join(match self {
            Self::Pi => "extensions/outrim.js",
            Self::OpenCode => "plugins/outrim.js",
        })
    }

    pub fn read(self, path: &Path) -> io::Result<Option<String>> {
        let marker = match self {
            Self::Pi => PI_EXTENSION_MARKER,
            Self::OpenCode => OPENCODE_PLUGIN_MARKER,
        };
        read_managed_plugin(path, marker, self.name())
    }

    pub fn render(self, executable: &Path) -> io::Result<String> {
        let template = match self {
            Self::Pi => PI_EXTENSION,
            Self::OpenCode => OPENCODE_PLUGIN,
        };
        render_plugin(template, executable, self.name())
    }
}

fn unmanaged_plugin_error(path: &Path, _kind: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("Not managed by outrim: {}", path.display()),
    )
}
