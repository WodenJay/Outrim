use std::{fs, process::Command};

#[test]
fn inspection_succeeds_without_a_usable_home() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("not-a-directory");
    fs::write(&home, "unchanged").unwrap();
    let mut cases = vec![vec![], vec!["--help"], vec!["-h"], vec!["--version"]];
    for level in [vec!["install"], vec!["uninstall"], vec!["upgrade"], vec!["update"], vec!["gain"], vec!["gain", "record"], vec!["bridge"]] {
        for help in ["-h", "--help"] {
            let mut args = level.clone();
            args.push(help);
            cases.push(args);
        }
    }
    for args in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_outrim"))
            .args(&args).env("HOME", &home).env("USERPROFILE", &home)
            .output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(!output.stdout.is_empty());
        if args == ["--version"] {
            assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("outrim {}\n", env!("CARGO_PKG_VERSION")));
        }
        assert!(output.stderr.is_empty());
    }
    assert_eq!(fs::read_to_string(home).unwrap(), "unchanged");
}
