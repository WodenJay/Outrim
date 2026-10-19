use crate::protocol::{Source, Tool};

pub(crate) fn protected(tool: &Tool) -> bool {
    if matches!(tool.source, Source::SourceCode | Source::Configuration) {
        return true;
    }
    // A search path is a scope, not the source text returned by the tool.
    if matches!(tool.source, Source::Search) {
        return false;
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

#[derive(Clone, Copy)]
pub(crate) enum AppliedRule {
    LogTail,
    SearchHitGroups,
}

impl AppliedRule {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::LogTail => "log_tail",
            Self::SearchHitGroups => "search_hit_groups",
        }
    }
}

pub(crate) fn compress_candidate(tool: &Tool, text: &str) -> Option<(AppliedRule, String)> {
    match tool.source {
        Source::BuildLog | Source::TestLog | Source::RuntimeLog => {
            Some((AppliedRule::LogTail, log_tail(text)))
        }
        Source::Search => {
            crate::search::group_search_hits(text).map(|text| (AppliedRule::SearchHitGroups, text))
        }
        _ => None,
    }
}

fn log_tail(text: &str) -> String {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    lines[lines.len().saturating_sub(10)..].concat()
}
