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
    let first = bridge.read(&path, &body);
    bridge.adopt(&first);
    fs::create_dir_all(home.path().join(".outrim")).unwrap();
    fs::write(home.path().join(".outrim/outputs"), "not a directory").unwrap();
    let repeated = bridge.read(&path, &body);
    assert_eq!(repeated["text"], body);
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
                "[outrim]: Already read {}:10–300; unchanged, so omitted. Search first (grep/rg); read fully if needed. Reread: read \"{recovery}\".",
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
