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
    next: usize,
}
impl Bridge {
    fn new(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_outrim"))
            .arg("bridge")
            .env("HOME", home)
            .env("USERPROFILE", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            next: 0,
        }
    }
    fn send(&mut self, mut value: Value) -> Value {
        self.next += 1;
        value["id"] = json!(self.next.to_string());
        value["version"] = json!(2);
        writeln!(self.input, "{value}").unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn read(&mut self, path: &std::path::Path, text: &str) -> Value {
        self.send(json!({"type":"read_process", "host":"pi", "session":"one", "tool":{"name":"read","input":{"path":path},"source":"source_code"}, "text":text, "selection":{"path":path,"key":"lines:10:300","label":"10–300"}}))
    }
    fn range(&mut self, path: &std::path::Path, start: usize, text: &str) -> Value {
        self.send(json!({"type":"read_process", "host":"pi", "session":"one", "tool":{"name":"read","input":{"path":path},"source":"source_code"}, "text":text, "selection":{"path":path,"key":format!("lines:{start}"),"label":"range", "body":{"start":start,"lines":text.split_inclusive('\n').map(|line| json!({"text":line,"rendered":line})).collect::<Vec<_>>()}}}))
    }
    fn shell(&mut self, home: &std::path::Path, command: &str) -> Value {
        let call = format!("shell-{}", self.next);
        let rewrite = self.send(json!({"type":"rewrite","host":"pi","session":"one","tool_call_id":call,"unit_id":call,"recording":false,"dialect":"bash","command":command}));
        let shell = if cfg!(windows) {
            let git = Command::new("git").arg("--exec-path").output().unwrap();
            std::path::PathBuf::from(String::from_utf8(git.stdout).unwrap().trim())
                .join("../../../usr/bin/bash.exe")
        } else {
            "bash".into()
        };
        let output = Command::new(shell)
            .args(["-c", rewrite["command"].as_str().unwrap()])
            .env("HOME", home)
            .env("USERPROFILE", home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        self.send(json!({"type":"shell_read","host":"pi","session":"one","tool_call_id":call,"tool":{"name":"bash","input":{"command":command},"source":"unknown","exit_code":0},"text":String::from_utf8(output.stdout).unwrap()}))
    }
    fn adopt(&mut self, response: &Value) {
        self.send(json!({"type":"read_delivered", "request_id":response["id"]}));
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn overlapping_delivered_ranges_accumulate_without_reference_chains() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let lines: Vec<_> = (1..=400)
        .map(|n| format!("line {n}: {}\n", "verified source text ".repeat(8)))
        .collect();
    let mut bridge = Bridge::new(home.path());
    let first = bridge.range(&path, 1, &lines[..200].concat());
    assert_eq!(first["text"], lines[..200].concat());
    assert_eq!(
        bridge.range(&path, 150, &lines[149..350].concat())["text"],
        lines[149..350].concat()
    );
    bridge.adopt(&first);
    let second = bridge.range(&path, 150, &lines[149..350].concat());
    let recovery = second["original_path"].as_str().unwrap();
    assert_eq!(
        second["text"],
        format!(
            "[outrim]: Already read {}:150–200; verified unchanged, so omitted.\n{}[outrim]: Full output: read \"{recovery}\".",
            path.display(),
            lines[200..350].concat()
        )
    );
    assert_eq!(
        fs::read_to_string(recovery).unwrap(),
        lines[149..350].concat()
    );
    bridge.adopt(&second);
    let third = bridge.range(&path, 180, &lines[179..].concat());
    assert!(
        third["text"]
            .as_str()
            .unwrap()
            .contains(":180–350; verified unchanged")
    );
    assert!(
        third["text"]
            .as_str()
            .unwrap()
            .contains(&lines[350..].concat())
    );
    assert_eq!(
        fs::read_to_string(third["original_path"].as_str().unwrap()).unwrap(),
        lines[179..].concat()
    );
}

#[test]
fn conflicting_overlap_clears_only_that_file_and_rejects_pending_evidence() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    let other = home.path().join("other.rs");
    fs::write(&path, "source").unwrap();
    fs::write(&other, "other").unwrap();
    let a = "original source content ".repeat(30) + "\n";
    let b = "modified source content ".repeat(30) + "\n";
    let mut bridge = Bridge::new(home.path());
    let first = bridge.range(&path, 1, &a.repeat(20));
    bridge.adopt(&first);
    let unrelated = bridge.range(&other, 1, &a.repeat(20));
    bridge.adopt(&unrelated);
    let pending = bridge.range(&path, 15, &a.repeat(20));
    let changed = bridge.range(&path, 10, &b.repeat(5));
    assert_eq!(changed["text"], b.repeat(5));
    bridge.adopt(&pending);
    assert_eq!(bridge.range(&path, 25, &a.repeat(10))["text"], a.repeat(10));
    assert_eq!(
        bridge.range(&other, 1, &a.repeat(20))["applied_rule"],
        "read_dedup"
    );
    bridge.adopt(&changed);
    assert_eq!(
        bridge.range(&path, 10, &b.repeat(5))["applied_rule"],
        "read_dedup"
    );
    // An unrelated edit does not require a whole-file snapshot match.
    fs::write(&path, "different contents outside the observed range").unwrap();
    assert_eq!(
        bridge.range(&path, 10, &b.repeat(5))["applied_rule"],
        "read_dedup"
    );
}

#[test]
fn disjoint_evidence_preserves_gaps_metadata_and_exact_recovery() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let a = "α source text ".repeat(40) + "\r\n";
    let gap = "new evidence\r\n";
    let mut bridge = Bridge::new(home.path());
    for start in [1, 3] {
        let result = bridge.range(&path, start, &a);
        bridge.adopt(&result);
    }
    let body = format!("{a}{gap}{a}no final newline");
    let result = bridge.range(&path, 1, &body);
    let recovery = result["original_path"].as_str().unwrap();
    assert_eq!(
        result["text"],
        format!(
            "[outrim]: Already read {}:1–1; verified unchanged, so omitted.\n{gap}[outrim]: Already read {}:3–3; verified unchanged, so omitted.\nno final newline\n[outrim]: Full output: read \"{recovery}\".",
            path.display(),
            path.display()
        )
    );
    fs::write(&path, "subsequent edit").unwrap();
    assert_eq!(fs::read_to_string(recovery).unwrap(), body);
    bridge.adopt(&result);
    let recovered = bridge.read(std::path::Path::new(recovery), &body);
    assert_eq!(recovered["text"], body);
    assert_eq!(recovered["disposition"], "recovery_skipped");
}

#[test]
fn range_shapes_reuse_only_same_positions_in_the_same_consumer() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let line = "same text at many positions ".repeat(30) + "\n";
    for (start, count, overlap) in [
        (10, 10, Some("10–19")),
        (12, 5, Some("12–16")),
        (5, 20, Some("10–19")),
        (5, 10, Some("10–14")),
        (15, 10, Some("15–19")),
        (1, 9, None),
        (20, 5, None),
        (40, 5, None),
    ] {
        let mut bridge = Bridge::new(home.path());
        let first = bridge.range(&path, 10, &line.repeat(10));
        bridge.adopt(&first);
        let current = bridge.range(&path, start, &line.repeat(count));
        if let Some(overlap) = overlap {
            assert!(
                current["text"]
                    .as_str()
                    .unwrap()
                    .contains(&format!(":{overlap}; verified unchanged"))
            );
        } else {
            assert_eq!(current["text"], line.repeat(count));
        }
        for (host, session) in [("opencode", "one"), ("pi", "two")] {
            let current = bridge.send(json!({"type":"read_process","host":host,"session":session,"tool":{"name":"read","input":{"path":path},"source":"source_code"},"text":line.repeat(10),"selection":{"path":path,"key":"range","label":"range","body":{"start":10,"lines":vec![json!({"text":line,"rendered":line});10]}}}));
            assert_eq!(current["text"], line.repeat(10));
        }
    }
    let mut bridge = Bridge::new(home.path());
    let first = bridge.range(&path, 1, &line.repeat(10));
    bridge.adopt(&first);
    let alias = path.parent().unwrap().join(".").join("source.rs");
    assert_eq!(
        bridge.range(&alias, 1, &line.repeat(10))["applied_rule"],
        "read_dedup"
    );
}

#[test]
fn concurrent_conflicting_deliveries_do_not_mix_file_baselines() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let mut bridge = Bridge::new(home.path());
    let a = "old source content ".repeat(30) + "\n";
    let b = "new source content ".repeat(30) + "\n";
    let first = bridge.range(&path, 1, &a.repeat(20));
    let second = bridge.range(&path, 10, &b.repeat(5));
    bridge.adopt(&first);
    bridge.adopt(&second);
    assert_eq!(bridge.range(&path, 1, &a.repeat(5))["text"], a.repeat(5));
}

#[test]
fn exact_native_repeats_omit_ambiguous_final_lines_but_keep_host_metadata() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let mut bridge = Bridge::new(home.path());
    for short in [true, false] {
        let end = if short { 11 } else { 10 };
        let prefix = format!("Read file source.rs, lines 10-{end}\n");
        let final_line = format!("{end}: {}", "x".repeat(10000));
        let notice = format!(
            "\n[Output truncated. Continue reading with offset: {}]",
            end + 1
        );
        let rendered = if short { "10: short\n" } else { "" };
        let text = format!("{prefix}{rendered}{final_line}{notice}");
        let lines = if short {
            vec![json!({"text": "short\n", "rendered": rendered})]
        } else {
            vec![]
        };
        let request = json!({
            "type": "read_process", "host": "opencode", "session": format!("exact-{short}"),
            "tool": {"name": "read", "input": {"path": path}, "source": "source_code"},
            "text": text,
            "selection": {
                "path": path, "key": "native", "label": format!("10–{end}"),
                "body": {"start": 10, "prefix": prefix, "lines": lines,
                    "suffix": format!("{final_line}{notice}"),
                    "final_line": {"text": "x".repeat(10000), "rendered": final_line}}
            }
        });
        let first = bridge.send(request.clone());
        bridge.adopt(&first);
        let second = bridge.send(request);
        let recovery = second["original_path"].as_str().unwrap();
        assert_eq!(
            second["text"],
            format!(
                "{prefix}[outrim]: Already read {}:10–{end}; verified unchanged, so omitted.\n{notice}\n[outrim]: Full output: read \"{recovery}\".",
                path.display()
            )
        );
        assert_eq!(fs::read_to_string(recovery).unwrap(), text);
    }
}

#[test]
fn changed_native_final_line_returns_full_output_and_clears_file_evidence() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let line = "long source line ".repeat(200) + "\n";
    let request = |last: &str, extra: bool| {
        let mut lines = vec![json!({"text": line, "rendered": line})];
        let final_line = if extra {
            lines.push(json!({"text": format!("{last}\n"), "rendered": format!("{last}\n")}));
            "extra"
        } else {
            last
        };
        json!({
            "type": "read_process", "host": "pi", "session": "one",
            "tool": {"name": "read", "input": {"path": path}, "source": "source_code"},
            "text": if extra { format!("{line}{last}\nextra") } else { format!("{line}{last}") },
            "selection": {"path": path, "key": "native", "label": "native",
                "body": {"start": 1, "lines": lines, "suffix": final_line,
                    "final_line": {"text": final_line, "rendered": final_line}}}
        })
    };
    for initially_partial in [false, true] {
        for (old_extra, new_extra) in [(false, false), (false, true), (true, false)] {
            let mut bridge = Bridge::new(home.path());
            if initially_partial {
                let seed = bridge.range(&path, 1, &line);
                bridge.adopt(&seed);
            }
            let pending = bridge.range(&path, 20, &line);
            let first = bridge.send(request("old", old_extra));
            bridge.adopt(&first);
            let mut next = request("new", new_extra);
            if initially_partial {
                next["selection"]["key"] = json!("different-limit");
            }
            let changed = bridge.send(next.clone());
            assert_eq!(changed["text"], next["text"]);
            bridge.adopt(&changed);
            bridge.adopt(&pending);
            assert_eq!(bridge.range(&path, 20, &line)["text"], line);
            assert_eq!(bridge.range(&path, 1, &line)["applied_rule"], "read_dedup");
        }
    }
}

#[test]
fn ranges_require_verified_rendering_and_final_savings() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let mut bridge = Bridge::new(home.path());
    let first = bridge.range(&path, 1, "tiny\n");
    bridge.adopt(&first);
    let tiny = bridge.range(&path, 1, "tiny\nnew\n");
    assert_eq!(tiny["text"], "tiny\nnew\n");
    assert!(tiny["original_path"].is_null());
    let long_line = "x".repeat(2000) + "\n";
    let result = bridge.range(&path, 5, &long_line);
    bridge.adopt(&result);
    let omitted = bridge.range(&path, 5, &long_line);
    let exact_cost = omitted["text"].as_str().unwrap().chars().count();
    // Recovery filenames have fixed length: a same-width range at line 6 has
    // exactly the same final cost, including the complete recovery hint.
    let equal = "y".repeat(exact_cost - 1) + "\n";
    let result = bridge.range(&path, 6, &equal);
    bridge.adopt(&result);
    let result = bridge.range(&path, 6, &equal);
    assert_eq!(result["text"], equal);
    assert_eq!(result["disposition"], "not_shorter");
    assert!(result["original_path"].is_null());
    let text = "long source content\n".repeat(200);
    let malformed = bridge.send(json!({"type":"read_process","host":"pi","session":"one","tool":{"name":"read","input":{"path":path},"source":"source_code"},"text":text,"selection":{"path":path,"key":"malformed","label":"range","body":{"start":10,"lines":[{"text":text,"rendered":"not actual output"}]}}}));
    bridge.adopt(&malformed);
    assert_eq!(bridge.range(&path, 10, &text)["text"], text);
}

#[test]
fn explicit_policy_snapshots_do_not_acquire_builtin_read_dedup() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let mut bridge = Bridge::new(home.path());
    let body = "fn unchanged_source() {}\n".repeat(200);
    let first = bridge.read(&path, &body);
    bridge.adopt(&first);
    for policy in [json!("off"), json!({"methods":["json_compact"]})] {
        let result = bridge.send(json!({"type":"read_process", "host":"pi", "session":"one", "tool":{"name":"read","input":{"path":path},"source":"source_code"}, "text":body, "selection":{"path":path,"key":"lines:10:300","label":"10–300"}, "policy":policy}));
        assert_eq!(result["text"], body);
        assert!(result["applied_rule"].is_null());
        bridge.adopt(&result);
    }
    assert_eq!(bridge.read(&path, &body)["applied_rule"], "read_dedup");
}

#[test]
fn omission_storage_failure_returns_the_complete_body() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.rs");
    fs::write(&path, "source").unwrap();
    let mut bridge = Bridge::new(home.path());
    let body = "完整源码保持可恢复\n".repeat(200);
    let first = bridge.range(&path, 1, &body);
    bridge.adopt(&first);
    fs::create_dir_all(home.path().join(".outrim")).unwrap();
    fs::write(home.path().join(".outrim/outputs"), "not a directory").unwrap();
    let repeated = bridge.range(&path, 50, &"完整源码保持可恢复\n".repeat(151));
    assert_eq!(repeated["text"], "完整源码保持可恢复\n".repeat(151));
    assert_eq!(repeated["disposition"], "failed_open");
    assert!(repeated["original_path"].is_null());
    assert!(repeated["applied_rule"].is_null());
}

#[test]
fn a_compressed_changed_read_invalidates_the_old_baseline() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.txt");
    fs::write(&path, "source").unwrap();
    let mut bridge = Bridge::new(home.path());
    let a = "original text\n".repeat(300);
    let b = serde_json::to_string_pretty(&vec![json!({"value":123}); 100]).unwrap();
    for (index, body) in [&a, &b, &a].into_iter().enumerate() {
        let result = bridge.send(json!({"type":"read_process","host":"pi","session":"one","tool":{"name":"read","input":{"path":path},"source":"unknown"},"text":body,"selection":{"path":path,"key":"whole","label":"whole file"}}));
        if index == 1 {
            assert_eq!(result["disposition"], "compressed");
            assert_ne!(result["applied_rule"], "read_dedup");
        } else {
            assert_eq!(result["text"], *body);
        }
        bridge.adopt(&result);
    }
}

#[test]
fn native_and_shell_ranges_share_positions_but_tail_does_not_guess_absolute_lines() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source.txt");
    let lines: Vec<_> = (1..=100)
        .map(|n| format!("line {n}: {}\n", "source evidence ".repeat(40)))
        .collect();
    fs::write(&path, lines.concat()).unwrap();
    let quoted = format!("'{}'", path.to_string_lossy().replace('\\', "/"));
    let mut bridge = Bridge::new(home.path());
    let native = bridge.range(&path, 10, &lines[9..30].concat());
    bridge.adopt(&native);
    for command in [
        "sed -n '10p'",
        "sed -n '20,40p'",
        "head -n 50",
        "tail -n +40",
        "cat",
    ] {
        let result = bridge.shell(home.path(), &format!("{command} {quoted}"));
        assert_eq!(result["applied_rule"], "read_dedup", "{command}");
        bridge.adopt(&result);
    }
    let native = bridge.range(&path, 75, &lines[74..90].concat());
    assert_eq!(native["applied_rule"], "read_dedup");
    // Last-N tail only has end-relative evidence, even after an absolute cat.
    let tail = bridge.shell(home.path(), &format!("tail -n 20 {quoted}"));
    assert_eq!(tail["text"], lines[80..].concat());
    bridge.adopt(&tail);
    let tail = bridge.shell(home.path(), &format!("tail -n 30 {quoted}"));
    assert!(
        tail["text"]
            .as_str()
            .unwrap()
            .contains(":last 20 lines; verified unchanged")
    );
    assert!(
        tail["text"]
            .as_str()
            .unwrap()
            .starts_with(&lines[70..80].concat())
    );
    assert_eq!(
        fs::read_to_string(tail["original_path"].as_str().unwrap()).unwrap(),
        lines[70..].concat()
    );
}

#[test]
fn shell_reads_execute_before_omission() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("reading sample.txt");
    let a_line = format!("{}\n", "original source content ".repeat(20));
    let b_line = format!("{}\n", "modified source content ".repeat(20));
    let a = a_line.repeat(100);
    let b = b_line.repeat(100);
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_outrim"))
        .canonicalize()
        .unwrap();
    for (command, lines) in [
        ("cat", 100),
        ("cat --", 100),
        ("head", 10),
        ("head -n 20", 20),
        ("head -n20", 20),
        ("head -20", 20),
        ("tail", 10),
        ("tail -n 20", 20),
        ("tail -n20", 20),
        ("tail -20", 20),
        ("tail -n +90", 11),
        ("tail -n+90", 11),
        ("sed -n '10p'", 1),
        ("sed -n '10,20p'", 11),
        ("sed -n '90,$p'", 11),
    ] {
        let mut bridge = Bridge::new(home.path());
        for (index, body) in [&a, &a, &a, &b, &a].into_iter().enumerate() {
            fs::write(&path, body).unwrap();
            let call = format!("call-{index}");
            let original = format!("{command} '{}'", path.to_string_lossy().replace('\\', "/"));
            let rewrite = bridge.send(json!({"type":"rewrite","host":"pi","session":"one","tool_call_id":call,"unit_id":call,"recording":false,"dialect":"bash","command":original}));
            let wrapped = rewrite["command"].as_str().unwrap();
            let shell = if cfg!(windows) {
                let git = Command::new("git").arg("--exec-path").output().unwrap();
                std::path::PathBuf::from(String::from_utf8(git.stdout).unwrap().trim())
                    .join("../../../usr/bin/bash.exe")
            } else {
                "bash".into()
            };
            let output = Command::new(shell)
                .args(["-c", wrapped])
                .env("HOME", home.path())
                .env("USERPROFILE", home.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}: {} ({})",
                command,
                String::from_utf8_lossy(&output.stderr),
                exe.display()
            );
            assert!(output.stderr.is_empty());
            let text = String::from_utf8(output.stdout).unwrap();
            let result = bridge.send(json!({"type":"shell_read","host":"pi","session":"one","tool_call_id":call,"tool":{"name":"bash","input":{"command":original},"source":"unknown","exit_code":0},"text":text}));
            if matches!(index, 1 | 2) {
                assert_eq!(result["applied_rule"], "read_dedup", "{command}");
                let recovery = result["original_path"].as_str().unwrap();
                assert_eq!(fs::read_to_string(recovery).unwrap(), a_line.repeat(lines));
            } else {
                let line = if index == 3 { &b_line } else { &a_line };
                assert_eq!(result["text"], line.repeat(lines), "{command}");
            }
            bridge.adopt(&result);
        }
    }
}

#[test]
fn delivered_reads_omit_only_the_current_identical_body_and_reset_on_compact() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("source with spaces.rs");
    fs::write(&path, "source").unwrap();
    let a = "original source line\n".repeat(300);
    let b = "changed source line\n".repeat(300);
    let mut bridge = Bridge::new(home.path());
    let first = bridge.read(&path, &a);
    assert_eq!(first["text"], a);
    let concurrent = bridge.read(&path, &a);
    assert_eq!(concurrent["text"], a);
    bridge.adopt(&first);
    for _ in 0..2 {
        let omitted = bridge.read(&path, &a);
        let recovery = omitted["original_path"].as_str().unwrap();
        assert_eq!(
            omitted["text"],
            format!(
                "[outrim]: Already read {}:10–300; verified unchanged, so omitted.\n[outrim]: Full output: read \"{recovery}\".",
                path.display()
            )
        );
        assert_eq!(fs::read_to_string(recovery).unwrap(), a);
        for _ in 0..2 {
            let recovered = bridge.read(std::path::Path::new(recovery), &a);
            assert_eq!(recovered["text"], a);
            assert_eq!(recovered["disposition"], "recovery_skipped");
        }
        bridge.adopt(&omitted);
    }
    let other = home.path().join("other.rs");
    fs::write(&other, "source").unwrap();
    assert_eq!(bridge.read(&other, &a)["text"], a);
    for (host, session, selection) in [
        ("opencode", "one", "lines:10:300"),
        ("pi", "two", "lines:10:300"),
        ("pi", "one", "lines:20:300"),
    ] {
        let result = bridge.send(json!({"type":"read_process", "host":host,"session":session,"tool":{"name":"read","input":{"path":path},"source":"source_code"},"text":a,"selection":{"path":path,"key":selection,"label":"range"}}));
        assert_eq!(result["text"], a);
    }
    let changed = bridge.read(&path, &b);
    assert_eq!(changed["text"], b);
    bridge.adopt(&changed);
    let reverted = bridge.read(&path, &a);
    assert_eq!(reverted["text"], a);
    bridge.adopt(&reverted);
    let pending = bridge.read(&path, &b);
    bridge.send(json!({"type":"read_reset"}));
    bridge.adopt(&pending);
    assert_eq!(bridge.read(&path, &a)["text"], a);
}
