use std::{fs, process::Command};

#[test]
fn invalid_input_never_runs_an_operation() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["unknown"],
        vec!["--unknown"],
        vec!["install"],
        vec!["install", "--agent"],
        vec!["install", "--agent", "--help"],
        vec!["uninstall", "--agent", "other"],
        vec!["upgrade", "extra"],
        vec!["update", "--unknown", "--help"],
        vec!["gain", "unknown"],
        vec!["gain", "--session", "id"],
        vec!["gain", "--detail", "--session"],
        vec!["gain", "--detail", "--session", "--help"],
        vec!["gain", "--detail", "--session", "--unknown"],
        vec!["gain", "record"],
        vec!["gain", "record", "invalid"],
        vec!["gain", "record", "on", "extra"],
        vec!["gain", "clear", "extra"],
        vec!["gain", "clear", "--yes"],
        vec!["gain", "--unknown", "--help"],
        vec!["bridge", "--recording"],
        vec!["bridge", "--recording", "invalid"],
        vec!["bridge", "--unknown", "--help"],
        vec!["--version", "extra"],
        vec!["gain", "--session", "id", "--detail"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
            .args(&args)
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PI_CODING_AGENT_DIR", home.path().join("pi"))
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0, "{args:?}");
    }
}

#[test]
fn inspection_succeeds_without_a_usable_home() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("not-a-directory");
    fs::write(&home, "unchanged").unwrap();
    let mut cases = vec![vec![], vec!["--help"], vec!["-h"], vec!["--version"]];
    for level in [
        vec!["install"],
        vec!["uninstall"],
        vec!["upgrade"],
        vec!["update"],
        vec!["gain"],
        vec!["gain", "record"],
        vec!["gain", "clear"],
        vec!["bridge"],
    ] {
        for help in ["-h", "--help"] {
            let mut args = level.clone();
            args.push(help);
            cases.push(args);
        }
    }
    for args in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
            .args(&args)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(!output.stdout.is_empty());
        if args == ["--version"] {
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!("outrim {}\n", env!("CARGO_PKG_VERSION"))
            );
        }
        assert!(output.stderr.is_empty());
    }
    assert_eq!(fs::read_to_string(home).unwrap(), "unchanged");
}
