use crate::protocol::{Source, Tool};

const RETAINED_LINES: usize = 10;

fn line_chunks(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn last_lines(lines: &[&str]) -> String {
    lines[lines.len().saturating_sub(RETAINED_LINES)..].concat()
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

pub(crate) fn generic_line_fallback(tool: &Tool, text: &str) -> Option<String> {
    if tool.name == "read" {
        return None;
    }
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
