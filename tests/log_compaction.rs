use outrim::{Processor, protocol::*};
use serde_json::json;
use std::fs;

fn request(command: &str, text: &str) -> Request {
    Request {
        version: VERSION,
        id: "logs".into(),
        host: Host::Pi,
        session: "logs".into(),
        tool: Tool {
            name: "bash".into(),
            input: json!({"command": command}),
            source: Source::TestLog,
            exit_code: Some(1),
        },
        text: text.into(),
    }
}

#[test]
fn unsupported_logs_are_not_truncated() {
    let home = tempfile::tempdir().unwrap();
    for command in ["pytest", "cargo run", "bun run build", "cargo test"] {
        let text = format!(
            "important first evidence\n{}last line\n",
            "custom output\n".repeat(100)
        );
        let response = Processor::new(home.path()).process(request(command, &text));
        assert_eq!(response.text, text, "{command}");
        assert_eq!(response.disposition, Disposition::NoMatch);
        assert!(response.original_path.is_none());
    }
}

// Crafted libtest fixture, matching cargo/libtest's native terse line reporter.
fn cargo_tests() -> String {
    let passes = (0..30)
        .map(|n| format!("test module::long_successful_test_name_{n} ... ok\n"))
        .collect::<String>();
    format!(
        "running 32 tests\n{passes}test module::ignored ... ignored\ntest module::failure ... FAILED\n\nfailures:\n\n---- module::failure stdout ----\nthread 'module::failure' panicked at src/lib.rs:42:9:\nassertion failed: expected value\nstack backtrace:\n   0: module::failure\n\nfailures:\n    module::failure\n\ntest result: FAILED. 30 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
    )
}

// Crafted Bun 1.4.3 default-reporter variants, not historical captures.
fn bun_tests() -> String {
    let passes = (0..25)
        .map(|n| format!("(pass) suite > successful test with long name {n} [0.12ms]\n"))
        .collect::<String>();
    format!(
        "bun test v1.4.3 (test-version)\n\ntests/example.test.ts:\n{passes}custom print\n17 | expect(actual).toEqual(expected);\n                       ^\nerror: expect(received).toEqual(expected)\n- expected\n+ received\n at tests/example.test.ts:17:24\n(fail) suite > failure [1.00ms]\n(skip) suite > incomplete\n(todo) suite > future work\n\n 25 pass\n 1 fail\n 1 skip\n 1 todo\n 26 expect() calls\nRan 28 tests across 1 file. [30.00ms]\n"
    )
}

#[test]
fn bun_preserves_pre_failure_diagnostics_and_native_summaries() {
    let home = tempfile::tempdir().unwrap();
    for command in ["bun test", "bun run test", "npm run test"] {
        let text = bun_tests();
        let response = Processor::new(home.path()).process(request(command, &text));
        assert_eq!(response.disposition, Disposition::Compressed, "{command}");
        assert!(response.text.contains("25 successful tests omitted"));
        assert!(
            response
                .text
                .contains(text.split_once("custom print").unwrap().1)
        );
        assert!(!response.text.contains("successful test with long name"));
        assert_eq!(
            fs::read_to_string(response.original_path.unwrap()).unwrap(),
            text
        );
    }
}

#[test]
fn bun_ansi_optional_timings_and_watch_cycles_remain_separate() {
    let home = tempfile::tempdir().unwrap();
    let plain = bun_tests();
    let colored = plain
        .replace("(pass)", "\x1b[32m✓\x1b[0m")
        .replace(" [0.12ms]", "");
    let text = format!("{colored}\nRerunning tests/example.test.ts\n{plain}");
    let response = Processor::new(home.path()).process(request("bun test --watch", &text));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(
        response.text.matches("25 successful tests omitted").count(),
        2
    );
    assert_eq!(
        response.text.matches("Ran 28 tests across 1 file.").count(),
        2
    );
    assert!(response.text.contains("Rerunning tests/example.test.ts"));
    assert!(response.text.contains("(todo) suite > future work"));
}

// Crafted variants of cli/cli watch_test.go and presentation_test.go at
// 7948e8056e45ac3c729b829e0eafe2d0797a95c6; not historical captures.
fn gh_snapshot(symbol: &str, jobs: &str) -> String {
    format!("{symbol} trunk CI · 2\nTriggered via push about 1 minute ago\n\nJOBS\n{jobs}\n")
}

#[test]
fn gh_latest_states_keep_missing_jobs_and_distinguish_historical_failures() {
    let home = tempfile::tempdir().unwrap();
    let successes = (0..20)
        .map(|n| format!("  ✓ successful setup step {n}\n"))
        .collect::<String>();
    let first = gh_snapshot(
        "*",
        &format!(
            "X Linux in 1m0s (ID 101)\n{successes}  X Check Rust\n- Windows (ID 102)\n  - Check Windows\n"
        ),
    );
    let second = gh_snapshot(
        "✓",
        &format!("✓ Linux in 2m0s (ID 101)\n{successes}  ✓ Check Rust\n"),
    );
    let text =
        format!("{first}{second}* trunk CI · 2\nTriggered via push now\n\nJOBS\n* Linux (ID ");
    let response = Processor::new(home.path()).process(request("gh run watch 2", &text));
    assert_eq!(response.disposition, Disposition::Compressed);
    for evidence in [
        "✓ trunk CI · 2",
        "✓ Linux in 2m0s (ID 101)",
        "21 successful steps omitted",
        "Windows (ID 102)",
        "Last observed",
        "Historical evidence for job ID 101",
        "  X Check Rust",
        "* Linux (ID ",
    ] {
        assert!(response.text.contains(evidence), "{evidence}");
    }
    assert!(!response.text.contains("successful setup step"));
}

#[test]
fn gh_supported_ansi_final_snapshot_and_opaque_annotations_survive() {
    let home = tempfile::tempdir().unwrap();
    let steps = "  ✓ setup with long descriptive name\n".repeat(20);
    let first = gh_snapshot("*", &format!("* Linux (ID 101)\n{steps}  X Check Rust\n"));
    let final_snapshot = gh_snapshot(
        "X",
        &format!("X Linux in 1m0s (ID 101)\n{steps}  X Check Rust\n"),
    );
    let annotation = "ANNOTATIONS\nX timed out waiting\nmultiline details\nLinux: src/a.rs#42\n\nrequesting annotations returned 403 Forbidden\n";
    let text = format!("\x1b[?1049h\x1b[0;0H\x1b[JRefreshing run status every 3 seconds. Press Ctrl+C to quit.\n\n{first}{annotation}\x1b[?1049l{final_snapshot}\nX Run CI (2) completed with 'timed_out'\nfailed to get jobs: EOF\n").replace("  X Check Rust", "  \x1b[31mX\x1b[0m Check Rust");
    let response = Processor::new(home.path()).process(request("gh run watch 2 --compact", &text));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert!(response.text.contains(annotation));
    assert!(response.text.contains("completed with 'timed_out'"));
    assert!(response.text.contains("failed to get jobs: EOF"));
    assert!(response.text.contains("X Linux in 1m0s (ID 101)"));
    assert!(!response.text.contains("setup with long descriptive name"));
}

// Historical recovered record r-QeA1Ue-23 (Outrim 0.2.1), original
// o-1cqNUh.txt; gh run watch 39711763723 --repo WodenJay/Outrim --exit-status --interval 30.
#[test]
fn gh_historical_failure_keeps_job_step_annotations_and_watcher_error() {
    let home = tempfile::tempdir().unwrap();
    let text = include_str!("fixtures/logs/gh-watch-historical.txt");
    let response = Processor::new(home.path())
        .process(request("gh run watch 39711763723 --exit-status", text));
    assert_eq!(response.disposition, Disposition::Compressed);
    for evidence in [
        "* v0.3.0 Check and release · 39711763723",
        "aarch64-apple-darwin in 1m21s (ID 119604492848)",
        "  X Check Rust",
        "X Process completed with exit code 101.",
        "aarch64-apple-darwin: .github#254",
        "Due to capacity constraints",
        "failed to get run:",
        ": EOF",
        "Independent Cargo source build",
        "x86_64-pc-windows-msvc",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ] {
        assert!(response.text.contains(evidence), "{evidence}");
    }
    assert_eq!(response.text.matches("Triggered via push").count(), 1);
    assert!(response.text.contains("Latest observed state"));
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        text
    );
}

#[test]
fn cargo_keeps_phase_results_targets_and_complete_diagnostics() {
    let home = tempfile::tempdir().unwrap();
    let progress = (0..20)
        .map(|n| format!("   Compiling dependency-{n} v1.2.3\n"))
        .collect::<String>();
    let warnings = "warning: unused import\n --> src/lib.rs:1:5\n  |\n1 | use unused;\n  |     ^^^^^^\nerror: a compiler diagnostic\n";
    for command in ["cargo build", "cargo check", "cargo clippy"] {
        let text = format!("{progress}{warnings}    Finished `dev` profile in 1.0s\n");
        let response = Processor::new(home.path()).process(request(command, &text));
        assert_eq!(response.disposition, Disposition::Compressed, "{command}");
        assert!(
            response
                .text
                .contains("20 observed Cargo progress entries omitted")
        );
        assert!(response.text.contains(warnings));
        assert!(response.text.contains("Finished `dev`"));
    }
    let first = cargo_tests();
    let second = first.replace("module::", "second::");
    let text = format!(
        "{progress}    Finished `test` profile\n     Running first (target/first)\n{first}\n     Running second (target/second)\n{second}\n   Doc-tests library\nrunning 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"
    );
    let response = Processor::new(home.path()).process(request("cargo test --workspace", &text));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(
        response.text.matches("30 successful tests omitted").count(),
        2
    );
    assert_eq!(response.text.matches("test result:").count(), 3);
    for evidence in [
        "Running first",
        "Running second",
        "Doc-tests library",
        "Finished `test`",
        "module::failure",
        "second::failure",
    ] {
        assert!(response.text.contains(evidence), "{evidence}");
    }
}

#[test]
fn uncertain_test_boundaries_and_reporter_shaped_diagnostics_are_opaque() {
    let home = tempfile::tempdir().unwrap();
    let embedded = cargo_tests();
    let cases = [
        (
            "cargo test",
            format!(
                "running 1 test\ntest failure ... FAILED\n\nexample output from panic:\n{embedded}"
            ),
        ),
        (
            "cargo test",
            cargo_tests().replace("running 32 tests", "running 999 tests"),
        ),
        (
            "cargo test",
            cargo_tests().replace("test result:", "interrupted result:"),
        ),
        (
            "cargo test",
            cargo_tests().replace(
                "test result: FAILED. 30 passed; 1 failed",
                &format!("test result: FAILED. {} passed; 1 failed", usize::MAX),
            ),
        ),
        (
            "bun test",
            bun_tests().replace("tests/example.test.ts:", "tests/unknown.reporter:"),
        ),
        ("bun run test", cargo_tests()),
        (
            "gh run watch 2",
            "Run CI (2) has already completed with 'cancelled'\n".to_owned(),
        ),
    ];
    for (command, text) in cases {
        let response = Processor::new(home.path()).process(request(command, &text));
        assert_eq!(
            response.disposition,
            Disposition::NoMatch,
            "{command}: {text}"
        );
        assert_eq!(response.text, text);
    }
    let bun = bun_tests().replace(
        "custom print",
        &format!(
            "custom print\n{}",
            "(pass) part of a multiline custom diagnostic, not a record\n".repeat(20)
        ),
    );
    let response = Processor::new(home.path()).process(request("bun test", &bun));
    assert!(
        response
            .text
            .contains(&"(pass) part of a multiline custom diagnostic, not a record\n".repeat(20))
    );
}

#[test]
fn metadata_controls_and_launcher_identity_gate_log_compaction() {
    let home = tempfile::tempdir().unwrap();
    let text = cargo_tests()
        .replace(" ... ok", " ... \x1b[32mok\x1b[0m")
        .replace('\n', "\r\n");
    for input in [
        json!({"command": "cargo test"}),
        json!({"argv": ["C:\\bin\\cargo.exe", "test"]}),
    ] {
        let mut req = request("", &text);
        req.tool.input = input;
        req.tool.exit_code = None;
        let response = Processor::new(home.path()).process(req);
        assert_eq!(response.disposition, Disposition::Compressed);
        assert!(
            response
                .text
                .contains("test module::failure ... FAILED\r\n")
        );
        assert_eq!(
            fs::read_to_string(response.original_path.unwrap()).unwrap(),
            text
        );
    }
    for source in [Source::SourceCode, Source::Configuration, Source::Unknown] {
        let mut req = request("cargo test", &text);
        req.tool.source = source;
        assert_eq!(Processor::new(home.path()).process(req).text, text);
    }
    for control in ["\x1b[2K", "\x1b]0;title\x07", "\x1b[H", "\r"] {
        let uncertain = format!("{control}{text}");
        assert_eq!(
            Processor::new(home.path())
                .process(request("cargo test", &uncertain))
                .text,
            uncertain
        );
    }
    for command in [
        "echo cargo test",
        "cargo test && echo done",
        "unknown test",
        "bun run test",
    ] {
        assert_eq!(
            Processor::new(home.path())
                .process(request(command, &text))
                .text,
            text
        );
    }
}

#[test]
fn bun_successful_files_and_repeated_names_keep_local_counts() {
    let home = tempfile::tempdir().unwrap();
    let passes = "(pass) suite > repeated name with optional timing omitted\n".repeat(20);
    let text = format!(
        "bun test v1.4.3 (test-version)\n\nfirst.test.ts:\n{passes}\nsecond.test.ts:\n{passes}\n 40 pass\n 0 fail\nRan 40 tests across 2 files. [1.00ms]\n"
    );
    for command in [
        "bun test",
        "bun run custom-script",
        "pnpm run test",
        "yarn test",
    ] {
        let response = Processor::new(home.path()).process(request(command, &text));
        assert_eq!(response.disposition, Disposition::Compressed, "{command}");
        assert_eq!(
            response.text.matches("20 successful tests omitted").count(),
            2
        );
        for evidence in [
            "first.test.ts:",
            "second.test.ts:",
            "40 pass",
            "0 fail",
            "Ran 40 tests across 2 files. [1.00ms]",
        ] {
            assert!(response.text.contains(evidence), "{evidence}");
        }
    }
}

#[test]
fn cargo_folds_successes_in_a_failed_target_and_recovers_exact_original() {
    let home = tempfile::tempdir().unwrap();
    let text = cargo_tests();
    let response = Processor::new(home.path()).process(request("cargo test", &text));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("log_compaction"));
    assert!(response.text.contains("30 successful tests omitted"));
    assert!(!response.text.contains("long_successful_test_name_0"));
    let diagnostics = text.split_once("test module::ignored").unwrap().1;
    assert!(response.text.contains(diagnostics));
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        text
    );
    assert!(response.text.chars().count() < text.chars().count());
    assert_eq!(
        response.candidates[0].final_chars,
        Some(response.text.chars().count() as u64)
    );
}
