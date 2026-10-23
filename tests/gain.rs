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
fn independent_commands_reuse_a_resident_bookkeeper() {
    let home = tempfile::tempdir().unwrap();
    let delivery = json!({
        "type": "record_delivery", "version": 2, "id": "same-local-id",
        "disposition": "no_match", "applied_rule": null,
        "original_text": "hello world", "final_text": "hello world"
    });
    let mut first = Bridge::start(home.path());
    assert_eq!(first.exchange(&delivery)["acknowledged"], true);
    first.stop();
    let endpoint_path = home.path().join(".outrim/bookkeeping.json");
    let endpoint = fs::read(&endpoint_path).expect("resident endpoint");
    let mut second = Bridge::start(home.path());
    assert_eq!(second.exchange(&delivery)["acknowledged"], true);
    second.stop();
    assert_eq!(fs::read(endpoint_path).unwrap(), endpoint);
    let report = String::from_utf8(gain_command(home.path(), &[]).stdout).unwrap();
    assert!(report.contains("Processed outputs: 2"), "{report}");
}

#[test]
fn a_crashed_resident_restarts_without_losing_committed_totals() {
    let home = tempfile::tempdir().unwrap();
    let delivery = json!({
        "type":"record_delivery", "version":2, "id":"reused-id",
        "disposition":"no_match", "applied_rule":null,
        "original_text":"hello world", "final_text":"hello world"
    });
    let mut first = Bridge::start(home.path());
    assert_eq!(first.exchange(&delivery)["acknowledged"], true);
    first.stop();
    let endpoint_path = home.path().join(".outrim/bookkeeping.json");
    let old: Value = serde_json::from_slice(&fs::read(&endpoint_path).unwrap()).unwrap();
    let pid = old["pid"].as_u64().unwrap() as u32;
    #[cfg(windows)]
    // SAFETY: the PID belongs to the worker started in this test's private home.
    unsafe {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
                WaitForSingleObject,
            },
        };
        let process = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        assert!(!process.is_null());
        assert_ne!(TerminateProcess(process, 1), 0);
        assert_eq!(WaitForSingleObject(process, 5000), 0);
        CloseHandle(process);
    }
    #[cfg(unix)]
    // SAFETY: this is the isolated worker PID, and SIGKILL takes no pointers.
    unsafe {
        assert_eq!(libc::kill(pid as i32, libc::SIGKILL), 0);
    }
    let mut second = Bridge::start(home.path());
    assert_eq!(second.exchange(&delivery)["acknowledged"], true);
    second.stop();
    let new: Value = serde_json::from_slice(&fs::read(&endpoint_path).unwrap()).unwrap();
    assert_ne!(new["secret"], old["secret"]);
    let report = String::from_utf8(gain_command(home.path(), &[]).stdout).unwrap();
    assert!(report.contains("Processed outputs: 2"), "{report}");
}

#[test]
fn exact_delivery_counts_use_full_o200k_text_and_preserve_negative_savings() {
    // Expected counts confirmed with Python tiktoken's o200k_base, ordinary text.
    for (original, final_text, before, after) in [
        ("hello world", "hello world", 2, 2),
        ("你好世界", "你好世界", 2, 2),
        ("fn main() { println!(\"hello\"); }", "", 9, 0),
        ("{\"ok\":true,\"n\":42}", "", 9, 0),
        ("C:/Users/alice/project/src/main.rs", "", 9, 0),
        (" \t\n", "", 1, 0),
        ("🦀 café é 中文", "", 7, 0),
        ("", "", 0, 0),
        ("aaaaaaaaaaaaaaaaaaaa", "<|endoftext|>", 3, 7),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut bridge = Bridge::start(home.path());
        let ack = bridge.exchange(&json!({
            "type": "record_delivery", "version": 2, "id": "exact",
            "disposition": "compressed", "applied_rule": "custom_rule",
            "original_text": original, "final_text": final_text
        }));
        assert_eq!(ack["acknowledged"], true, "{ack}");
        bridge.stop();
        let stats: Value =
            serde_json::from_slice(&fs::read(home.path().join(".outrim/gain.json")).unwrap())
                .unwrap();
        assert_eq!(stats["rules"]["custom_rule"]["before_tokens"], before);
        assert_eq!(stats["rules"]["custom_rule"]["after_tokens"], after);
        let report = String::from_utf8(gain_command(home.path(), &[]).stdout).unwrap();
        assert!(
            report.contains(&format!("Compression savings: {} tokens", before - after)),
            "{report}"
        );
        if after > before {
            assert!(report.contains("Compression rate: -133%"), "{report}");
            assert!(report.contains(&"░".repeat(24)), "{report}");
        }
        assert!(!home.path().join(".outrim/records").exists());
    }
}

#[test]
fn whole_delivery_and_recovery_counts_match_recording_without_archiving_recovery_text() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        gain_command(home.path(), &["record", "on"])
            .status
            .success()
    );
    let original = "aaaaaaaaaaaaaaaaaaaa";
    let final_text = "hello\n[outrim] original: C:/tmp/full.txt\nUse read to recover.";
    let mut bridge = Bridge::start(home.path());
    let observation = json!({
        "type": "record_observation", "version": 2, "id": "facts", "host": "pi",
        "session": "whole", "tool": {"name": "read", "input": {}, "source": "unknown"},
        "original_text": original, "final_text": final_text,
        "outcome": "compressed", "applied_rule": "custom_rule", "candidates": []
    });
    assert_eq!(
        bridge.exchange(&json!({
            "type": "record_delivery", "version": 2, "id": "delivery",
            "disposition": "compressed", "applied_rule": "custom_rule",
            "original_text": original, "final_text": final_text, "observation": observation
        }))["acknowledged"],
        true
    );
    assert_eq!(
        bridge.exchange(&json!({
            "type": "record_recovery", "version": 2, "id": "recovery", "host": "pi",
            "event_id": "whole/recovery", "session": "whole", "tool_call_id": "read",
            "text": "recovered original\n<|endoftext|>\nUse read to recover.",
            "attribution": "confirmed"
        }))["acknowledged"],
        true
    );
    bridge.stop();
    let stats: Value =
        serde_json::from_slice(&fs::read(home.path().join(".outrim/gain.json")).unwrap()).unwrap();
    assert_eq!(stats["rules"]["custom_rule"]["before_tokens"], 3);
    assert_eq!(stats["rules"]["custom_rule"]["after_tokens"], 19);
    assert_eq!(stats["recovery_tokens"], 16);
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
    assert_eq!(events[0]["token_encoding"], "o200k_base");
    assert_eq!(events[0]["before_tokens"], 3);
    assert_eq!(events[0]["after_tokens"], 19);
    assert_eq!(events[1]["diagnostics"]["recovery"]["tokens"], 16);
    assert!(events[1]["content"]["original"].is_null());
    assert!(!lines.contains("recovered original"));
    let report = String::from_utf8(gain_command(home.path(), &[]).stdout).unwrap();
    assert!(
        report.contains("Compression savings: -16 tokens"),
        "{report}"
    );
    assert!(report.contains("Net savings: -32 tokens"), "{report}");
    let detail =
        String::from_utf8(gain_command(home.path(), &["--detail", "--session", "whole"]).stdout)
            .unwrap();
    assert!(detail.contains("tokens 3 → 19"), "{detail}");
}

#[test]
fn aggregate_rate_includes_unchanged_deliveries_across_processes() {
    let home = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::start(home.path());
    for disposition in [
        "protected",
        "no_match",
        "failed_open",
        "not_shorter",
        "recovery_skipped",
    ] {
        assert_eq!(
            bridge.exchange(&json!({
                "type": "record_delivery", "version": 2, "id": disposition,
                "disposition": disposition, "applied_rule": null,
                "original_text": "hello world", "final_text": "hello world"
            }))["acknowledged"],
            true
        );
    }
    bridge.stop();
    let mut bridge = Bridge::start(home.path());
    assert_eq!(
        bridge.exchange(&json!({
            "type": "record_delivery", "version": 2, "id": "compressed",
            "disposition": "compressed", "applied_rule": "custom_rule",
            "original_text": "hello world", "final_text": "hello"
        }))["acknowledged"],
        true
    );
    bridge.stop();
    let stats: Value =
        serde_json::from_slice(&fs::read(home.path().join(".outrim/gain.json")).unwrap()).unwrap();
    assert_eq!(stats["before_tokens"], 12);
    assert_eq!(stats["after_tokens"], 11);
    assert_eq!(stats["rules"]["custom_rule"]["before_tokens"], 2);
    assert_eq!(stats["rules"]["custom_rule"]["after_tokens"], 1);
    let report = String::from_utf8(gain_command(home.path(), &[]).stdout).unwrap();
    assert!(report.contains("Compression savings: 1 tokens"), "{report}");
    assert!(report.contains("Compression rate: 8%"), "{report}");
    assert!(report.contains("50%"), "{report}");
}

#[test]
fn gain_renders_token_summary_and_rule_totals() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join(".outrim")).unwrap();
    fs::write(
        home.path().join(".outrim/gain.json"),
        json!({
            "before_tokens": 97320, "after_tokens": 44704,
            "rules": {
                "log_compaction": { "count": 69, "before_tokens": 60000, "after_tokens": 12000 },
                "search_hit_groups": { "count": 33, "before_tokens": 35000, "after_tokens": 30500 },
                "path_prefix_groups": { "count": 6, "before_tokens": 2320, "after_tokens": 2204 }
            },
            "outcomes": { "compressed": 108, "protected": 376, "recovery_skipped": 12,
                "not_shorter": 123, "no_match": 418, "failed_open": 0 },
            "recovery_tokens": 0, "recovery_events": []
        })
        .to_string(),
    )
    .unwrap();
    let output = gain_command(home.path(), &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "Gain\n\n",
            "Processed outputs: 1,037\n",
            "Compression savings: 52.6k tokens\n",
            "Compression rate: 54%\n",
            "Recovery cost: 0 tokens\n",
            "Net savings: 52.6k tokens\n",
            "\nSavings: █████████████░░░░░░░░░░░\n",
            "\nBy rule\n",
            "Rule                     Count      Saved   Rate  Impact\n",
            "Log compaction              69        48k    80%  ██████████\n",
            "Search groups               33       4.5k    13%  █░░░░░░░░░\n",
            "Path prefix groups           6        116     5%  █░░░░░░░░░\n",
            "\nOutcomes\n",
            "  Compressed: 108\n",
            "  Protected: 376\n",
            "  Recovery skipped: 12\n",
            "  No reduction: 123\n",
            "  No matching rule: 418\n",
            "  Failed open: 0\n"
        )
    );
}

#[test]
fn gain_formats_token_boundaries_and_preserves_unknown_rule_labels() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join(".outrim")).unwrap();
    for (tokens, saved) in [
        (998, "998"),
        (999, "999"),
        (1000, "1k"),
        (48000, "48k"),
        (999949, "999.9k"),
        (999950, "1M"),
        (1000000, "1M"),
        (1769000, "1.77M"),
        (1100000, "1.1M"),
    ] {
        fs::write(home.path().join(".outrim/gain.json"), json!({
            "before_tokens": tokens, "after_tokens": 0,
            "rules": { "custom_rule": { "count": 1234, "before_tokens": tokens, "after_tokens": 0 } },
            "outcomes": { "compressed": 1234 },
            "recovery_tokens": 0, "recovery_events": []
        }).to_string()).unwrap();
        let output = gain_command(home.path(), &[]);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                concat!(
                    "Gain\n\n",
                    "Processed outputs: 1,234\n",
                    "Compression savings: {saved} tokens\n",
                    "Compression rate: 100%\n",
                    "Recovery cost: 0 tokens\n",
                    "Net savings: {saved} tokens\n",
                    "\nSavings: ████████████████████████\n",
                    "\nBy rule\n",
                    "Rule                     Count      Saved   Rate  Impact\n",
                    "custom_rule              1,234 {saved:>10}   100%  ██████████\n",
                    "\nOutcomes\n",
                    "  Compressed: 1,234\n",
                    "  Protected: 0\n",
                    "  Recovery skipped: 0\n",
                    "  No reduction: 0\n",
                    "  No matching rule: 0\n",
                    "  Failed open: 0\n"
                ),
                saved = saved
            ),
            "{tokens} tokens"
        );
    }
}

#[test]
fn gain_distinguishes_empty_outcomes_from_recovery_and_zero_count_data() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join(".outrim")).unwrap();
    for (outcomes, events, recovery, expected) in [
        (json!({}), json!([]), 0, "No gain data.\n".to_owned()),
        (
            json!({"protected": 0}),
            json!([]),
            0,
            zero_output_report("0", "0"),
        ),
        (
            json!({}),
            json!(["unattributed-event"]),
            0,
            zero_output_report("0", "0"),
        ),
        (
            json!({}),
            json!(["confirmed-event"]),
            1234500,
            zero_output_report("1.23M", "-1.23M"),
        ),
    ] {
        fs::write(
            home.path().join(".outrim/gain.json"),
            json!({
                "before_tokens": 0, "after_tokens": 0,
                "rules": {}, "outcomes": outcomes,
                "recovery_tokens": recovery, "recovery_events": events
            })
            .to_string(),
        )
        .unwrap();
        let output = gain_command(home.path(), &[]);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
}

fn zero_output_report(recovery: &str, net: &str) -> String {
    format!(
        concat!(
            "Gain\n\n",
            "Processed outputs: 0\n",
            "Compression savings: 0 tokens\n",
            "Compression rate: 0%\n",
            "Recovery cost: {recovery} tokens\n",
            "Net savings: {net} tokens\n",
            "\nSavings: ░░░░░░░░░░░░░░░░░░░░░░░░\n",
            "\nBy rule\n",
            "Rule                     Count      Saved   Rate  Impact\n",
            "No compressed outputs.\n",
            "\nOutcomes\n",
            "  Compressed: 0\n",
            "  Protected: 0\n",
            "  Recovery skipped: 0\n",
            "  No reduction: 0\n",
            "  No matching rule: 0\n",
            "  Failed open: 0\n"
        ),
        recovery = recovery,
        net = net
    )
}

#[test]
fn gain_rejects_old_incomplete_and_invalid_statistics_without_overwriting() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join(".outrim")).unwrap();
    let path = home.path().join(".outrim/gain.json");
    let complete = json!({
        "before_tokens": 100, "after_tokens": 20,
        "rules": { "json_toon": { "count": 1, "before_tokens": 100, "after_tokens": 20 } },
        "outcomes": { "compressed": 1 }, "recovery_tokens": 0, "recovery_events": []
    });
    let mut invalid = vec!["{invalid}".to_owned()];
    let mut old = complete.clone();
    old["outputs"] = json!(1);
    old["before_chars"] = json!(100);
    old["after_chars"] = json!(20);
    invalid.push(old.to_string());
    for field in [
        "before_tokens",
        "after_tokens",
        "rules",
        "outcomes",
        "recovery_tokens",
        "recovery_events",
    ] {
        let mut missing = complete.clone();
        missing.as_object_mut().unwrap().remove(field);
        invalid.push(missing.to_string());
    }
    for field in ["count", "before_tokens", "after_tokens"] {
        let mut missing = complete.clone();
        missing["rules"]["json_toon"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        invalid.push(missing.to_string());
    }
    for text in invalid {
        fs::write(&path, &text).unwrap();
        let output = gain_command(home.path(), &[]);
        assert!(!output.status.success(), "{text}: {output:?}");
        assert!(!output.stderr.is_empty());
        assert!(output.stdout.is_empty());
        let mut bridge = Bridge::start(home.path());
        assert_eq!(
            bridge.exchange(&delivery_request("invalid-stats", "no_match", None, 10, 10))["acknowledged"],
            false
        );
        bridge.stop();
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
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

fn log() -> String {
    let tests = (0..100)
        .map(|n| format!("test case_{n} ... ok\n"))
        .collect::<String>();
    format!(
        "running 100 tests\n{tests}test result: ok. 100 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"
    )
}

#[test]
fn recovery_cost_is_independent_deduplicated_and_can_exceed_savings() {
    for recording in [false, true] {
        let home = tempfile::tempdir().unwrap();
        if recording {
            assert!(
                gain_command(home.path(), &["record", "on"])
                    .status
                    .success()
            );
        }
        let mut bridge = Bridge::start(home.path());
        let delivery = json!({
            "type": "record_delivery", "version": 2, "id": "delivery",
            "disposition": "compressed", "applied_rule": "log_compaction",
            "original_text": " a".repeat(250), "final_text": " a".repeat(50)
        });
        assert_eq!(bridge.exchange(&delivery)["acknowledged"], true);
        let recovery = json!({
            "type": "record_recovery", "version": 2, "id": "transport-1",
            "event_id": "pi/session/call-1/recovery", "host": "pi",
            "session": "session", "tool_call_id": "call-1",
            "text": " a".repeat(150), "attribution": "confirmed"
        });
        assert_eq!(bridge.exchange(&recovery)["acknowledged"], true);
        let mut retry = recovery.clone();
        retry["id"] = json!("transport-2");
        assert_eq!(bridge.exchange(&retry)["acknowledged"], true);
        let mut repeated = recovery;
        repeated["event_id"] = json!("pi/session/call-2/recovery");
        repeated["tool_call_id"] = json!("call-2");
        assert_eq!(bridge.exchange(&repeated)["acknowledged"], true);
        bridge.stop();
        let report = String::from_utf8(gain_command(home.path(), &[]).stdout).unwrap();
        assert!(
            report.contains("Compression savings: 200 tokens"),
            "{report}"
        );
        assert!(report.contains("Recovery cost: 300 tokens"), "{report}");
        assert!(report.contains("Net savings: -100 tokens"), "{report}");
        assert_eq!(home.path().join(".outrim/records").exists(), recording);
    }
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
    let text = log();
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
    assert_eq!(event["before_chars"], count_codepoints(&text));
    assert_eq!(
        event["after_chars"],
        count_codepoints(response["text"].as_str().unwrap())
    );
    assert_eq!(event["outrim_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(event["candidates"][0]["rule"], "log_compaction");
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
    assert!(report.contains("tokens 739 → "), "{report}");
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
fn recording_detail_ignores_unfinished_lines_but_rejects_invalid_complete_events() {
    let home = tempfile::tempdir().unwrap();
    outrim::gain::set_recording(home.path(), true).unwrap();
    let mut bridge = Bridge::start(home.path());
    assert_eq!(
        bridge.exchange(&json!({
            "type": "record_observation", "version": 2, "id": "observe",
            "host": "pi", "session": "reader-test",
            "tool": tool_request("unused", "small")["tool"],
            "original_text": "small", "final_text": "small", "outcome": "not_shorter",
            "candidates": []
        }))["acknowledged"],
        true
    );
    bridge.stop();
    let instance = fs::read_dir(home.path().join(".outrim/records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = instance.join("events.jsonl");
    let complete = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{complete}{{\"format_version\":")).unwrap();
    let report = outrim::gain::render_detail(home.path(), Some("reader-test")).unwrap();
    assert_eq!(report.matches("Outcome:").count(), 1);
    assert!(
        outrim::gain::render_detail(home.path(), None)
            .unwrap()
            .contains("1 records")
    );

    fs::write(&path, format!("{complete}{{invalid}}\n")).unwrap();
    assert!(outrim::gain::render_detail(home.path(), None).is_err());

    let mut event: Value = serde_json::from_str(complete.trim_end()).unwrap();
    event["format_version"] = json!(2);
    fs::write(&path, format!("{event}\n")).unwrap();
    assert_eq!(
        outrim::gain::render_detail(home.path(), None)
            .unwrap_err()
            .to_string(),
        "Unsupported recording format"
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
    before_tokens: u64,
    after_tokens: u64,
) -> Value {
    json!({
        "type": "record_delivery",
        "version": 2,
        "id": id,
        "disposition": disposition,
        "applied_rule": applied_rule,
        "original_text": " a".repeat(before_tokens as usize),
        "final_text": " a".repeat(after_tokens as usize)
    })
}

#[test]
fn gain_counts_an_acknowledged_delivery_and_attributes_its_rule() {
    let home = tempfile::tempdir().unwrap();
    let text = log();
    let mut bridge = Bridge::start(home.path());
    let response = bridge.exchange(&tool_request("process-1", &text));
    assert_eq!(response["disposition"], "compressed");
    assert_eq!(response["applied_rule"], "log_compaction");

    let acknowledgement = bridge.exchange(&json!({
        "type": "record_delivery",
        "version": 2,
        "id": "delivery-1",
        "disposition": response["disposition"],
        "applied_rule": response["applied_rule"],
        "original_text": text,
        "final_text": response["text"]
    }));
    assert_eq!(acknowledgement["id"], "delivery-1");
    assert_eq!(acknowledgement["acknowledged"], true);
    let bridge_output = bridge.stop();
    assert!(bridge_output.stderr.is_empty());

    let stats_path = home.path().join(".outrim/gain.json");
    let persisted = fs::read_to_string(&stats_path).unwrap();
    let stats: Value = serde_json::from_str(&persisted).unwrap();
    assert_eq!(stats["rules"]["log_compaction"]["before_tokens"], 739);
    let after = stats["rules"]["log_compaction"]["after_tokens"]
        .as_u64()
        .unwrap();
    assert!(after > 0 && after < 739);
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
    assert!(report.contains("Processed outputs: 1"), "{report}");
    assert!(report.contains("Compression rate:"), "{report}");
    assert!(report.contains("Log compaction"), "{report}");
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
    assert!(report.contains("Processed outputs: 2"), "{report}");
    assert!(report.contains("No matching rule: 2"), "{report}");
}

#[test]
fn clear_removes_statistics_but_preserves_recovery_and_settings() {
    let home = tempfile::tempdir().unwrap();
    outrim::gain::clear(home.path()).unwrap();
    assert!(!home.path().join(".outrim").exists());
    outrim::gain::set_recording(home.path(), true).unwrap();
    let directory = home.path().join(".outrim");
    fs::create_dir_all(directory.join("records/instance")).unwrap();
    fs::write(directory.join("records/instance/events.jsonl"), "event").unwrap();
    fs::write(directory.join("records/instance/original.txt"), "recorded").unwrap();
    fs::create_dir(directory.join("outputs")).unwrap();
    let original = directory.join("outputs/original.txt");
    fs::write(&original, "recoverable").unwrap();
    fs::write(directory.join("gain.json"), "old statistics").unwrap();
    let settings = fs::read(directory.join("recording.json")).unwrap();

    outrim::gain::clear(home.path()).unwrap();
    assert!(!directory.join("gain.json").exists());
    assert!(!directory.join("records").exists());
    assert_eq!(fs::read_to_string(original).unwrap(), "recoverable");
    assert_eq!(
        fs::read(directory.join("recording.json")).unwrap(),
        settings
    );
    outrim::gain::clear(home.path()).unwrap();

    let mut bridge = Bridge::start(home.path());
    assert_eq!(
        bridge.exchange(&delivery_request(
            "new",
            "compressed",
            Some("log_compaction"),
            100,
            50
        ))["acknowledged"],
        true
    );
    let text = "new output";
    let acknowledgement = bridge.exchange(&json!({
        "type": "record_observation", "version": 2, "id": "new-observation",
        "host": "pi", "session": "after-clear", "tool": tool_request("unused", text)["tool"],
        "original_text": text, "final_text": text, "outcome": "no_match", "candidates": []
    }));
    assert_eq!(acknowledgement["acknowledged"], true);
    bridge.stop();
    let stats: Value =
        serde_json::from_slice(&fs::read(directory.join("gain.json")).unwrap()).unwrap();
    assert_eq!(stats["outcomes"]["compressed"], 1);
    assert_eq!(stats["rules"]["log_compaction"]["before_tokens"], 100);
    assert_eq!(fs::read_dir(directory.join("records")).unwrap().count(), 1);
}

#[test]
fn clear_reports_deletion_failure() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".outrim");
    fs::create_dir(&directory).unwrap();
    // A file where the records directory belongs cannot be cleared as a directory.
    fs::write(directory.join("records"), "unchanged").unwrap();
    assert!(outrim::gain::clear(home.path()).is_err());
    assert_eq!(
        fs::read_to_string(directory.join("records")).unwrap(),
        "unchanged"
    );
}

#[test]
fn clear_rejects_piped_confirmation_without_deleting_data() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".outrim");
    fs::create_dir_all(directory.join("records")).unwrap();
    fs::write(directory.join("gain.json"), "unchanged").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .args(["gain", "clear"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The process may reject the pipe before the write reaches it.
    let _ = child.stdin.take().unwrap().write_all(b"yes\n");
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(
        fs::read_to_string(directory.join("gain.json")).unwrap(),
        "unchanged"
    );
    assert!(directory.join("records").is_dir());
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
    assert_eq!(output.stdout, b"No gain data.\n");
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
        (
            "compressed-3",
            "compressed",
            Some("log_compaction"),
            100,
            50,
        ),
        ("protected", "protected", None, 50, 50),
        ("recovery", "recovery_skipped", None, 70, 70),
        ("not-shorter", "not_shorter", None, 80, 80),
        ("no-match", "no_match", None, 90, 90),
        ("failed-open", "failed_open", None, 110, 110),
    ];

    for (id, disposition, applied_rule, before_tokens, after_tokens) in deliveries {
        let acknowledgement = bridge.exchange(&delivery_request(
            id,
            disposition,
            applied_rule,
            before_tokens,
            after_tokens,
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
        "Processed outputs: 8",
        "Compression savings: 170 tokens",
        "Compression rate: 24%",
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
    assert!(report.find("JSON compact").unwrap() < report.find("Log compaction").unwrap());
    let json_rule = report
        .lines()
        .find(|line| line.contains("JSON compact"))
        .unwrap();
    let log_rule = report
        .lines()
        .find(|line| line.contains("Log compaction"))
        .unwrap();
    assert!(json_rule.contains("55%"), "{json_rule}");
    assert!(json_rule.ends_with("██████████"), "{json_rule}");
    assert!(log_rule.contains("50%"), "{log_rule}");
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
    assert!(report.contains("Processed outputs: 2"), "{report}");
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
    assert!(report.contains("Processed outputs: 200"), "{report}");
    assert!(report.contains("No matching rule: 200"), "{report}");
}

#[test]
fn stats_persistence_failure_does_not_block_processed_output() {
    let home = tempfile::tempdir().unwrap();
    let text = log();
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
    let stopped = bridge.stop();
    let error = String::from_utf8(stopped.stderr).unwrap();
    assert!(
        error
            .trim_start()
            .starts_with("[outrim] Failed to save gain:"),
        "{error}"
    );

    let gain = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("gain")
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(!gain.status.success());
    assert!(!gain.stderr.is_empty());
}
