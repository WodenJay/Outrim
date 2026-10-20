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
