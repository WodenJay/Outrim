use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

struct Bridge {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Bridge {
    fn start(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_outrim"))
            .arg("bridge")
            .env("HOME", home)
            .env("USERPROFILE", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    fn exchange(&mut self, request: &Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn stop(self) -> std::process::Output {
        let Self { child, input, .. } = self;
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        output
    }
}

fn tool_request(id: &str, text: &str) -> Value {
    json!({
        "version": 1,
        "id": id,
        "host": "pi",
        "session": "gain-test",
        "tool": {
            "name": "bash",
            "input": { "command": "cargo test" },
            "source": "test_log",
            "exit_code": 0
        },
        "text": text
    })
}

fn count_codepoints(text: &str) -> usize {
    text.chars().count()
}

fn delivery_request(
    id: &str,
    disposition: &str,
    applied_rule: Option<&str>,
    before_chars: u64,
    after_chars: u64,
) -> Value {
    json!({
        "type": "record_delivery",
        "version": 1,
        "id": id,
        "disposition": disposition,
        "applied_rule": applied_rule,
        "before_chars": before_chars,
        "after_chars": after_chars
    })
}

#[test]
fn gain_counts_an_acknowledged_delivery_and_attributes_its_rule() {
    let home = tempfile::tempdir().unwrap();
    let text = format!(
        "{}\n01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n",
        "🦀 old log line\n".repeat(100)
    );
    let mut bridge = Bridge::start(home.path());
    let response = bridge.exchange(&tool_request("process-1", &text));
    assert_eq!(response["disposition"], "compressed");
    assert_eq!(response["applied_rule"], "log_tail");

    let acknowledgement = bridge.exchange(&json!({
        "type": "record_delivery",
        "version": 1,
        "id": "delivery-1",
        "disposition": response["disposition"],
        "applied_rule": response["applied_rule"],
        "before_chars": count_codepoints(&text),
        "after_chars": count_codepoints(response["text"].as_str().unwrap())
    }));
    assert_eq!(acknowledgement["id"], "delivery-1");
    assert_eq!(acknowledgement["acknowledged"], true);
    let bridge_output = bridge.stop();
    assert!(bridge_output.stderr.is_empty());

    let stats_path = home.path().join(".outrim/gain.json");
    let persisted = fs::read_to_string(&stats_path).unwrap();
    let stats: Value = serde_json::from_str(&persisted).unwrap();
    assert_eq!(stats["before_chars"], count_codepoints(&text));
    assert_eq!(
        stats["after_chars"],
        count_codepoints(response["text"].as_str().unwrap())
    );
    assert!(!persisted.contains("old log line"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(home.path().join(".outrim"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&stats_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("Outputs: 1"), "{report}");
    assert!(report.contains("Tokens (est.)"), "{report}");
    assert!(report.contains("Log tail"), "{report}");
    assert!(report.contains("Compressed: 1"), "{report}");
}

#[test]
fn gain_persists_deliveries_across_bridge_restarts() {
    let home = tempfile::tempdir().unwrap();
    for id in ["delivery-1", "delivery-2"] {
        let mut bridge = Bridge::start(home.path());
        let acknowledgement = bridge.exchange(&delivery_request(id, "no_match", None, 100, 100));
        assert_eq!(acknowledgement["acknowledged"], true);
        assert!(bridge.stop().stderr.is_empty());
    }

    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("Outputs: 2"), "{report}");
    assert!(report.contains("No matching rule: 2"), "{report}");
}

#[test]
fn gain_shows_empty_state_without_creating_data_directory() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("No gain data yet.")
    );
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn gain_shows_an_empty_bar_when_deliveries_do_not_compress() {
    let home = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::start(home.path());
    let acknowledgement =
        bridge.exchange(&delivery_request("delivery-1", "no_match", None, 100, 100));
    assert_eq!(acknowledgement["acknowledged"], true);
    bridge.stop();

    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains(&format!("Savings: {} 0.0%", "░".repeat(24))));
}

#[test]
fn gain_aggregates_all_delivery_outcomes_across_writes() {
    let home = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::start(home.path());
    let deliveries = [
        ("compressed-1", "compressed", Some("json_compact"), 100, 40),
        ("compressed-2", "compressed", Some("json_compact"), 120, 60),
        ("compressed-3", "compressed", Some("log_tail"), 100, 50),
        ("protected", "protected", None, 50, 50),
        ("recovery", "recovery_skipped", None, 70, 70),
        ("not-shorter", "not_shorter", None, 80, 80),
        ("no-match", "no_match", None, 90, 90),
        ("failed-open", "failed_open", None, 110, 110),
    ];

    for (id, disposition, applied_rule, before_chars, after_chars) in deliveries {
        let acknowledgement = bridge.exchange(&delivery_request(
            id,
            disposition,
            applied_rule,
            before_chars,
            after_chars,
        ));
        assert_eq!(acknowledgement["id"], id);
        assert_eq!(acknowledgement["acknowledged"], true);
    }
    let bridge_output = bridge.stop();
    assert!(bridge_output.stderr.is_empty());

    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "Outputs: 8",
        "Saved:  42",
        "JSON compact",
        "Compressed: 3",
        "Protected: 1",
        "Recovery skipped: 1",
        "No reduction: 1",
        "No matching rule: 1",
        "Failed open: 1",
    ] {
        assert!(
            report.contains(expected),
            "missing {expected:?} in:\n{report}"
        );
    }
    assert!(report.find("JSON compact").unwrap() < report.find("Log tail").unwrap());
    let json_rule = report
        .lines()
        .find(|line| line.contains("JSON compact"))
        .unwrap();
    let log_rule = report
        .lines()
        .find(|line| line.contains("Log tail"))
        .unwrap();
    assert!(json_rule.contains("54.5%"), "{json_rule}");
    assert!(json_rule.ends_with("██████████"), "{json_rule}");
    assert!(log_rule.contains("50.0%"), "{log_rule}");
    assert!(log_rule.ends_with("████░░░░░░"), "{log_rule}");
}

#[cfg(windows)]
#[test]
fn gain_retries_when_a_reader_temporarily_blocks_replacement() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let home = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::start(home.path());
    let first = bridge.exchange(&delivery_request("first", "no_match", None, 100, 100));
    assert_eq!(first["acknowledged"], true);

    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(home.path().join(".outrim/gain.json"))
        .unwrap();
    let pending = std::thread::spawn(move || {
        let acknowledgement =
            bridge.exchange(&delivery_request("second", "no_match", None, 100, 100));
        (bridge, acknowledgement)
    });
    std::thread::sleep(std::time::Duration::from_millis(60));
    drop(blocker);

    let (bridge, acknowledgement) = pending.join().unwrap();
    assert!(bridge.stop().stderr.is_empty());
    assert_eq!(acknowledgement["acknowledged"], true);

    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("Outputs: 2"), "{report}");
}

#[test]
fn gain_aggregates_concurrent_bridge_processes() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_path_buf();

    std::thread::scope(|scope| {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let writers: Vec<_> = (0..4)
            .map(|worker| {
                let home = home_path.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let mut bridge = Bridge::start(&home);
                    barrier.wait();
                    for index in 0..50 {
                        let id = format!("worker-{worker}-{index}");
                        let response =
                            bridge.exchange(&delivery_request(&id, "no_match", None, 100, 100));
                        assert_eq!(response["acknowledged"], true);
                    }
                    assert!(bridge.stop().stderr.is_empty());
                })
            })
            .collect();
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let home = home_path.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..20 {
                        let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
                            .arg("gain")
                            .env("HOME", &home)
                            .env("USERPROFILE", &home)
                            .output()
                            .unwrap();
                        assert!(output.status.success());
                    }
                })
            })
            .collect();
        for worker in writers.into_iter().chain(readers) {
            worker.join().unwrap();
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", &home_path)
        .env("USERPROFILE", &home_path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.contains("Outputs: 200"), "{report}");
    assert!(report.contains("No matching rule: 200"), "{report}");
}

#[test]
fn stats_persistence_failure_does_not_block_processed_output() {
    let home = tempfile::tempdir().unwrap();
    let text = format!(
        "{}\n01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n",
        "failure test log\n".repeat(100)
    );
    let mut bridge = Bridge::start(home.path());
    let response = bridge.exchange(&tool_request("process-1", &text));
    assert_eq!(response["disposition"], "compressed");
    assert_ne!(response["text"], text);

    fs::create_dir(home.path().join(".outrim/gain.json")).unwrap();
    let acknowledgement = bridge.exchange(&delivery_request(
        "delivery-1",
        "compressed",
        response["applied_rule"].as_str(),
        count_codepoints(&text) as u64,
        count_codepoints(response["text"].as_str().unwrap()) as u64,
    ));
    assert_eq!(acknowledgement["acknowledged"], false);
    assert!(bridge.stop().stderr.is_empty());

    let gain = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(!gain.status.success());
    assert!(!gain.stderr.is_empty());
}
