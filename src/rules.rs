use crate::protocol::{Source, Tool};

const RETAINED_LINES: usize = 10;

fn line_chunks(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn last_lines(lines: &[&str]) -> String {
    lines[lines.len().saturating_sub(RETAINED_LINES)..].concat()
}

pub(crate) fn protected(tool: &Tool) -> bool {
    if matches!(tool.source, Source::SourceCode | Source::Configuration) {
        return true;
    }
    ["path", "file_path", "filePath"].iter().any(|key| {
        let Some(path) = tool.input.get(key).and_then(|value| value.as_str()) else {
            return false;
        };
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
    })
}

pub(crate) fn log_tail(tool: &Tool, text: &str) -> Option<String> {
    if !matches!(
        tool.source,
        Source::BuildLog | Source::TestLog | Source::RuntimeLog
    ) {
        return None;
    }
    let lines = line_chunks(text);
    Some(last_lines(&lines))
}

pub(crate) fn generic_line_fallback(text: &str) -> Option<String> {
    let lines = line_chunks(text);
    if lines.len() < RETAINED_LINES * 2 {
        return None;
    }
    Some(format!(
        "{}{}",
        lines[..RETAINED_LINES].concat(),
        last_lines(&lines[RETAINED_LINES..])
    ))
}
