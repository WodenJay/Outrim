use outrim::protocol::{Disposition, Host, Request, Response, Source, Tool, VERSION};
use serde_json::json;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
};

fn start(home: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_outrim"))
        .arg("bridge")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn request(id: &str) -> Request {
    Request {
        context: None,
        version: VERSION,
        id: id.into(),
        host: Host::Pi,
        session: "session/../../escape".into(),
        tool: Tool {
            name: "bash".into(),
            input: json!({"command": "cargo test"}),
            source: Source::TestLog,
            exit_code: Some(0),
        },
        text: format!(
            "running 100 tests\n{}test result: ok. 100 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
            (0..100)
                .map(|n| format!(
                    "test {}_case_{n} ... ok\n",
                    id.replace(char::is_whitespace, "_")
                ))
                .collect::<String>()
        ),
    }
}

#[test]
fn concurrent_sessions_and_pipelined_requests_do_not_overwrite_or_cross_ids() {
    let home = tempfile::tempdir().unwrap();
    let mut workers = Vec::new();
    for worker in 0..4 {
        let home = home.path().to_path_buf();
        workers.push(std::thread::spawn(move || {
            let mut child = start(&home);
            let mut input = child.stdin.take().unwrap();
            let mut requests = std::collections::HashMap::new();
            for index in 0..12 {
                let id = format!("worker-{worker}-request-{index}");
                // Deliberately share host/session across independent processes.
                let item = request(&id);
                writeln!(input, "{}", serde_json::to_string(&item).unwrap()).unwrap();
                requests.insert(id, item.text);
            }
            drop(input);
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            let mut paths = Vec::new();
            for line in String::from_utf8(output.stdout).unwrap().lines() {
                let response: Response = serde_json::from_str(line).unwrap();
                assert_eq!(response.disposition, Disposition::Compressed);
                let original = requests.remove(&response.id).unwrap();
                let path = response.original_path.unwrap();
                assert_eq!(fs::read_to_string(&path).unwrap(), original);
                paths.push(path);
            }
            assert!(requests.is_empty());
            paths
        }));
    }
    let paths: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(paths.len(), 48);
    assert_eq!(
        paths.iter().collect::<std::collections::HashSet<_>>().len(),
        48
    );
}

#[test]
fn real_process_fails_open_for_storage_and_processing_errors() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".outrim"), "not a directory").unwrap();
    let mut child = start(home.path());
    let mut input = child.stdin.take().unwrap();
    let storage_failure = request("storage-failure");
    let mut invalid_version = request("unsupported-version");
    invalid_version.version = VERSION + 1;
    for item in [&storage_failure, &invalid_version] {
        writeln!(input, "{}", serde_json::to_string(item).unwrap()).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses: Vec<Response> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 2);
    for (response, original) in responses.iter().zip([storage_failure, invalid_version]) {
        assert_eq!(response.id, original.id);
        assert_eq!(response.disposition, Disposition::FailedOpen);
        assert_eq!(response.text, original.text);
        assert!(response.original_path.is_none());
        assert!(response.applied_rule.is_none());
    }
}

#[test]
fn malformed_request_gets_correlated_error_and_next_request_still_works() {
    let home = tempfile::tempdir().unwrap();
    let mut child = start(home.path());
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "not json").unwrap();
    input.write_all(b"\xff\n").unwrap();
    writeln!(input, "{{\"version\":1,\"id\":\"bad-request\"}}").unwrap();
    writeln!(
        input,
        "{}",
        serde_json::to_string(&request("good")).unwrap()
    )
    .unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["error"], "invalid_json");
    assert_eq!(lines[0]["id"], serde_json::Value::Null);
    assert_eq!(lines[1]["error"], "invalid_json");
    assert_eq!(lines[1]["id"], serde_json::Value::Null);
    assert_eq!(lines[2]["error"], "invalid_request");
    assert_eq!(lines[2]["id"], "bad-request");
    assert_eq!(lines[3]["id"], "good");
    assert_eq!(lines[3]["disposition"], "compressed");
    assert!(!output.stderr.is_empty());
}

#[test]
fn real_process_compresses_and_original_survives_restart_and_fork() {
    let home = tempfile::tempdir().unwrap();
    let input = request("escaped\nrequest-id");
    let mut child = start(home.path());
    writeln!(
        child.stdin.as_mut().unwrap(),
        "{}",
        serde_json::to_string(&input).unwrap()
    )
    .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let response: Response = serde_json::from_str(&line).unwrap();
    assert_eq!(response.id, input.id);
    assert_eq!(response.version, VERSION);
    assert_eq!(response.disposition, Disposition::Compressed);
    let old_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&old_path).unwrap(), input.text);
    drop(child.stdin.take());
    assert!(child.wait_with_output().unwrap().status.success());

    let mut forked = request("forked");
    forked.host = Host::Opencode;
    forked.session = "new-session".into();
    forked.text = forked.text.replace("case_", "fork_case_");
    let mut child = start(home.path());
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        serde_json::to_string(&forked).unwrap()
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: Response = serde_json::from_slice(&output.stdout).unwrap();
    let new_path = response.original_path.unwrap();
    assert_ne!(old_path, new_path);
    assert!(std::path::Path::new(&old_path).starts_with(home.path().join(".outrim/outputs/pi")));
    assert!(
        std::path::Path::new(&new_path).starts_with(home.path().join(".outrim/outputs/opencode"))
    );
    assert_eq!(fs::read_to_string(old_path).unwrap(), input.text);
    assert_eq!(fs::read_to_string(new_path).unwrap(), forked.text);
}
