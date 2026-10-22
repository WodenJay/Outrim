use crate::protocol::{Source, Tool};

pub(crate) fn protected(tool: &Tool) -> bool {
    if matches!(tool.source, Source::SourceCode | Source::Configuration) {
        return true;
    }
    if let Some(argv) = tool.input.get("argv").and_then(serde_json::Value::as_array) {
        let program = argv
            .first()
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let program = crate::command::program_name(program);
        if matches!(program.as_str(), "cat" | "head" | "tail" | "type")
            && argv[1..]
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(protected_path)
        {
            return true;
        }
    }
    ["path", "file_path", "filePath"].iter().any(|key| {
        let Some(path) = tool.input.get(key).and_then(|value| value.as_str()) else {
            return false;
        };
        protected_path(path)
    })
}

pub(crate) fn recovery_reference(input: &serde_json::Value) -> bool {
    match input {
        serde_json::Value::String(text) => text.replace('\\', "/").contains(".outrim/outputs/"),
        serde_json::Value::Array(values) => values.iter().any(recovery_reference),
        serde_json::Value::Object(values) => values.values().any(recovery_reference),
        _ => false,
    }
}

fn protected_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path);
    let extension = name.rsplit('.').next().unwrap_or("");
    matches!(
        name,
        "dockerfile" | "makefile" | "justfile" | ".env" | ".gitignore"
    ) || matches!(
        extension,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "sh"
            | "ps1"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "ini"
            | "cfg"
            | "xml"
            | "lock"
    )
}
