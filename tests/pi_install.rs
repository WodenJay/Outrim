use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run(home: &Path, agent_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_outrim"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PI_CODING_AGENT_DIR", agent_dir)
        .output()
        .unwrap()
}

fn run_with_default_agent_dir(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_outrim"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("PI_CODING_AGENT_DIR")
        .output()
        .unwrap()
}

fn extension_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("extensions/outrim.js")
}

#[test]
fn install_and_uninstall_touch_only_the_managed_extension() {
    let home = tempfile::tempdir().unwrap();
    let agent_dir = home.path().join("pi-agent");
    let extensions = agent_dir.join("extensions");
    fs::create_dir_all(&extensions).unwrap();
    let settings = agent_dir.join("settings.json");
    fs::write(&settings, r#"{"theme":"dark"}"#).unwrap();
    let unrelated_extension = extensions.join("other.js");
    fs::write(&unrelated_extension, "other extension").unwrap();
    let original = home.path().join(".outrim/outputs/pi/session/original.txt");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, "saved output").unwrap();
    let binary = home.path().join("outrim");
    fs::write(&binary, "shared binary").unwrap();

    let first = run(home.path(), &agent_dir, &["install", "--agent", "pi"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("Restart Pi to activate"));
    let installed_path = extension_path(&agent_dir);
    let installed = fs::read_to_string(&installed_path).unwrap();
    assert!(installed.contains("outrim-managed: pi-extension"));
    let executable = serde_json::to_string(env!("CARGO_BIN_EXE_outrim")).unwrap();
    assert!(installed.contains(&format!("BRIDGE_EXECUTABLE = {executable};")));

    let repeated = run(home.path(), &agent_dir, &["install", "--agent", "pi"]);
    assert!(repeated.status.success());
    assert_eq!(fs::read_to_string(&installed_path).unwrap(), installed);

    let removed = run(home.path(), &agent_dir, &["uninstall", "--agent", "pi"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(String::from_utf8_lossy(&removed.stdout).contains("Restart Pi to unload"));
    assert!(!installed_path.exists());
    assert_eq!(fs::read_to_string(settings).unwrap(), r#"{"theme":"dark"}"#);
    assert_eq!(
        fs::read_to_string(unrelated_extension).unwrap(),
        "other extension"
    );
    assert_eq!(fs::read_to_string(original).unwrap(), "saved output");
    assert_eq!(fs::read_to_string(binary).unwrap(), "shared binary");
}

#[test]
fn install_refuses_to_replace_an_unmanaged_extension() {
    let home = tempfile::tempdir().unwrap();
    let agent_dir = home.path().join("pi-agent");
    let installed_path = extension_path(&agent_dir);
    fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
    fs::write(&installed_path, "user-owned extension").unwrap();

    let output = run(home.path(), &agent_dir, &["install", "--agent", "pi"]);

    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(&installed_path).unwrap(),
        "user-owned extension"
    );

    let removed = run(home.path(), &agent_dir, &["uninstall", "--agent", "pi"]);
    assert!(!removed.status.success());
    assert_eq!(
        fs::read_to_string(installed_path).unwrap(),
        "user-owned extension"
    );
}

#[test]
fn install_defaults_to_the_current_users_pi_agent_directory() {
    let home = tempfile::tempdir().unwrap();
    let output = run_with_default_agent_dir(home.path(), &["install", "--agent", "pi"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed_path = home
        .path()
        .join(".pi")
        .join("agent")
        .join("extensions")
        .join("outrim.js");
    assert!(installed_path.is_file());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&installed_path.display().to_string())
    );

    let removed = run_with_default_agent_dir(home.path(), &["uninstall", "--agent", "pi"]);
    assert!(removed.status.success());
    assert!(
        String::from_utf8_lossy(&removed.stdout).contains(&installed_path.display().to_string())
    );
}
