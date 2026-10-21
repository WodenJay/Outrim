//! Locate only flat shell lists. Opaque consumer units stay byte-for-byte intact.
use std::ops::Range;

pub(super) struct Segment {
    pub range: Range<usize>,
    pub host_output: bool,
}

pub(super) fn segments(command: &str, powershell: bool) -> Option<Vec<Segment>> {
    // PowerShell treats typographic quotes as syntax too; decline rather than
    // accidentally segmenting the contents of an unfamiliar quoted string.
    if command.contains(['‘', '’', '“', '”']) {
        return None;
    }
    let bytes = command.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut quote = None;
    let mut host_output = true;
    let mut pipe_input = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == 0 || c == b'\r' {
            return None;
        }
        if quote == Some(b'\'') {
            if c == b'\'' {
                if powershell && bytes.get(i + 1) == Some(&b'\'') {
                    i += 1;
                } else {
                    quote = None;
                }
            }
        } else if c == b'\\' && !powershell || c == b'`' && powershell {
            i += 1;
            if matches!(bytes.get(i), None | Some(b'\n' | b'\r' | 0)) {
                return None;
            }
        } else if !powershell && c == b'$' && matches!(bytes.get(i + 1), Some(b'\'' | b'"')) {
            return None; // ANSI-C and locale quotes have additional lexical rules
        } else if c == b'$' && bytes.get(i + 1) == Some(&b'(') {
            i = substitution(bytes, i + 2, powershell, 0)?;
            host_output = false;
        } else if quote == Some(b'"') {
            if c == b'"' {
                quote = None;
            } else if c == b'`' && !powershell {
                return None;
            }
        } else if c == b'\'' || c == b'"' {
            quote = Some(c);
        } else if matches!(
            c,
            b'(' | b')' | b'{' | b'}' | b'[' | b']' | b'#' | b'`' | b'!'
        ) || powershell && (bytes[i..].starts_with(b"--%") || c == b'@')
        {
            return None;
        } else if c == b'<' || c == b'>' {
            // Here-documents, here-strings and process substitution are not flat lists.
            if bytes.get(i + 1) == Some(&b'<') || bytes.get(i + 1) == Some(&b'(') {
                return None;
            }
            host_output = false;
            if bytes.get(i + 1) == Some(&b'&') {
                i += 1;
            }
        } else if matches!(c, b';' | b'\n' | b'&' | b'|') {
            // A newline after a chain/pipe operator continues that operand.
            if c == b'\n' && command[start..i].trim().is_empty() {
                i += 1;
                continue;
            }
            let doubled = bytes.get(i + 1) == Some(&c);
            if c == b'&' && !doubled {
                // PowerShell's literal call operator is part of the command head.
                if powershell && command[start..i].trim().is_empty() {
                    i += 1;
                    continue;
                }
                return None; // background jobs are not host-facing foreground lists
            }
            if c == b';' && doubled || c == b'|' && bytes.get(i + 1) == Some(&b'&') {
                return None;
            }
            let pipe = c == b'|' && !doubled;
            push(
                &mut result,
                command,
                start..i,
                host_output && !pipe && !pipe_input,
                powershell,
            )?;
            i += usize::from(doubled);
            start = i + 1;
            host_output = true;
            pipe_input = pipe;
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    push(
        &mut result,
        command,
        start..i,
        host_output && !pipe_input,
        powershell,
    )?;
    Some(result)
}

fn push(
    result: &mut Vec<Segment>,
    command: &str,
    range: Range<usize>,
    host_output: bool,
    powershell: bool,
) -> Option<()> {
    let text = command[range.clone()].trim();
    let prefix = &command[..range.start];
    if text.is_empty()
        && (prefix.ends_with("&&") || prefix.ends_with("||") || prefix.ends_with('|'))
    {
        return None;
    }
    let head = text.split_whitespace().next().unwrap_or("");
    // PowerShell assignment can capture an entire pipeline chain, not just the
    // first operand. Its later output is not proven to face the host.
    if powershell && head.starts_with('$') {
        return None;
    }
    // These introduce grammar or change how later tokens are parsed, not external commands.
    let reserved = if powershell {
        matches!(
            head.to_ascii_lowercase().as_str(),
            "if" | "else"
                | "elseif"
                | "foreach"
                | "for"
                | "while"
                | "do"
                | "switch"
                | "try"
                | "catch"
                | "finally"
                | "function"
                | "filter"
                | "class"
                | "trap"
                | "param"
                | "using"
                | "data"
                | "exit"
                | "return"
                | "throw"
                | "break"
                | "continue"
        )
    } else {
        matches!(
            head,
            "if" | "then"
                | "else"
                | "elif"
                | "fi"
                | "for"
                | "while"
                | "until"
                | "do"
                | "done"
                | "case"
                | "esac"
                | "select"
                | "function"
                | "time"
                | "coproc"
                | "in"
                | "[["
                | "]]"
        )
    };
    if reserved {
        return None;
    }
    result.push(Segment { range, host_output });
    Some(())
}

// Skip an opaque command substitution, never rewrite its consumer or its contents.
// This deliberately declines richer grammar (including here-documents), even inside it.
fn substitution(bytes: &[u8], mut i: usize, powershell: bool, depth: usize) -> Option<usize> {
    if depth == 32 || bytes.get(i) == Some(&b'(') {
        return None;
    }
    let mut quote = None;
    while let Some(&c) = bytes.get(i) {
        if matches!(c, 0 | b'\r' | b'\n') {
            return None;
        }
        if quote == Some(b'\'') {
            if c == b'\'' {
                if powershell && bytes.get(i + 1) == Some(&b'\'') {
                    i += 1;
                } else {
                    quote = None;
                }
            }
        } else if c == b'\\' && !powershell || c == b'`' && powershell {
            i += 1;
            if matches!(bytes.get(i), None | Some(0 | b'\r' | b'\n')) {
                return None;
            }
        } else if !powershell && c == b'$' && matches!(bytes.get(i + 1), Some(b'\'' | b'"')) {
            return None;
        } else if c == b'$' && bytes.get(i + 1) == Some(&b'(') {
            i = substitution(bytes, i + 2, powershell, depth + 1)?;
        } else if quote == Some(b'"') {
            if c == b'"' {
                quote = None;
            } else if c == b'`' && !powershell {
                return None;
            }
        } else if c == b'\'' || c == b'"' {
            quote = Some(c);
        } else if c == b')' {
            return Some(i);
        } else if matches!(c, b'(' | b'{' | b'}' | b'<' | b'#' | b'`') || powershell && c == b'@' {
            return None;
        }
        i += 1;
    }
    None
}
