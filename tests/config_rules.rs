use outrim::{Processor, protocol::*};
use serde_json::json;
use std::{fs, path::Path};

fn config(directory: &Path, rules: serde_json::Value) {
    fs::create_dir_all(directory.join(".outrim")).unwrap();
    fs::write(
        directory.join(".outrim/config.json"),
        json!({"rules": rules}).to_string(),
    )
    .unwrap();
}

fn request(cwd: &Path) -> Request {
    serde_json::from_value(json!({
        "version": VERSION, "id": "rules", "host": "pi", "session": "rules",
        "context": {"cwd": cwd},
        "tool": {"name": "read", "input": {}, "source": "unknown"},
        "text": serde_json::to_string_pretty(&json!({"items": (0..100).collect::<Vec<_>>()})).unwrap()
    })).unwrap()
}

#[test]
fn ordered_command_tool_and_input_conditions_use_literal_argv_and_explicit_globs() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut input = request(project.path());
    input.context.as_mut().unwrap().command =
        Some(vec!["custom".into(), "report".into(), "a b\"c*".into()]);
    input.tool.name = "provider.custom".into();
    input.tool.input =
        json!({"options": {"path": "data/file.json", "all": true}, "numbers": [1,2]});
    for (condition, matches) in [
        (json!({"command": ["custom", "report"]}), true),
        (
            json!({"command": ["custom", "report"], "exact": true}),
            false,
        ),
        (
            json!({"command": ["custom", "report", "a b\"c*"], "exact": true}),
            true,
        ),
        (json!({"command": ["custom", "report", "*"]}), false),
        (
            json!({"command": ["custom", "report", {"glob": "a ?*\\*"}]}),
            true,
        ),
        (json!({"tool": "custom"}), false),
        (
            json!({"tool": {"glob": "provider.*"}, "input": {"options": {"path": {"glob": "data/*.json"}}}}),
            true,
        ),
        (
            json!({"tool": "provider.custom", "input": {"missing": null}}),
            false,
        ),
        (json!({"input": {"numbers": [1]}}), false),
    ] {
        config(
            project.path(),
            json!([{"match": condition, "compression": "off"}]),
        );
        let response = Processor::new(home.path()).process(input.clone());
        assert_eq!(response.text == input.text, matches, "{condition}");
    }
}

#[test]
fn program_matching_resolves_paths_and_preserves_platform_names() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut input = request(project.path());
    input.context.as_mut().unwrap().command = Some(vec!["./bin/custom".into(), "report".into()]);
    for (program, matches) in [
        (json!("custom"), true),
        (json!("CUSTOM.EXE"), cfg!(windows)),
        (json!("bin/custom"), true),
        (json!("other/custom"), false),
        (json!(project.path().join("bin/custom")), true),
        (json!({"glob": "./bin/cust*"}), true),
        (json!({"glob": "bin/../bin/cust*"}), true),
        (json!({"glob": "bin/cust*"}), true),
    ] {
        config(
            project.path(),
            json!([{"match": {"command": [program]}, "compression": "off"}]),
        );
        assert_eq!(
            Processor::new(home.path()).process(input.clone()).text == input.text,
            matches,
            "{program}"
        );
    }
    if cfg!(windows) {
        input.context.as_mut().unwrap().command = Some(vec![r".\BIN\CUSTOM.EXE".into()]);
        config(
            project.path(),
            json!([{"match": {"command": [r"bin\custom"]}, "compression": "off"}]),
        );
        assert_eq!(
            Processor::new(home.path()).process(input.clone()).text,
            input.text
        );
        for pattern in [
            r".\\bin\\cust*.exe".to_owned(),
            format!(
                r"{}\\BIN\\cust*.EXE",
                project.path().display().to_string().replace('\\', r"\\")
            ),
        ] {
            config(
                project.path(),
                json!([{"match": {"command": [{"glob": pattern}]}, "compression": "off"}]),
            );
            assert_eq!(
                Processor::new(home.path()).process(input.clone()).text,
                input.text,
                "{pattern}"
            );
        }
    }
}

#[test]
fn explicit_methods_override_protection_without_substitution() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut input = request(project.path());
    input.tool.source = Source::SourceCode;
    input.tool.input = json!({"path": "outside/config.json"});
    for (methods, expected) in [
        (json!("json_compact"), Some("json_compact")),
        (json!("json_toon"), Some("json_toon")),
        (json!(["json_toon", "json_compact"]), Some("json_toon")),
        (json!("log_compaction"), None),
    ] {
        config(
            project.path(),
            json!([
                {"match": {}, "compression": methods},
                {"match": {}, "compression": "json_compact"}
            ]),
        );
        let response = Processor::new(home.path()).process(input.clone());
        assert_eq!(response.applied_rule.as_deref(), expected, "{methods}");
        if let Some(path) = response.original_path {
            assert_eq!(fs::read_to_string(path).unwrap(), input.text);
            assert!(response.text.chars().count() < input.text.chars().count());
        } else {
            assert_eq!(response.text, input.text);
        }
        assert!(
            response
                .candidates
                .iter()
                .all(|candidate| methods.as_array().map_or_else(
                    || methods.as_str() == Some(candidate.rule.as_str()),
                    |methods| methods.contains(&json!(candidate.rule))
                ))
        );
    }
}

#[test]
fn nearest_project_does_not_merge_ancestors_and_invalid_files_fail_open() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested");
    fs::create_dir_all(&nested).unwrap();
    config(
        home.path(),
        json!([{"match": {}, "compression": "json_compact"}]),
    );
    config(project.path(), json!([{"match": {}, "compression": "off"}]));
    config(
        &nested,
        json!([{"match": {"tool": "other"}, "compression": "off"}]),
    );
    let processor = Processor::new(home.path());
    let input = request(&nested);
    assert_eq!(
        processor.process(input.clone()).applied_rule.as_deref(),
        Some("json_compact")
    );
    for invalid in [
        "not json".to_owned(),
        json!({"rules": [{"match": {}, "compression": null}]}).to_string(),
        json!({"rules": [{"match": {"tool": null}, "compression": "off"}]}).to_string(),
        json!({"rules": [{"match": {"command": null}, "compression": "off"}]}).to_string(),
        json!({"rules": [{"match": {"input": null}, "compression": "off"}]}).to_string(),
        json!({"rules": [{"match": {}, "compression": "off"}, {"match": {}, "compression": "unknown"}]}).to_string(),
        json!({"rules": [{"match": {"command": []}, "compression": "off"}]}).to_string(),
        json!({"rules": [{"match": {}, "compression": []}]}).to_string(),
        json!({"rules": [{"match": {}, "compression": ["off", "json_compact"]}]}).to_string(),
        json!({"rules": [{"match": {"tool": {"glob": "trailing\\"}}}]}).to_string(),
    ] {
        fs::write(nested.join(".outrim/config.json"), invalid).unwrap();
        assert_eq!(processor.process(input.clone()).applied_rule.as_deref(), Some("json_compact"));
    }
}

#[test]
fn explicit_log_selection_recognizes_custom_reporters_and_preserves_diagnostics() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    config(
        project.path(),
        json!([{"match": {}, "compression": "log_compaction"}]),
    );
    let mut input = request(project.path());
    input.tool.source = Source::Unknown;
    input.tool.input = json!({"argv": ["custom", "report"]});
    input.text = format!(
        "running 81 tests\n{}test broken ... FAILED\ntest result: FAILED. 80 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\nopaque diagnostic\n",
        (0..80)
            .map(|i| format!("test check_{i} ... ok\n"))
            .collect::<String>()
    );
    let response = Processor::new(home.path()).process(input.clone());
    assert_eq!(response.applied_rule.as_deref(), Some("log_compaction"));
    assert!(response.text.contains("test broken ... FAILED"));
    assert!(response.text.contains("opaque diagnostic"));
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        input.text
    );
}

#[test]
fn user_config_is_not_a_project_config_through_a_path_alias() {
    let outer = tempfile::tempdir().unwrap();
    let home = outer.path().join("user");
    let cwd = home.join("work");
    fs::create_dir_all(home.join("alias")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    config(&home, json!([{"match": {}, "compression": "off"}]));
    config(
        outer.path(),
        json!([{"match": {}, "compression": "json_compact"}]),
    );
    assert_eq!(
        Processor::new(&home.join("alias/.."))
            .process(request(&cwd))
            .applied_rule
            .as_deref(),
        Some("json_compact")
    );
}

#[test]
fn project_builtin_rule_overrides_global_off_and_edits_apply_next_call() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    config(home.path(), json!([{"match": {}, "compression": "off"}]));
    let processor = Processor::new(home.path());
    let input = request(project.path());
    let response = processor.process(input.clone());
    assert_eq!(response.text, input.text);
    assert!(response.original_path.is_none());

    config(project.path(), json!([{"match": {"tool": "read"}}]));
    let response = processor.process(input.clone());
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        input.text
    );
}
