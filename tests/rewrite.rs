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
    let mut child = Command::new(env!("CARGO_BIN_EXE_outrim"))
        .args(["bridge", "--recording", "off"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
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
            "recording": true, "dialect": dialect, "command": command
        })
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn bridge_rewrites_simple_commands_with_execution_context() {
    for dialect in ["bash", "powershell"] {
        let result = rewrite("cargo test 'a b'", dialect);
        let command = result["command"].as_str().expect("rewritten command");
        assert!(command.contains("exec --context"), "{command}");
        assert!(command.contains("exec --context '7b"));
        assert!(command.contains("'a b'"));
    }
}

#[test]
fn uncertain_or_ineligible_commands_are_not_rewritten() {
    for dialect in ["bash", "powershell"] {
        for command in [
            "outrim exec -- cargo test",
            "outrim exec --no-compress -- cargo test",
            "cargo test && git diff",
            "cargo test | sort",
            "cargo test > out",
            "cargo test $(git diff)",
            "cargo test\ngit diff",
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
        let mut shell = Command::new("powershell");
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
        let gain: Value =
            serde_json::from_slice(&fs::read(home.path().join(".outrim/gain.json")).unwrap())
                .unwrap();
        assert_eq!(gain["outputs"], 2);
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
fn rewritten_streams_have_recoverable_originals_and_one_sample_each() {
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
        for (compressed, original) in [
            (&output.stdout, &original.stdout),
            (&output.stderr, &original.stderr),
        ] {
            let text = std::str::from_utf8(compressed).unwrap();
            let path = text
                .split("The full output is available at ")
                .nth(1)
                .expect(text)
                .trim();
            assert_eq!(&fs::read(path).unwrap(), original);
        }
        let gain: Value =
            serde_json::from_slice(&fs::read(home.path().join(".outrim/gain.json")).unwrap())
                .unwrap();
        assert_eq!(gain["outputs"], 2);
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
        let rewritten = rewrite("cargo test", dialect);
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
    }
}
