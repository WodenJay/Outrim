use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

struct Release {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Release {
    fn current() -> Self {
        Self::serve(env!("CARGO_PKG_VERSION"), None, false)
    }

    fn serve(version: &str, binary: Option<Vec<u8>>, corrupt: bool) -> Self {
        use sha2::{Digest, Sha256};
        let version = version.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let base = url.clone();
        let worker = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let mut request = Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            let mut byte = [0];
                            if stream.read_exact(&mut byte).is_err() {
                                break;
                            }
                            request.push(byte[0]);
                        }
                        let asset = asset_name();
                        let request = String::from_utf8_lossy(&request);
                        let body = if request.starts_with("GET /download/") {
                            if request.contains("/SHA256SUMS ") {
                                let hash = if corrupt {
                                    "bad".to_string()
                                } else {
                                    Sha256::digest(binary.as_deref().unwrap_or_default())
                                        .iter()
                                        .map(|byte| format!("{byte:02x}"))
                                        .collect::<String>()
                                };
                                format!("{hash}  {asset}\n").into_bytes()
                            } else {
                                binary.clone().unwrap_or_default()
                            }
                        } else {
                            serde_json::to_vec(&serde_json::json!({"tag_name": format!("v{version}"), "prerelease": false, "draft": false,
                                "assets": [{"name": asset, "browser_download_url": format!("{base}/download/v{version}/{asset}")},
                                {"name": "SHA256SUMS", "browser_download_url": format!("{base}/download/v{version}/SHA256SUMS")}]})).unwrap()
                        };
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(&body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::yield_now()
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

struct Installation {
    home: tempfile::TempDir,
    binary: PathBuf,
}

impl Installation {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let binary = home
            .path()
            .join(format!("custom-outrim{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_outrim"), &binary).unwrap();
        Self { home, binary }
    }

    fn command(&self, args: &[&str], release: &Release) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env("OUTRIM_RELEASE_API", &release.url);
        command
    }

    fn run(&self, args: &[&str], release: &Release) -> Output {
        self.command(args, release).output().unwrap()
    }

    fn pi(&self) -> PathBuf {
        self.home.path().join(".pi/agent/extensions/outrim.js")
    }
    fn opencode(&self) -> PathBuf {
        self.home.path().join(".config/opencode/plugins/outrim.js")
    }
}

fn asset_name() -> String {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc.exe",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => panic!("unsupported test platform"),
    };
    format!("outrim-{target}")
}

#[test]
fn checksum_failure_preserves_every_formal_target() {
    let release = Release::serve("999.0.0", Some(b"invalid binary".to_vec()), true);
    for alias in ["upgrade", "update"] {
        let install = Installation::new();
        put(&install.pi(), "// outrim-managed: pi-extension\nold");
        let before = fs::read(&install.binary).unwrap();
        let output = install.run(&[alias], &release);
        assert!(!output.status.success());
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        assert_eq!(
            fs::read_to_string(install.pi()).unwrap(),
            "// outrim-managed: pi-extension\nold"
        );
        assert!(!install.opencode().exists());
    }
}

fn release_binary() -> Vec<u8> {
    static BINARY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BINARY
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let binary = directory
                .path()
                .join(format!("release{}", std::env::consts::EXE_SUFFIX));
            let output = Command::new("rustc")
                .args(["--edition=2024", "tests/fixtures/upgrade-release.rs", "-o"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            fs::read(binary).unwrap()
        })
        .clone()
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "update did not finish"
        );
        thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn new_release_updates_in_place_using_new_binary_resources() {
    let binary = release_binary();
    let release = Release::serve("999.0.0", Some(binary.clone()), false);
    for alias in ["upgrade", "update"] {
        let install = Installation::new();
        put(&install.pi(), "// outrim-managed: pi-extension\nold");
        let output = install.run(&[alias], &release);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        wait_until(|| fs::read(&install.binary).is_ok_and(|content| content == binary));
        wait_until(|| {
            fs::read_to_string(install.pi())
                .is_ok_and(|content| content.contains("new release fixture"))
        });
        let pi = fs::read_to_string(install.pi()).unwrap();
        assert!(
            pi.contains(&serde_json::to_string(&install.binary).unwrap()),
            "{pi}"
        );
        assert!(!install.opencode().exists());
        let version = Command::new(&install.binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&version.stdout).contains("999.0.0"));
    }
}

#[cfg(windows)]
struct Bridge(std::process::Child);
#[cfg(windows)]
impl Bridge {
    fn start(install: &Installation) -> Self {
        Self(
            Command::new(&install.binary)
                .arg("bridge")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .env("USERPROFILE", install.home.path())
                .spawn()
                .unwrap(),
        )
    }
}
#[cfg(windows)]
impl Drop for Bridge {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.kill();
        self.0.wait().unwrap();
    }
}

#[cfg(windows)]
#[test]
fn background_waits_for_all_real_bridges_and_keeps_the_original_release_and_scope() {
    let binary = release_binary();
    let release = Release::serve("999.0.0", Some(binary.clone()), false);
    for alias in ["upgrade", "update"] {
        let install = Installation::new();
        put(&install.pi(), "// outrim-managed: pi-extension\nold");
        let first = Bridge::start(&install);
        let second = Bridge::start(&install);
        let before = fs::read(&install.binary).unwrap();
        let output = install.run(&[alias], &release);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        assert_eq!(
            fs::read_to_string(install.pi()).unwrap(),
            "// outrim-managed: pi-extension\nold"
        );
        // A later release and newly installed Agent must not alter the waiting task.
        let later = Release::serve("1000.0.0", Some(b"must not be downloaded".to_vec()), false);
        put(
            &install.opencode(),
            "// outrim-managed: opencode-plugin\nadded later",
        );
        let repeated = install.run(&["update"], &later);
        assert!(repeated.status.success());
        drop(first);
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        drop(second);
        wait_until(|| {
            fs::read_to_string(install.pi()).is_ok_and(|text| text.contains("new release fixture"))
        });
        assert_eq!(fs::read(&install.binary).unwrap(), binary);
        assert_eq!(
            fs::read_to_string(install.opencode()).unwrap(),
            "// outrim-managed: opencode-plugin\nadded later"
        );
    }
}

#[cfg(windows)]
#[test]
fn background_respects_removed_or_unmanaged_integrations_and_reports_failure_next_time() {
    let release = Release::serve("999.0.0", Some(release_binary()), false);
    let current = Release::current();
    for replacement in [None, Some("user-owned now")] {
        let install = Installation::new();
        put(&install.pi(), "// outrim-managed: pi-extension\nold");
        let bridge = Bridge::start(&install);
        let before = fs::read(&install.binary).unwrap();
        let output = install.run(&["upgrade"], &release);
        assert!(output.status.success());
        if let Some(text) = replacement {
            fs::write(install.pi(), text).unwrap();
        } else {
            fs::remove_file(install.pi()).unwrap();
        }
        drop(bridge);
        let mut reported = false;
        wait_until(|| {
            let output = install.run(&["update"], &current);
            reported = !output.stderr.is_empty();
            reported
        });
        assert!(reported);
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        if let Some(text) = replacement {
            assert_eq!(fs::read_to_string(install.pi()).unwrap(), text);
        } else {
            assert!(!install.pi().exists());
        }
    }
}

#[test]
fn only_current_config_directories_and_existing_agents_are_updated_without_downgrading() {
    let release = Release::serve(
        "0.0.1",
        Some(b"must not install older release".to_vec()),
        false,
    );
    for custom in [false, true] {
        for mask in 0..4 {
            let install = Installation::new();
            let pi_dir = if custom {
                install.home.path().join("custom-pi")
            } else {
                install.home.path().join(".pi/agent")
            };
            let config = if custom {
                install.home.path().join("custom-config")
            } else {
                install.home.path().join(".config")
            };
            let pi = pi_dir.join("extensions/outrim.js");
            let opencode = config.join("opencode/plugins/outrim.js");
            if mask & 1 != 0 {
                put(&pi, "// outrim-managed: pi-extension\nold");
            }
            if mask & 2 != 0 {
                put(&opencode, "// outrim-managed: opencode-plugin\nold");
            }
            let historical = install.home.path().join("historical/extensions/outrim.js");
            put(&historical, "// outrim-managed: pi-extension\nhistorical");
            if custom {
                put(
                    &install.pi(),
                    "// outrim-managed: pi-extension\ndefault unchanged",
                );
            }
            let before = fs::read(&install.binary).unwrap();
            let mut command = install.command(&["update"], &release);
            if custom {
                command
                    .env("PI_CODING_AGENT_DIR", &pi_dir)
                    .env("XDG_CONFIG_HOME", &config);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(fs::read(&install.binary).unwrap(), before);
            assert_eq!(pi.exists(), mask & 1 != 0);
            assert_eq!(opencode.exists(), mask & 2 != 0);
            if pi.exists() {
                assert!(
                    fs::read_to_string(&pi)
                        .unwrap()
                        .contains("BRIDGE_EXECUTABLE")
                );
            }
            if opencode.exists() {
                assert!(
                    fs::read_to_string(&opencode)
                        .unwrap()
                        .contains("BRIDGE_EXECUTABLE")
                );
            }
            assert_eq!(
                fs::read_to_string(historical).unwrap(),
                "// outrim-managed: pi-extension\nhistorical"
            );
            if custom {
                assert_eq!(
                    fs::read_to_string(install.pi()).unwrap(),
                    "// outrim-managed: pi-extension\ndefault unchanged"
                );
            }
        }
    }
}

#[test]
fn preflight_conflict_and_download_failure_preserve_the_whole_installation() {
    let release = Release::current();
    for alias in ["upgrade", "update"] {
        let install = Installation::new();
        let before = fs::read(&install.binary).unwrap();
        put(&install.pi(), "// outrim-managed: pi-extension\nold");
        put(&install.opencode(), "my plugin");
        let output = install
            .command(&[alias], &release)
            .env("OUTRIM_RELEASE_API", "http://127.0.0.1:1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        assert_eq!(
            fs::read_to_string(install.pi()).unwrap(),
            "// outrim-managed: pi-extension\nold"
        );
        assert_eq!(fs::read_to_string(install.opencode()).unwrap(), "my plugin");
        fs::remove_file(install.opencode()).unwrap();
        let output = install
            .command(&[alias], &release)
            .env("OUTRIM_RELEASE_API", "http://127.0.0.1:1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        assert_eq!(
            fs::read_to_string(install.pi()).unwrap(),
            "// outrim-managed: pi-extension\nold"
        );
    }
}

#[test]
fn stale_task_with_reused_pid_does_not_block_the_next_update() {
    let install = Installation::new();
    let release = Release::current();
    let root = install.binary.with_file_name(format!(
        ".{}.outrim-update",
        install.binary.file_name().unwrap().to_string_lossy()
    ));
    let work = root.join("interrupted");
    put(
        &work.join(format!("helper{}", std::env::consts::EXE_SUFFIX)),
        "not the running process",
    );
    fs::write(
        root.join("active.json"),
        serde_json::to_vec(&serde_json::json!({"pid": std::process::id(), "work": work})).unwrap(),
    )
    .unwrap();
    put(&install.pi(), "// outrim-managed: pi-extension\nold");
    let output = install.run(&["upgrade"], &release);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stderr.is_empty());
    assert!(
        fs::read_to_string(install.pi())
            .unwrap()
            .contains("BRIDGE_EXECUTABLE")
    );
    assert!(!root.join("active.json").exists());
    assert!(
        work.exists(),
        "interrupted recovery files must not be discarded"
    );
}

#[cfg(windows)]
#[test]
fn background_waits_for_an_ordinary_integration_handle_without_upgrading_the_binary() {
    use std::os::windows::fs::OpenOptionsExt;
    let install = Installation::new();
    let release = Release::current();
    put(&install.pi(), "// outrim-managed: pi-extension\nold");
    let original_binary = fs::read(&install.binary).unwrap();
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(install.pi())
        .unwrap();
    let output = install.run(&["upgrade"], &release);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(install.pi()).unwrap(),
        "// outrim-managed: pi-extension\nold"
    );
    drop(handle);
    wait_until(|| {
        fs::read_to_string(install.pi()).is_ok_and(|text| text.contains("BRIDGE_EXECUTABLE"))
    });
    assert_eq!(fs::read(&install.binary).unwrap(), original_binary);
    wait_until(|| {
        let output = install.run(&["update"], &release);
        assert!(output.status.success());
        !output.stderr.is_empty()
    });
}

#[test]
fn readonly_target_is_not_reported_as_background_waiting() {
    let install = Installation::new();
    let release = Release::current();
    put(&install.pi(), "// outrim-managed: pi-extension\nold");
    let original = fs::metadata(install.pi()).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    fs::set_permissions(install.pi(), readonly).unwrap();
    let output = install.run(&["upgrade"], &release);
    fs::set_permissions(install.pi(), original).unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(install.pi()).unwrap(),
        "// outrim-managed: pi-extension\nold"
    );
}

#[cfg(unix)]
#[test]
fn symlink_integrations_are_never_followed() {
    let install = Installation::new();
    let release = Release::current();
    let user_file = install.home.path().join("user.js");
    put(&user_file, "// outrim-managed: pi-extension\nuser file");
    fs::create_dir_all(install.pi().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&user_file, install.pi()).unwrap();
    let output = install.run(&["upgrade"], &release);
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(user_file).unwrap(),
        "// outrim-managed: pi-extension\nuser file"
    );
}

#[cfg(unix)]
#[test]
fn old_bridge_keeps_running_while_new_processes_use_updated_disk_binary() {
    let install = Installation::new();
    let binary = release_binary();
    let release = Release::serve("999.0.0", Some(binary.clone()), false);
    let mut bridge = Command::new(&install.binary)
        .arg("bridge")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .env("HOME", install.home.path())
        .spawn()
        .unwrap();
    let output = install.run(&["upgrade"], &release);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&install.binary).unwrap(), binary);
    assert!(bridge.try_wait().unwrap().is_none());
    drop(bridge.stdin.take());
    bridge.wait().unwrap();
}

fn put(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn both_aliases_refresh_installed_integrations_even_at_latest_version() {
    let release = Release::current();
    for alias in ["upgrade", "update"] {
        let install = Installation::new();
        put(&install.pi(), "// outrim-managed: pi-extension\nold pi");
        put(
            &install.opencode(),
            "// outrim-managed: opencode-plugin\nold opencode",
        );
        let unrelated = install.home.path().join(".pi/agent/settings.json");
        put(&unrelated, "user settings");
        let before = fs::read(&install.binary).unwrap();
        let output = install.run(&[alias], &release);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout
                .replace('\\', "/")
                .contains(&install.pi().to_string_lossy().replace('\\', "/"))
        );
        let pi = fs::read_to_string(install.pi()).unwrap();
        assert!(pi.contains("BRIDGE_EXECUTABLE"));
        assert!(pi.contains(&serde_json::to_string(&install.binary).unwrap()));
        assert!(
            fs::read_to_string(install.opencode())
                .unwrap()
                .contains("BRIDGE_EXECUTABLE")
        );
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "user settings");
        let repeated = install.run(&[alias], &release);
        assert!(repeated.status.success());
        assert_eq!(fs::read_to_string(install.pi()).unwrap(), pi);
    }
}
