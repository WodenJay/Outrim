//! Conservative, single-output reporter compaction. Unknown text is never a tail budget.
mod bun;
mod cargo;
mod gh;

use crate::protocol::{Source, Tool};

pub(super) fn compact(tool: &Tool, text: &str) -> Option<String> {
    if !matches!(
        tool.source,
        Source::BuildLog | Source::TestLog | Source::RuntimeLog
    ) {
        return None;
    }
    let words = command(tool)?;
    let words: Vec<_> = words.iter().map(String::as_str).collect();
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let view = lines
        .iter()
        .map(|line| recognition(line))
        .collect::<Option<Vec<_>>>()?;
    let candidate = match words.as_slice() {
        ["cargo", "test", ..] => cargo::compact(&lines, &view, true),
        ["cargo", "build" | "check" | "clippy", ..] => cargo::compact(&lines, &view, false),
        ["gh", "run", "watch", ..] => gh::compact(&lines, &view),
        ["bun", "test", ..]
        | ["bun" | "npm" | "pnpm" | "yarn", "run", ..]
        | ["npm" | "pnpm" | "yarn", "test", ..] => bun::compact(&lines, &view),
        _ => None,
    }?;
    (candidate != text).then_some(candidate)
}

pub(super) fn selected(text: &str) -> Option<String> {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let view = lines
        .iter()
        .map(|line| recognition(line))
        .collect::<Option<Vec<_>>>()?;
    // Only existing, recognized reporter records are folded; opaque text survives.
    [
        cargo::compact(&lines, &view, true),
        gh::compact(&lines, &view),
        bun::compact(&lines, &view),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| candidate != text)
}

// Host adapters already reject compound commands. Here accept only plain words,
// or structured argv; this is intentionally not another shell parser.
fn command(tool: &Tool) -> Option<Vec<String>> {
    let mut words: Vec<String> =
        if let Some(argv) = tool.input.get("argv").and_then(|v| v.as_array()) {
            argv.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<_>>()?
        } else {
            let command = tool.input.get("command")?.as_str()?;
            if command.chars().any(|c| {
                matches!(
                    c,
                    '\'' | '"' | '`' | '$' | ';' | '&' | '|' | '<' | '>' | '\n' | '\r'
                )
            }) {
                return None;
            }
            command.split_whitespace().map(str::to_owned).collect()
        };
    let first = words.first_mut()?;
    *first = crate::command::program_name(first);
    Some(words)
}

// SGR only for ordinary reporters. gh's known screen controls are handled by gh
// as snapshot delimiters, never as instructions to mutate previously seen text.
fn recognition(line: &str) -> Option<String> {
    let mut result = String::new();
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next()? != '[' {
                return None;
            }
            let mut sequence = String::new();
            loop {
                let c = chars.next()?;
                sequence.push(c);
                if c.is_ascii_alphabetic() || c == '~' {
                    break;
                }
                if !c.is_ascii_digit() && c != ';' && c != '?' {
                    return None;
                }
            }
            if !(sequence.ends_with('m')
                && sequence[..sequence.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ';'))
                && !matches!(
                    sequence.as_str(),
                    "H" | "0;0H" | "2J" | "0J" | "J" | "?25l" | "?25h" | "?1049h" | "?1049l"
                )
            {
                return None;
            }
            // Keep screen controls visible to gh; reject them in other reporters.
            if !sequence.ends_with('m') {
                result.push('\x1b');
                result.push('[');
                result.push_str(&sequence);
            }
        } else if c.is_control() && c != '\t' {
            return None;
        } else {
            result.push(c);
        }
    }
    Some(result)
}

fn fold(lines: &[&str], label: &str) -> String {
    let summary = format!("[outrim]: {} {label} omitted\n", lines.len());
    let original = lines.concat();
    if summary.chars().count() < original.chars().count() {
        summary
    } else {
        original
    }
}
