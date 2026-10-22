use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::OnceLock,
};

fn rewrite(command: &str, dialect: &str) -> Value {
    let home = tempfile::tempdir().unwrap();
    rewrite_at(command, dialect, home.path(), false)
}

fn rewrite_at(command: &str, dialect: &str, home: &Path, recording: bool) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .args([
            "bridge",
            "--recording",
            if recording { "on" } else { "off" },
        ])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        json!({
            "type": "rewrite", "version": 2, "id": "rewrite-1", "host": "pi",
            "session": "session-1", "tool_call_id": "call-1", "unit_id": "unit-1",
            "recording": true, "dialect": dialect, "command": command,
            "tool": {"name": "provider.shell", "input": {"command": command, "marker": true}, "source": "unknown"}
        })
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn planning_preserves_partial_units_without_execution_or_content() {
    let home = tempfile::tempdir().unwrap();
    let response = rewrite_at("cargo test && echo secret-value", "bash", home.path(), true);
    assert!(response["command"].is_string());
    let observed = events(home.path());
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0]["diagnostics"]["wrapping"], "planned");
    assert_eq!(observed[1]["diagnostics"]["wrapping"], "rejected");
    assert_ne!(
        observed[0]["diagnostics"]["unit_id"],
        observed[1]["diagnostics"]["unit_id"]
    );
    for event in observed {
        assert!(event["execution"].is_null());
        assert!(
            event["content"]
                .as_object()
                .unwrap()
                .values()
                .all(Value::is_null)
        );
        assert!(!event.to_string().contains("secret-value"));
    }
    let report = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .args(["gain", "--detail"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&report.stdout).contains("Executed commands: 0"));
}

#[test]
fn custom_program_policy_uses_runtime_cwd_original_identity_and_refreshes() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("project");
        fs::create_dir_all(project.join(".outrim")).unwrap();
        let custom = home
            .path()
            .join(format!("custom{}", std::env::consts::EXE_SUFFIX));
        fs::copy(fixture(), &custom).unwrap();
        let target = custom.to_str().unwrap().replace('\\', "/");
        let invoke = format!(
            "{}'{target}' json",
            if dialect == "bash" { "" } else { "& " }
        );
        let command = format!("cd project; {invoke}");
        let rewritten = rewrite(&command, dialect);
        let rewritten = rewritten["command"].as_str().unwrap();
        // A wrapper may resolve an unknown external program, but cannot capture it by default.
        let raw = shell(rewritten, dialect, home.path());
        assert_eq!(
            raw.status.code(),
            Some(if dialect == "bash" { 19 } else { 1 })
        );
        assert!(!home.path().join(".outrim/gain.json").exists());
        assert_eq!(fs::read(project.join("executed")).unwrap(), b"once");
        let path = project.join(".outrim/config.json");
        fs::write(
            &path,
            json!({"rules": [{"match": {
            "tool": "provider.shell", "input": {"marker": true},
            "command": ["custom", "json"], "exact": true
        }, "compression": "json_compact"}]})
            .to_string(),
        )
        .unwrap();
        let compressed = shell(rewritten, dialect, home.path());
        assert_eq!(compressed.status.code(), raw.status.code());
        for (actual, expected) in [
            (&compressed.stdout, &raw.stdout),
            (&compressed.stderr, &raw.stderr),
        ] {
            let actual = std::str::from_utf8(actual).unwrap();
            let original = actual
                .split("The full output is available at ")
                .nth(1)
                .unwrap()
                .trim();
            assert_eq!(fs::read(original).unwrap(), *expected);
            assert!(actual.len() < expected.len());
        }
        // Explicit exec selects the same command policy; only host-specific conditions differ.
        fs::write(&path, json!({"rules": [{"match": {"command": ["custom", "json"]}, "compression": "json_compact"}]}).to_string()).unwrap();
        let explicit = Command::new(env!("CARGO_BIN_EXE_outrim"))
            .args(["exec", "--"])
            .arg(&custom)
            .arg("json")
            .current_dir(&project)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .output()
            .unwrap();
        assert_eq!(explicit.status.code(), Some(19));
        assert!(
            std::str::from_utf8(&explicit.stdout)
                .unwrap()
                .contains("The full output is available at ")
        );
        fs::write(&path, r#"{"rules":[{"match":{},"compression":"off"}]}"#).unwrap();
        let unchanged = shell(rewritten, dialect, home.path());
        assert_eq!(unchanged.stdout, raw.stdout);
        assert_eq!(unchanged.stderr, raw.stderr);
        assert_eq!(
            fs::read(project.join("executed")).unwrap(),
            b"onceonceonceonce"
        );
    }
}

#[test]
fn bridge_rewrites_simple_commands_with_execution_context() {
    for dialect in ["bash", "powershell"] {
        let result = rewrite("cargo test 'a b'", dialect);
        let command = result["command"].as_str().expect("rewritten command");
        assert!(command.contains("exec --context"), "{command}");
        assert!(command.contains("exec --context 'eyJ"));
        assert!(command.contains("'a b'"));
    }
}

#[test]
#[cfg(windows)]
fn oversized_bash_context_is_left_native() {
    let command = format!("cargo test '{}'", "x".repeat(4000));
    let response = rewrite(&command, "bash");
    assert!(response["command"].is_null());
    assert_eq!(response["reason"], "rewritten_command_limit");
}

#[test]
fn compound_commands_preserve_short_circuiting_and_distinct_stream_records() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let program = program(dialect);
        let command = format!(
            "{program} test 'a; b && c' && {program} build; {program} build || {program} test 'fallback'"
        );
        let direct = shell(&command, dialect, home.path());
        assert_eq!(fs::read(home.path().join("executed")).unwrap(), b"once");
        fs::remove_file(home.path().join("executed")).unwrap();
        let rewritten = rewrite(&command, dialect);
        let output = shell(
            rewritten["command"].as_str().expect("compound rewrite"),
            dialect,
            home.path(),
        );
        assert_eq!(
            output.status.code(),
            direct.status.code(),
            "{dialect}: {output:?}"
        );
        assert_eq!(fs::read(home.path().join("executed")).unwrap(), b"once");
        assert!(String::from_utf8_lossy(&output.stdout).contains("fallback"));
        let gain = outrim::gain::render(home.path()).unwrap();
        assert!(gain.contains("Processed outputs: 6"), "{gain}");
        let events = events(home.path());
        assert_eq!(events.len(), 6);
        let units: std::collections::HashSet<_> = events
            .iter()
            .map(|e| e["execution"]["unit_id"].as_str().unwrap())
            .collect();
        assert_eq!(units.len(), 3);
        let mut ordered = events.clone();
        ordered.sort_by_key(|event| event["execution"]["unit_id"].as_str().unwrap().to_owned());
        for (stream, expected) in [("stdout", &direct.stdout), ("stderr", &direct.stderr)] {
            let recovered: Vec<u8> = ordered
                .iter()
                .filter(|event| event["execution"]["stream"] == stream)
                .flat_map(|event| {
                    let directory = home
                        .path()
                        .join(".outrim/records")
                        .join(event["instance_id"].as_str().unwrap());
                    fs::read(directory.join(event["content"]["original"].as_str().unwrap()))
                        .unwrap()
                })
                .collect();
            assert_eq!(&recovered, expected, "{dialect}: {stream}");
        }
        for event in events {
            let unit = event["execution"]["unit_id"].as_str().unwrap();
            assert_eq!(
                event["source"],
                if unit.ends_with(":3") {
                    "build_log"
                } else {
                    "test_log"
                }
            );
            assert_eq!(event["tool_call_id"], "call-1");
            assert_eq!(event["session"], "session-1");
            if unit.ends_with(":3") {
                assert!(event["applied_rule"].is_null());
                assert!(event["original_path"].is_null());
                assert_eq!(event["outcome"], "no_match");
            }
        }
    }
}

#[test]
fn conditional_continuations_keep_their_branch() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let p = program(dialect);
        let command = format!("{p} test &&\n {p} build; {p} build ||\n {p} test");
        let original = shell(&command, dialect, home.path());
        fs::remove_file(home.path().join("executed")).unwrap();
        let rewritten = rewrite(&command, dialect);
        let output = shell(rewritten["command"].as_str().unwrap(), dialect, home.path());
        assert_eq!(output.status.code(), original.status.code());
        assert_eq!(
            fs::read(home.path().join("executed")).unwrap(),
            b"once",
            "{dialect}: {output:?}"
        );
        assert_eq!(events(home.path()).len(), 6);
    }
}

#[test]
fn machine_consumers_keep_original_data_while_other_units_are_wrapped() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let p = program(dialect);
        let command = format!(
            "{p} build > raw; {p} build | {p} test 'pipe' > piped; {p} test \"$({p} build)\" > substituted; {p} build"
        );
        let original = shell(&command, dialect, home.path());
        let files =
            ["raw", "piped", "substituted"].map(|name| fs::read(home.path().join(name)).unwrap());
        fs::remove_file(home.path().join("executed")).unwrap();
        let rewritten = rewrite(&command, dialect);
        let output = shell(
            rewritten["command"]
                .as_str()
                .expect("last unit remains eligible"),
            dialect,
            home.path(),
        );
        assert_eq!(
            output.status.code(),
            original.status.code(),
            "{dialect}: {output:?}"
        );
        for (name, expected) in ["raw", "piped", "substituted"].into_iter().zip(files) {
            assert_eq!(
                fs::read(home.path().join(name)).unwrap(),
                expected,
                "{dialect}: {name}"
            );
        }
        assert_eq!(
            fs::read(home.path().join("executed")).unwrap(),
            b"onceonceonceonce"
        );
        assert_eq!(events(home.path()).len(), 2);
    }
}

#[test]
fn successful_branches_and_explicit_bypass_remain_native() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let p = program(dialect);
        let wrapper = env!("CARGO_BIN_EXE_outrim").replace('\\', "/");
        let call = if dialect == "bash" { "" } else { "& " };
        let command = format!(
            "{p} check || {p} build && {p} test; {call}'{wrapper}' exec --no-compress -- {p} build; {p} check"
        );
        // The nested call operator is not an argv token for explicit exec.
        let command = command.replace("-- & ", "-- ");
        let direct = shell(&command, dialect, home.path());
        assert_eq!(fs::read(home.path().join("executed")).unwrap(), b"once");
        fs::remove_file(home.path().join("executed")).unwrap();
        let rewritten = rewrite(&command, dialect);
        let output = shell(rewritten["command"].as_str().unwrap(), dialect, home.path());
        assert_eq!(output.status.code(), direct.status.code());
        assert_eq!(output.stdout, direct.stdout);
        assert_eq!(output.stderr, direct.stderr);
        assert_eq!(fs::read(home.path().join("executed")).unwrap(), b"once");
        assert_eq!(events(home.path()).len(), 6); // three automatic units, not the bypass
    }
}

#[test]
fn unfamiliar_grammar_declines_the_entire_call() {
    for (dialect, command) in [
        ("bash", "cargo build; if true; then cargo test; fi"),
        ("bash", "cargo build; cat <<EOF\n; cargo test\nEOF"),
        ("bash", "cargo build; echo $'a\\'; cargo test; b'"),
        ("bash", "cargo build; (cargo test)"),
        ("bash", "cargo build & cargo test"),
        ("powershell", "cargo build; @'\n'; cargo test; '\n'@"),
        ("powershell", "cargo build; Write-Output “a; cargo test; b”"),
        ("powershell", "$data = cargo build && cargo test"),
        ("powershell", "cargo build; cargo test --% x; cargo build"),
    ] {
        assert!(
            rewrite(command, dialect)["command"].is_null(),
            "{dialect}: {command}"
        );
    }
}

#[test]
#[cfg(windows)]
fn windows_powershell_sequences_and_cmdlet_aliases_keep_native_behavior() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("input.txt"), "native cmdlet data").unwrap();
    let command = format!(
        "cat 'input.txt'; Get-Content 'input.txt'; {} check",
        program("powershell")
    );
    let rewritten = rewrite(&command, "powershell");
    for dialect in ["powershell", "powershell-legacy"] {
        let direct = shell(&command, dialect, home.path());
        let output = shell(rewritten["command"].as_str().unwrap(), dialect, home.path());
        assert_eq!(output.stdout, direct.stdout, "{dialect}: {output:?}");
        assert_eq!(output.stderr, direct.stderr);
        assert_eq!(output.status.code(), direct.status.code());
    }
    let events = events(home.path());
    assert_eq!(
        events
            .iter()
            .filter(|event| !event["execution"].is_null())
            .count(),
        4
    ); // external check, twice
    assert_eq!(
        events
            .iter()
            .filter(|event| event["diagnostics"]["execution_state"] == "runtime_fallback")
            .count(),
        2
    ); // cat resolves to a cmdlet, twice
}

fn events(home: &Path) -> Vec<Value> {
    fs::read_dir(home.join(".outrim/records"))
        .unwrap()
        .flat_map(|entry| {
            fs::read_to_string(entry.unwrap().path().join("events.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<Value>>()
        })
        .collect()
}

#[test]
fn uncertain_or_ineligible_commands_are_not_rewritten() {
    for dialect in ["bash", "powershell"] {
        for command in [
            "outrim exec -- cargo test",
            "outrim exec --no-compress -- cargo test",
            "cargo test | sort",
            "cargo test > out",
            "cargo test $(git diff)",
            "cargo test $value",
            "cargo test *.rs",
            "cargo test 'unclosed",
            "cargo test --watch",
            "cargo test --watch=true",
            "cargo test -wpkg",
            "pytest --pdb",
            "pytest --pdb=custom:Debugger",
            "pytest --trace",
            "vitest --ui",
            "tail -F build.log",
            "tail --follow=name build.log",
            "tail -fn100 build.log",
            "git add --patch",
            "git commit",
            "echo hello",
            "python",
            "Get-Content file",
            "cargo test --% hi",
            "cargo test @args",
            "cargo test ‘hi’",
        ] {
            assert!(
                rewrite(command, dialect)["command"].is_null(),
                "{dialect}: {command}"
            );
        }
    }
    assert!(rewrite("cargo test", "unknown")["command"].is_null());
    assert!(rewrite(r#"cargo test "a""b""#, "powershell")["command"].is_null());
}

fn fixture() -> &'static PathBuf {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap().keep();
        let path = directory.join(format!("cargo{}", std::env::consts::EXE_SUFFIX));
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

fn shell(command: &str, dialect: &str, home: &Path) -> Output {
    let mut shell = if dialect == "bash" {
        // Windows searches System32 before PATH; do not accidentally run WSL bash.
        let bash = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|directory| directory.join(format!("bash{}", std::env::consts::EXE_SUFFIX)))
            .find(|path| path.is_file())
            .unwrap();
        let mut shell = Command::new(bash);
        shell.args(["--noprofile", "--norc", "-c"]);
        shell
    } else {
        let mut shell = Command::new(if dialect == "powershell-legacy" {
            "powershell"
        } else {
            "pwsh"
        });
        shell.args(["-NoProfile", "-NonInteractive", "-Command"]);
        shell
    };
    let path = std::env::join_paths(
        std::iter::once(fixture().parent().unwrap().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    shell
        .env("PATH", path)
        .arg(command)
        .current_dir(home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("EXEC_VALUE", "environment kept")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn program(dialect: &str) -> String {
    let path = fixture().to_str().unwrap().replace('\\', "/");
    if dialect == "bash" {
        format!("'{path}'")
    } else {
        format!("& '{path}'")
    }
}

#[test]
fn rewritten_commands_preserve_arguments_environment_directory_and_status() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let escapes = if dialect == "bash" {
            r#"escaped\ space "back\slash" 'a"b' "a'b""#
        } else {
            r#"escaped` space 'back\slash' 'a"b' "a'b""#
        };
        let command = format!(
            "{} test 'a b' '' \"double quoted\" '工具' {escapes}",
            program(dialect)
        );
        let direct = shell(&command, dialect, home.path());
        let rewritten = rewrite(&command, dialect);
        let wrapped = shell(rewritten["command"].as_str().unwrap(), dialect, home.path());
        assert_eq!(wrapped.stdout, direct.stdout, "{dialect}: {wrapped:?}");
        assert_eq!(wrapped.stderr, direct.stderr);
        assert_eq!(wrapped.status.code(), direct.status.code());
        let gain = outrim::gain::render(home.path()).unwrap();
        assert!(gain.contains("Processed outputs: 2"), "{gain}");
    }
}

#[test]
fn plain_command_names_resolve_native_executables_without_an_exe_suffix() {
    let home = tempfile::tempdir().unwrap();
    let rewritten = rewrite("cargo test", "bash");
    let output = shell(rewritten["command"].as_str().unwrap(), "bash", home.path());
    assert_eq!(output.status.code(), Some(37), "{output:?}");
    assert!(
        home.path().join(".outrim/gain.json").exists(),
        "must run wrapper, not fallback"
    );
}

#[test]
fn rewritten_custom_build_streams_stay_unchanged_with_one_sample_each() {
    for dialect in if cfg!(windows) {
        vec!["bash", "powershell"]
    } else {
        vec!["bash"]
    } {
        let home = tempfile::tempdir().unwrap();
        let command = format!("{} build", program(dialect));
        let original = shell(&command, dialect, home.path());
        fs::remove_file(home.path().join("executed")).unwrap();
        let rewritten = rewrite(&command, dialect);
        let output = shell(rewritten["command"].as_str().unwrap(), dialect, home.path());
        assert_eq!(output.status.code(), original.status.code());
        assert_eq!(fs::read(home.path().join("executed")).unwrap(), b"once");
        assert_eq!(output.stdout, original.stdout);
        assert_eq!(output.stderr, original.stderr);
        let gain = outrim::gain::render(home.path()).unwrap();
        assert!(gain.contains("Processed outputs: 2"), "{gain}");
        let records = home.path().join(".outrim/records");
        let events: Vec<Value> = fs::read_dir(records)
            .unwrap()
            .flat_map(|entry| {
                fs::read_to_string(entry.unwrap().path().join("events.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<Value>>()
            })
            .collect();
        assert_eq!(events.len(), 2);
        for event in events {
            assert_eq!(event["host"], "pi");
            assert_eq!(event["session"], "session-1");
            assert_eq!(event["tool_call_id"], "call-1");
            assert_eq!(event["execution"]["unit_id"], "unit-1");
        }
    }
}

#[test]
fn fallback_observer_failure_does_not_trigger_errexit() {
    let home = tempfile::tempdir().unwrap();
    let response = rewrite("cargo test", "bash");
    let command = response["command"]
        .as_str()
        .unwrap()
        .replace("observe-fallback", "invalid-observer-command");
    let output = shell(
        &format!("set -e; cargo() {{ printf 'still-native'; }}; {command}"),
        "bash",
        home.path(),
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"still-native");
    assert!(output.stderr.is_empty());
}

#[test]
fn actual_shell_aliases_and_functions_remain_native() {
    let home = tempfile::tempdir().unwrap();
    for (dialect, setup) in [
        ("bash", "cargo() { printf 'native function'; }; "),
        (
            "bash",
            "shopt -s expand_aliases; alias cargo='printf native-alias';\n",
        ),
        #[cfg(windows)]
        ("powershell", "function cargo { 'native function' }; "),
    ] {
        let rewritten = rewrite("cargo test; cargo build", dialect);
        let output = shell(
            &format!("{setup}{}", rewritten["command"].as_str().unwrap()),
            dialect,
            home.path(),
        );
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("native"),
            "{output:?}"
        );
        assert!(!home.path().join(".outrim/gain.json").exists());
        let fallbacks: Vec<_> = events(home.path())
            .into_iter()
            .filter(|event| event["diagnostics"]["execution_state"] == "runtime_fallback")
            .collect();
        assert!(fallbacks.len() >= 2);
        assert!(fallbacks.iter().all(|event| {
            event["content"]
                .as_object()
                .unwrap()
                .values()
                .all(Value::is_null)
                && event["before_chars"].is_null()
        }));
    }
}
