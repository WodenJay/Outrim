use crate::protocol::{Source, Tool};
use std::collections::HashSet;

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

pub(crate) fn rewrite(tool: &Tool, text: &str) -> Option<(&'static str, String)> {
    match tool.source {
        Source::BuildLog | Source::TestLog | Source::RuntimeLog => {
            let lines: Vec<_> = text.split_inclusive('\n').collect();
            Some(("log_tail", lines[lines.len().saturating_sub(10)..].concat()))
        }
        Source::Search => search_paths(text).map(|text| ("search_paths", text)),
        _ => None,
    }
}

struct SearchHit<'a> {
    path: &'a str,
    line: &'a str,
    content: &'a str,
    terminated: bool,
}

fn search_paths(text: &str) -> Option<String> {
    let hits: Vec<_> = text
        .split_inclusive('\n')
        .map(parse_search_hit)
        .collect::<Option<_>>()?;
    if hits.len() < 2 {
        return None;
    }

    let mut seen = HashSet::new();
    if !hits.iter().any(|hit| !seen.insert(hit.path)) {
        return None;
    }

    let mut compact = String::with_capacity(text.len());
    let mut previous_path = None;
    for hit in hits {
        if previous_path != Some(hit.path) {
            compact.push_str(hit.path);
            compact.push_str(":\n");
            previous_path = Some(hit.path);
        }
        compact.push_str("  ");
        compact.push_str(hit.line);
        compact.push(':');
        compact.push_str(hit.content);
        if hit.terminated {
            compact.push('\n');
        }
    }
    Some(compact)
}

fn parse_search_hit(record: &str) -> Option<SearchHit<'_>> {
    let (record, terminated) = match record.strip_suffix('\n') {
        Some(record) => (record, true),
        None => (record, false),
    };

    for (separator, character) in record.char_indices() {
        if character != ':' {
            continue;
        }
        let number_start = separator + 1;
        let number_end = record[number_start..]
            .find(|character: char| !character.is_ascii_digit())
            .map_or(record.len(), |offset| number_start + offset);
        if number_end == number_start || record.as_bytes().get(number_end) != Some(&b':') {
            continue;
        }

        let path = &record[..separator];
        let line = &record[number_start..number_end];
        let content = &record[number_end + 1..];
        if !valid_search_path(path)
            || line.parse::<u64>().ok().is_none_or(|line| line == 0)
            || contains_location_separator(content)
        {
            return None;
        }
        return Some(SearchHit {
            path,
            line,
            content,
            terminated,
        });
    }
    None
}

fn valid_search_path(path: &str) -> bool {
    if path.is_empty() || path.chars().any(char::is_control) {
        return false;
    }
    match path.find(':') {
        None => true,
        Some(1) if path.as_bytes()[0].is_ascii_alphabetic() => !path[2..].contains(':'),
        Some(_) => false,
    }
}

fn contains_location_separator(text: &str) -> bool {
    text.char_indices().any(|(separator, character)| {
        if character != ':' {
            return false;
        }
        let number_start = separator + 1;
        let number_end = text[number_start..]
            .find(|character: char| !character.is_ascii_digit())
            .map_or(text.len(), |offset| number_start + offset);
        number_end > number_start && text.as_bytes().get(number_end) == Some(&b':')
    })
}
