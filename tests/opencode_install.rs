use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run(home: &Path, config_home: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_outrim"));
    command
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home);
    if let Some(config_home) = config_home {
        command.env("XDG_CONFIG_HOME", config_home);
    } else {
        command.env_remove("XDG_CONFIG_HOME");
    }
    command.output().unwrap()
}

fn plugin_path(config_home: &Path) -> PathBuf {
    config_home.join("opencode/plugins/outrim.js")
}

#[test]
fn install_and_uninstall_touch_only_the_managed_opencode_plugin() {
    let home = tempfile::tempdir().unwrap();
    let config_home = home.path().join("xdg");
    let config_dir = config_home.join("opencode");
    let plugins = config_dir.join("plugins");
    fs::create_dir_all(&plugins).unwrap();
    let config = config_dir.join("opencode.json");
    fs::write(&config, r#"{"theme":"dark","plugins":["other-plugin"]}"#).unwrap();
    let other_plugin = plugins.join("other.js");
    fs::write(&other_plugin, "other plugin").unwrap();
    let pi_extension = home.path().join(".pi/agent/extensions/outrim.js");
    fs::create_dir_all(pi_extension.parent().unwrap()).unwrap();
    fs::write(&pi_extension, "Pi extension").unwrap();
    let original = home
        .path()
        .join(".outrim/outputs/opencode/session/original.txt");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, "saved output").unwrap();
    let binary = home.path().join("outrim");
    fs::write(&binary, "shared binary").unwrap();

    let first = run(
        home.path(),
        Some(&config_home),
        &["install", "--agent", "opencode"],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("Restart OpenCode to activate"));
    let installed_path = plugin_path(&config_home);
    let installed = fs::read_to_string(&installed_path).unwrap();
    assert!(installed.contains("outrim-managed: opencode-plugin"));
    let executable = serde_json::to_string(env!("CARGO_BIN_EXE_outrim")).unwrap();
    assert!(installed.contains(&format!("BRIDGE_EXECUTABLE = {executable};")));

    let repeated = run(
        home.path(),
        Some(&config_home),
        &["install", "--agent", "opencode"],
    );
    assert!(repeated.status.success());
    assert_eq!(fs::read_to_string(&installed_path).unwrap(), installed);

    let removed = run(
        home.path(),
        Some(&config_home),
        &["uninstall", "--agent", "opencode"],
    );
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(String::from_utf8_lossy(&removed.stdout).contains("Restart OpenCode to unload"));
    assert!(!installed_path.exists());
    assert_eq!(
        fs::read_to_string(config).unwrap(),
        r#"{"theme":"dark","plugins":["other-plugin"]}"#
    );
    assert_eq!(fs::read_to_string(other_plugin).unwrap(), "other plugin");
    assert_eq!(fs::read_to_string(pi_extension).unwrap(), "Pi extension");
    assert_eq!(fs::read_to_string(original).unwrap(), "saved output");
    assert_eq!(fs::read_to_string(binary).unwrap(), "shared binary");
}

#[test]
fn install_defaults_to_the_users_xdg_opencode_directory() {
    let home = tempfile::tempdir().unwrap();
    let output = run(home.path(), None, &["install", "--agent", "opencode"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(plugin_path(&home.path().join(".config")).is_file());
}

#[test]
fn install_refuses_to_replace_or_remove_an_unmanaged_plugin() {
    let home = tempfile::tempdir().unwrap();
    let config_home = home.path().join("xdg");
    let installed_path = plugin_path(&config_home);
    fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
    fs::write(&installed_path, "user-owned plugin").unwrap();

    let installed = run(
        home.path(),
        Some(&config_home),
        &["install", "--agent", "opencode"],
    );
    assert!(!installed.status.success());
    assert_eq!(
        fs::read_to_string(&installed_path).unwrap(),
        "user-owned plugin"
    );

    let removed = run(
        home.path(),
        Some(&config_home),
        &["uninstall", "--agent", "opencode"],
    );
    assert!(!removed.status.success());
    assert_eq!(
        fs::read_to_string(installed_path).unwrap(),
        "user-owned plugin"
    );
}
