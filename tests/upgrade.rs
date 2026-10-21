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

    fn run(&self, args: &[&str], release: &Release) -> Output {
        Command::new(&self.binary)
            .args(args)
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env("OUTRIM_RELEASE_API", &release.url)
            .output()
            .unwrap()
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
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("SHA256"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&install.binary).unwrap(), before);
        assert_eq!(
            fs::read_to_string(install.pi()).unwrap(),
            "// outrim-managed: pi-extension\nold"
        );
        assert!(!install.opencode().exists());
    }
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
        assert!(stdout.contains("Restart Pi"), "{stdout}");
        assert!(stdout.contains("Restart OpenCode"), "{stdout}");
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
        assert!(String::from_utf8_lossy(&repeated.stdout).contains("No changes"));
        assert_eq!(fs::read_to_string(install.pi()).unwrap(), pi);
    }
}
