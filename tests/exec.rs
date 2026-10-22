use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::OnceLock,
};

fn fixture() -> &'static PathBuf {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let path = directory.join(format!("exec-child{}", std::env::consts::EXE_SUFFIX));
        assert!(
            Command::new("rustc")
                .args(["--edition=2024", "tests/fixtures/exec-child.rs", "-o"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    })
}

fn wrapper(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_outrim"));
    command.env("HOME", home).env("USERPROFILE", home);
    command
}

#[test]
fn binary_and_over_capacity_streams_return_every_byte_without_samples() {
    for mode in ["binary", "large"] {
        let home = tempfile::tempdir().unwrap();
        assert!(
            wrapper(home.path())
                .args(["gain", "record", "on"])
                .status()
                .unwrap()
                .success()
        );
        let output = wrapper(home.path())
            .args(["exec", "--"])
            .arg(fixture())
            .arg(mode)
            .output()
            .unwrap();
        assert!(output.status.success());
        if mode == "binary" {
            assert_eq!(output.stdout, b"\xff\x00\xfe\n");
            assert_eq!(output.stderr, b"\x01\x00text\n");
        } else {
            assert_eq!(output.stdout, vec![b'o'; 9 * 1024 * 1024]);
            assert_eq!(output.stderr, vec![b'e'; 9 * 1024 * 1024]);
        }
        let gain = wrapper(home.path()).arg("gain").output().unwrap();
        assert_eq!(gain.stdout, b"No gain data.\n");
        let instance = fs::read_dir(home.path().join(".outrim/records"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let events: Vec<serde_json::Value> = fs::read_to_string(instance.join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        for event in events {
            assert_eq!(event["diagnostics"]["output_origin"], "native_capture");
            assert_eq!(
                event["diagnostics"]["completeness"],
                if mode == "large" {
                    "capture_limit_passthrough"
                } else {
                    "complete"
                }
            );
            assert_eq!(event["diagnostics"]["limit_bytes"], 8 * 1024 * 1024);
            assert!(event["content"]["arguments"].is_null());
        }
    }
}

#[test]
fn cancellation_stops_descendants_in_both_modes() {
    for direct in [false, true] {
        let home = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        {
            let output = Command::new(fixture())
                .args([
                    "cancel-wrapper",
                    env!("CARGO_BIN_EXE_outrim"),
                    if direct { "direct" } else { "capture" },
                ])
                .env("HOME", home.path())
                .env("USERPROFILE", home.path())
                .current_dir(home.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        #[cfg(unix)]
        {
            let mut command = wrapper(home.path());
            command.arg("exec");
            if direct {
                command.arg("--no-compress");
            }
            let mut child = command
                .arg("--")
                .arg(fixture())
                .arg("tree")
                .current_dir(home.path())
                .spawn()
                .unwrap();
            let start = std::time::Instant::now();
            while !home.path().join("ready").exists() {
                assert!(start.elapsed() < std::time::Duration::from_secs(5));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            assert!(!child.wait().unwrap().success());
        }
        std::thread::sleep(std::time::Duration::from_millis(1400));
        assert!(!home.path().join("escaped").exists());
    }
}

#[test]
fn timeout_stops_descendants_in_both_modes() {
    for mode in [vec!["exec"], vec!["exec", "--no-compress"]] {
        let home = tempfile::tempdir().unwrap();
        let start = std::time::Instant::now();
        let output = wrapper(home.path())
            .args(mode)
            .args(["--timeout-ms", "400", "--"])
            .arg(fixture())
            .arg("tree")
            .current_dir(home.path())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        assert!(home.path().join("ready").exists());
        std::thread::sleep(std::time::Duration::from_millis(1400));
        assert!(!home.path().join("escaped").exists());
    }
}

fn original(bytes: &[u8]) -> PathBuf {
    PathBuf::from(
        std::str::from_utf8(bytes)
            .unwrap()
            .split("The full output is available at ")
            .nth(1)
            .expect("recovery hint")
            .trim(),
    )
}

fn raw_json() -> std::process::Output {
    let work = tempfile::tempdir().unwrap();
    Command::new(fixture())
        .arg("json")
        .current_dir(work.path())
        .output()
        .unwrap()
}

#[test]
fn storage_and_diagnostic_failures_never_replace_the_child_result() {
    let expected = raw_json();
    for broken in ["outputs", "gain.json", "records"] {
        let home = tempfile::tempdir().unwrap();
        let recording = wrapper(home.path())
            .args(["gain", "record", "on"])
            .output()
            .unwrap();
        assert!(recording.status.success());
        let path = home.path().join(".outrim").join(broken);
        if broken == "gain.json" {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, "blocked").unwrap();
        }
        let output = wrapper(home.path())
            .args(["exec", "--"])
            .arg(fixture())
            .args(["json", "one arg", "; not a shell"])
            .env("OUTRIM_FIXTURE", "inherited")
            .current_dir(home.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(19));
        if broken == "outputs" {
            assert_eq!(output.stdout, expected.stdout);
            assert_eq!(output.stderr, expected.stderr);
        } else {
            assert_eq!(
                std::fs::read(original(&output.stdout)).unwrap(),
                expected.stdout
            );
            assert_eq!(
                std::fs::read(original(&output.stderr)).unwrap(),
                expected.stderr
            );
        }
        assert_eq!(
            std::fs::read_to_string(home.path().join("executed")).unwrap(),
            "once"
        );
    }
}

#[test]
fn shared_execution_keeps_json_eligible_but_custom_logs_unchanged() {
    use outrim::{
        execution::{Context, Execution},
        protocol::{Host, OutputStream, Source},
    };
    for source in [None, Some(Source::TestLog)] {
        let home = tempfile::tempdir().unwrap();
        let text = if source.is_none() {
            String::from_utf8(raw_json().stdout).unwrap()
        } else {
            (0..100).map(|line| format!("log line {line}\n")).collect()
        };
        let execution = Execution::new(
            home.path(),
            &["bash".into(), "-c".into(), "opaque script".into()],
            Context {
                unit_id: "provided-unit".into(),
                host: Host::Pi,
                session: Some("real-session".into()),
                tool_call_id: Some("real-call".into()),
                source,
                recording: Some(true),
            },
        );
        let mut output = Vec::new();
        execution
            .deliver(OutputStream::Stdout, text.clone(), Some(0), &mut output)
            .unwrap();
        if source.is_none() {
            assert!(output.len() < text.len());
            assert_eq!(fs::read_to_string(original(&output)).unwrap(), text);
        } else {
            assert_eq!(output, text.as_bytes());
        }
        let detail = wrapper(home.path())
            .args(["gain", "--detail", "--session", "real-session"])
            .output()
            .unwrap();
        let detail = String::from_utf8(detail.stdout).unwrap();
        assert!(detail.contains("real-call"), "{detail}");
        assert!(
            detail.contains("provided-unit · stdout · recording true"),
            "{detail}"
        );
    }
}

#[test]
fn shared_source_classification_uses_complete_argv_words() {
    use outrim::command::source;
    for (argv, expected) in [
        (vec!["CARGO.EXE", "test"], "test_log"),
        (vec!["/usr/bin/cargo", "build"], "build_log"),
        (vec!["gh", "run", "watch", "123"], "runtime_log"),
        (vec!["rg", "--files"], "search"),
        (vec!["git", "ls-files"], "search"),
        (vec!["npm", "run", "test:unit"], "unknown"),
        (vec!["node", "-e", "anything"], "unknown"),
        (vec!["bash", "-c", "cargo test"], "unknown"),
    ] {
        let argv: Vec<_> = argv.into_iter().map(Into::into).collect();
        assert_eq!(serde_json::to_value(source(&argv)).unwrap(), expected);
    }
}

#[test]
fn closed_stdout_does_not_hide_stderr_or_replace_the_child_status() {
    let home = tempfile::tempdir().unwrap();
    let expected = raw_json();
    let mut child = wrapper(home.path())
        .args(["exec", "--"])
        .arg(fixture())
        .arg("json")
        .current_dir(home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(19));
    assert_eq!(fs::read(original(&output.stderr)).unwrap(), expected.stderr);
}

#[test]
fn concurrent_executions_keep_all_samples_and_originals() {
    let expected = raw_json();
    let home = tempfile::tempdir().unwrap();
    assert!(
        wrapper(home.path())
            .args(["gain", "record", "on"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut children = Vec::new();
    for index in 0..4 {
        let work = home.path().join(index.to_string());
        std::fs::create_dir(&work).unwrap();
        children.push(
            wrapper(home.path())
                .args(["exec", "--"])
                .arg(fixture())
                .args(["json", "one arg", "; not a shell"])
                .env("OUTRIM_FIXTURE", "inherited")
                .current_dir(work)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let mut references = std::collections::BTreeSet::new();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(19));
        for (bytes, expected) in [
            (&output.stdout, &expected.stdout),
            (&output.stderr, &expected.stderr),
        ] {
            let reference = original(bytes);
            assert_eq!(&std::fs::read(&reference).unwrap(), expected);
            assert!(references.insert(reference));
        }
    }
    let gain = wrapper(home.path())
        .args(["gain", "--detail"])
        .output()
        .unwrap();
    let gain = String::from_utf8(gain.stdout).unwrap();
    let summary = wrapper(home.path()).arg("gain").output().unwrap();
    assert!(
        String::from_utf8(summary.stdout)
            .unwrap()
            .contains("Processed outputs: 8")
    );
    assert_eq!(gain.matches(" · stdout · compressed").count(), 4, "{gain}");
    assert_eq!(gain.matches(" · stderr · compressed").count(), 4, "{gain}");
    assert_eq!(gain.matches("  Events:").count(), 8, "{gain}");
}

#[test]
fn captured_streams_have_independent_recovery_and_gain() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        wrapper(home.path())
            .args(["gain", "record", "on"])
            .status()
            .unwrap()
            .success()
    );
    let output = wrapper(home.path())
        .args(["exec", "--"])
        .arg(fixture())
        .arg("json")
        .current_dir(home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(19));
    let mut paths = Vec::new();
    for bytes in [&output.stdout, &output.stderr] {
        let text = std::str::from_utf8(bytes).unwrap();
        let path = text
            .split("The full output is available at ")
            .nth(1)
            .expect("recovery hint")
            .trim();
        let original = fs::read_to_string(path).unwrap();
        assert!(original.starts_with("{\n  \"items\""));
        assert!(text.chars().count() < original.chars().count());
        paths.push(path.to_owned());
    }
    assert_ne!(paths[0], paths[1]);
    assert_ne!(fs::read(&paths[0]).unwrap(), fs::read(&paths[1]).unwrap());
    let recovered = wrapper(home.path())
        .args(["exec", "--"])
        .arg(fixture())
        .arg("read")
        .arg(&paths[0])
        .output()
        .unwrap();
    assert!(recovered.status.success());
    assert_eq!(recovered.stdout, fs::read(&paths[0]).unwrap());
    assert!(recovered.stderr.is_empty());
    let gain = wrapper(home.path()).arg("gain").output().unwrap();
    let gain = String::from_utf8(gain.stdout).unwrap();
    assert!(gain.contains("Processed outputs: 3"));
    assert!(gain.contains("Recovery skipped: 1"), "{gain}");
    assert_eq!(
        fs::read_to_string(home.path().join("executed")).unwrap(),
        "once"
    );
    let detail = wrapper(home.path())
        .args(["gain", "--detail"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(detail.stdout)
            .unwrap()
            .contains("4 records")
    );
}

#[test]
fn passthrough_preserves_execution_without_collecting() {
    let home = tempfile::tempdir().unwrap();
    let mut child = wrapper(home.path())
        .args(["exec", "--no-compress", "--"])
        .arg(fixture())
        .args(["echo", "space and ; $literal", "--help"])
        .env("EXEC_VALUE", "inherited")
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"stdin\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let directory = stdout
        .strip_prefix("stdin\n[\"space and ; $literal\", \"--help\"]\ninherited\n")
        .unwrap()
        .strip_suffix('\n')
        .unwrap();
    assert_eq!(
        fs::canonicalize(directory).unwrap(),
        fs::canonicalize(home.path()).unwrap()
    );
    assert_eq!(output.stderr, b"separate stderr\n");
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}
