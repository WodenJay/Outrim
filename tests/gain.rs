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

fn gain_command(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

#[test]
fn recording_is_explicit_persistent_and_detail_is_read_only() {
    let home = tempfile::tempdir().unwrap();
    let detail = gain_command(home.path(), &["--detail"]);
    assert!(detail.status.success());
    let report = String::from_utf8(detail.stdout).unwrap();
    assert!(report.contains("off"));
    assert!(
        report.contains(
            &home
                .path()
                .join(".outrim")
                .join("records")
                .display()
                .to_string()
        )
    );
    assert!(!home.path().join(".outrim").exists());

    let enabled = gain_command(home.path(), &["record", "on"]);
    assert!(enabled.status.success());
    let detail = gain_command(home.path(), &["--detail"]);
    assert!(String::from_utf8(detail.stdout).unwrap().contains("on"));
    assert!(
        gain_command(home.path(), &["record", "off"])
            .status
            .success()
    );
    let detail = gain_command(home.path(), &["--detail"]);
    assert!(String::from_utf8(detail.stdout).unwrap().contains("off"));
    assert!(!gain_command(home.path(), &["record"]).status.success());
    assert!(
        !gain_command(home.path(), &["record", "metadata"])
            .status
            .success()
    );
}

fn tool_request(id: &str, text: &str) -> Value {
    json!({
        "version": 2,
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

#[test]
fn recording_snapshots_startup_and_saves_real_text_separately_from_recovery() {
    let home = tempfile::tempdir().unwrap();
    let mut disabled = Bridge::start(home.path());
    // Force startup before changing the saved setting.
    disabled.exchange(&tool_request("ready", "small"));
    assert!(
        gain_command(home.path(), &["record", "on"])
            .status
            .success()
    );
    let mut active = Bridge::start(home.path());
    let text = "🦀 日志\n".repeat(100);
    let response = active.exchange(&tool_request("process", &text));
    assert_eq!(response["disposition"], "compressed");
    let observation = json!({
        "type": "record_observation", "version": 2, "id": "observe",
        "host": "pi", "session": "record-session", "tool_call_id": "host-call",
        "tool": tool_request("unused", &text)["tool"],
        "original_text": text, "final_text": response["text"],
        "outcome": response["disposition"], "applied_rule": response["applied_rule"],
        "original_path": response["original_path"], "candidates": response["candidates"],
        "reason": response["reason"]
    });
    assert_eq!(disabled.exchange(&observation)["acknowledged"], true);
    assert!(!home.path().join(".outrim/records").exists());
    assert_eq!(active.exchange(&observation)["acknowledged"], true);
    assert!(
        gain_command(home.path(), &["record", "off"])
            .status
            .success()
    );
    assert_eq!(active.exchange(&observation)["acknowledged"], true);
    disabled.stop();
    active.stop();
    let instance = fs::read_dir(home.path().join(".outrim/records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let lines = fs::read_to_string(instance.join("events.jsonl")).unwrap();
    let events: Vec<Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    let event = &events[0];
    assert_eq!(event["format_version"], 1);
    assert_ne!(event["event_id"], events[1]["event_id"]);
    assert_eq!(event["sequence"], 1);
    assert_eq!(events[1]["sequence"], 2);
    assert_eq!(event["tool_call_id"], "host-call");
    assert_eq!(event["before_chars"], 500);
    assert_eq!(
        event["after_chars"],
        count_codepoints(response["text"].as_str().unwrap())
    );
    assert_eq!(event["outrim_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(event["candidates"][0]["rule"], "log_tail");
    assert_eq!(event["candidates"][0]["reason"], "selected");
    let original = instance.join(event["content"]["original"].as_str().unwrap());
    assert_eq!(fs::read_to_string(&original).unwrap(), text);
    assert_eq!(
        fs::read_to_string(instance.join(event["content"]["final"].as_str().unwrap())).unwrap(),
        response["text"].as_str().unwrap()
    );
    let args: Value = serde_json::from_slice(
        &fs::read(instance.join(event["content"]["arguments"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    assert_eq!(args, json!({"command":"cargo test"}));
    fs::remove_file(response["original_path"].as_str().unwrap()).unwrap();
    assert_eq!(fs::read_to_string(original).unwrap(), text);
    let detail = gain_command(home.path(), &["--detail", "--session", "record-session"]);
    assert!(detail.status.success());
    let report = String::from_utf8(detail.stdout).unwrap();
    assert!(report.contains("compressed"));
    assert!(report.contains("host-call"));
    assert!(report.contains("chars 500 → "));
    assert!(!report.contains("Some("));
    let native_instance = home
        .path()
        .join(".outrim")
        .join("records")
        .join(instance.file_name().unwrap());
    for (label, filename) in [
        ("Events", "events.jsonl"),
        ("Arguments", event["content"]["arguments"].as_str().unwrap()),
        ("Original", event["content"]["original"].as_str().unwrap()),
        ("Final", event["content"]["final"].as_str().unwrap()),
    ] {
        assert!(report.contains(&format!(
            "  {label}: {}",
            native_instance.join(filename).display()
        )));
    }
    let mut restarted = Bridge::start(home.path());
    restarted.exchange(&observation);
    restarted.stop();
    assert_eq!(
        fs::read_dir(home.path().join(".outrim/records"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn recording_instances_remain_distinct_and_portable() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        gain_command(home.path(), &["record", "on"])
            .status
            .success()
    );
    std::thread::scope(|scope| {
        for worker in 0..2 {
            let home = home.path();
            scope.spawn(move || {
                let mut bridge = Bridge::start(home);
                for sequence in 1..=2 {
                    let text = format!("worker {worker} output {sequence} 🦀");
                    let response = bridge.exchange(&tool_request("process", &text));
                    let acknowledgement = bridge.exchange(&json!({
                        "type": "record_observation", "version": 2, "id": "same-transport-id",
                        "host": "pi", "session": "shared-session", "tool_call_id": null,
                        "tool": tool_request("unused", &text)["tool"],
                        "original_text": text, "final_text": response["text"],
                        "outcome": response["disposition"], "applied_rule": response["applied_rule"],
                        "original_path": response["original_path"], "candidates": response["candidates"],
                        "reason": response["reason"]
                    }));
                    assert_eq!(acknowledgement["acknowledged"], true);
                }
                bridge.stop();
            });
        }
    });
    let copied = tempfile::tempdir().unwrap();
    let mut ids = std::collections::BTreeSet::new();
    let mut texts = std::collections::BTreeSet::new();
    let mut instances = 0;
    for instance in fs::read_dir(home.path().join(".outrim/records")).unwrap() {
        let instance = instance.unwrap().path();
        instances += 1;
        let destination = copied.path().join(instance.file_name().unwrap());
        fs::create_dir(&destination).unwrap();
        for entry in fs::read_dir(&instance).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
        // Resolve all references from the copy, not the original directory.
        let lines = fs::read_to_string(destination.join("events.jsonl")).unwrap();
        for (index, line) in lines.lines().enumerate() {
            let event: Value = serde_json::from_str(line).unwrap();
            assert_eq!(event["sequence"], index + 1);
            assert!(ids.insert(event["event_id"].as_str().unwrap().to_owned()));
            for field in ["arguments", "original", "final"] {
                let relative = std::path::Path::new(event["content"][field].as_str().unwrap());
                assert!(relative.is_relative());
                let text = fs::read_to_string(destination.join(relative)).unwrap();
                if field == "original" {
                    texts.insert(text);
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&instance).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                assert_eq!(
                    fs::metadata(instance.join("events.jsonl"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }
    assert_eq!(instances, 2);
    assert_eq!(ids.len(), 4);
    assert_eq!(texts.len(), 4);
    let detail = gain_command(home.path(), &["--detail", "--session", "shared-session"]);
    assert!(detail.status.success());
    assert_eq!(
        String::from_utf8(detail.stdout)
            .unwrap()
            .matches("Outcome:")
            .count(),
        4
    );
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
        "version": 2,
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
        "version": 2,
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
    assert!(output.stderr.is_empty());
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
    assert!(report.contains(&"░".repeat(24)));
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
    let lock_path = home.path().join(".outrim/gain.lock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let probe = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        match probe.try_lock() {
            Ok(()) => probe.unlock().unwrap(),
            Err(std::fs::TryLockError::WouldBlock) => break,
            Err(std::fs::TryLockError::Error(error)) => {
                panic!("failed to probe gain lock: {error}")
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "bridge never acquired gain lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    std::thread::sleep(std::time::Duration::from_millis(130));
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
