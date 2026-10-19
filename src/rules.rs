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

pub(crate) fn compress_candidate(tool: &Tool, text: &str) -> Option<(&'static str, String)> {
    match tool.source {
        Source::BuildLog | Source::TestLog | Source::RuntimeLog => {
            let lines: Vec<_> = text.split_inclusive('\n').collect();
            Some(("log_tail", lines[lines.len().saturating_sub(10)..].concat()))
        }
        Source::Search => group_search_results(text).map(|text| ("search_paths", text)),
        _ => None,
    }
}

struct SearchHit<'a> {
    path: &'a str,
    line_number: &'a str,
    content: &'a str,
    terminated: bool,
}

fn group_search_results(text: &str) -> Option<String> {
    let hits: Vec<_> = text
        .split_inclusive('\n')
        .map(parse_search_hit)
        .collect::<Option<_>>()?;
    if !hits.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return None;
    }

    let mut grouped = String::with_capacity(text.len());
    let mut previous_path = None;
    for hit in hits {
        if previous_path != Some(hit.path) {
            grouped.push_str(hit.path);
            grouped.push_str(":\n");
            previous_path = Some(hit.path);
        }
        grouped.push_str("  ");
        grouped.push_str(hit.line_number);
        grouped.push(':');
        grouped.push_str(hit.content);
        if hit.terminated {
            grouped.push('\n');
        }
    }
    Some(grouped)
}

fn parse_search_hit(record: &str) -> Option<SearchHit<'_>> {
    let (record, terminated) = match record.strip_suffix('\n') {
        Some(record) => (record.strip_suffix('\r').unwrap_or(record), true),
        None => (record, false),
    };

    for (separator, character) in record.char_indices() {
        if character != ':' {
            continue;
        }
        let number_start = separator + 1;
        let Some(number_end) = digit_run_end(record, number_start) else {
            continue;
        };
        let has_content = record.as_bytes().get(number_end) == Some(&b':');
        if number_end != record.len() && !has_content {
            continue;
        }

        let path = &record[..separator];
        let line_number = &record[number_start..number_end];
        let content = if has_content {
            &record[number_end + 1..]
        } else {
            ""
        };
        if !valid_search_path(path)
            || line_number
                .parse::<u64>()
                .ok()
                .is_none_or(|line_number| line_number == 0)
            || (has_content && contains_location_separator(&record[number_end..]))
        {
            return None;
        }
        return Some(SearchHit {
            path,
            line_number,
            content,
            terminated,
        });
    }
    None
}

fn digit_run_end(text: &str, start: usize) -> Option<usize> {
    let end = text[start..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(text.len(), |offset| start + offset);
    (end > start).then_some(end)
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
        character == ':'
            && digit_run_end(text, separator + 1)
                .is_some_and(|number_end| text.as_bytes().get(number_end) == Some(&b':'))
    })
}
