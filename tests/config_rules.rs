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
