use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

const EXECUTABLE_PLACEHOLDER: &str = "__OUTRIM_EXECUTABLE__";

/// The two generated integrations supported by installation and upgrade.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Agent {
    Pi,
    OpenCode,
}

struct Definition {
    name: &'static str,
    config_variable: &'static str,
    default_directory: &'static [&'static str],
    config_subdirectory: Option<&'static str>,
    plugin_directory: &'static str,
    marker: &'static str,
    template: &'static str,
}

impl Agent {
    pub const ALL: [Self; 2] = [Self::Pi, Self::OpenCode];

    fn definition(self) -> Definition {
        match self {
            Self::Pi => Definition {
                name: "Pi",
                config_variable: "PI_CODING_AGENT_DIR",
                default_directory: &[".pi", "agent"],
                config_subdirectory: None,
                plugin_directory: "extensions",
                marker: "outrim-managed: pi-extension",
                template: include_str!("../assets/outrim-pi.js"),
            },
            Self::OpenCode => Definition {
                name: "OpenCode",
                config_variable: "XDG_CONFIG_HOME",
                default_directory: &[".config"],
                config_subdirectory: Some("opencode"),
                plugin_directory: "plugins",
                marker: "outrim-managed: opencode-plugin",
                template: include_str!("../assets/outrim-opencode.js"),
            },
        }
    }

    pub fn name(self) -> &'static str {
        self.definition().name
    }

    pub fn config_variable(self) -> &'static str {
        self.definition().config_variable
    }

    pub fn directory(self, user_home: &Path, configured: Option<&OsStr>) -> PathBuf {
        let definition = self.definition();
        let directory = configured
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                definition
                    .default_directory
                    .iter()
                    .fold(user_home.to_path_buf(), |path, segment| path.join(segment))
            });
        match definition.config_subdirectory {
            Some(subdirectory) => directory.join(subdirectory),
            None => directory,
        }
    }

    pub fn path(self, directory: &Path) -> PathBuf {
        directory
            .join(self.definition().plugin_directory)
            .join("outrim.js")
    }

    pub fn read(self, path: &Path) -> io::Result<Option<String>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file() {
            return Err(unmanaged_plugin_error(path));
        }
        let existing = fs::read_to_string(path)?;
        if !existing.contains(self.definition().marker) {
            return Err(unmanaged_plugin_error(path));
        }
        Ok(Some(existing))
    }

    pub fn render(self, executable: &Path) -> io::Result<String> {
        let executable = executable
            .to_str()
            .ok_or_else(|| io::Error::other("Invalid executable path encoding"))?;
        let executable = serde_json::to_string(executable).map_err(io::Error::other)?;
        let executable_slot = format!("\"{EXECUTABLE_PLACEHOLDER}\"");
        let definition = self.definition();
        if definition.template.matches(&executable_slot).count() != 1 {
            return Err(io::Error::other(format!(
                "Invalid {} integration template",
                definition.name
            )));
        }
        Ok(definition.template.replace(&executable_slot, &executable))
    }

    pub fn install(self, directory: &Path, executable: &Path) -> io::Result<PathBuf> {
        let plugin = self.render(executable)?;
        let path = self.path(directory);
        fs::create_dir_all(path.parent().expect("integration has a parent directory"))?;
        if self.read(&path)?.as_deref() != Some(plugin.as_str()) {
            fs::write(&path, plugin)?;
        }
        Ok(path)
    }

    pub fn uninstall(self, directory: &Path) -> io::Result<Option<PathBuf>> {
        let path = self.path(directory);
        if self.read(&path)?.is_none() {
            return Ok(None);
        }
        fs::remove_file(&path)?;
        Ok(Some(path))
    }
}

/// Keep the positional upgrade wire format compatible with older binaries.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Templates([String; 2]);

impl Templates {
    pub fn render(executable: &Path) -> io::Result<Self> {
        Ok(Self([
            Agent::Pi.render(executable)?,
            Agent::OpenCode.render(executable)?,
        ]))
    }

    pub fn get(&self, agent: Agent) -> &str {
        match agent {
            Agent::Pi => &self.0[0],
            Agent::OpenCode => &self.0[1],
        }
    }
}

pub fn pi_agent_dir(user_home: &Path, configured_dir: Option<&OsStr>) -> PathBuf {
    Agent::Pi.directory(user_home, configured_dir)
}

pub fn opencode_config_dir(user_home: &Path, configured_home: Option<&OsStr>) -> PathBuf {
    Agent::OpenCode.directory(user_home, configured_home)
}

pub fn install_pi_extension(agent_dir: &Path, executable: &Path) -> io::Result<PathBuf> {
    Agent::Pi.install(agent_dir, executable)
}

pub fn uninstall_pi_extension(agent_dir: &Path) -> io::Result<Option<PathBuf>> {
    Agent::Pi.uninstall(agent_dir)
}

pub fn install_opencode_plugin(config_dir: &Path, executable: &Path) -> io::Result<PathBuf> {
    Agent::OpenCode.install(config_dir, executable)
}

pub fn uninstall_opencode_plugin(config_dir: &Path) -> io::Result<Option<PathBuf>> {
    Agent::OpenCode.uninstall(config_dir)
}

fn unmanaged_plugin_error(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("Not managed by outrim: {}", path.display()),
    )
}
