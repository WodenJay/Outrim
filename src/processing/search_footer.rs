// Only verified host suffixes are split; all other text stays with the body parser.
pub(super) fn split<'a>(text: &'a str, counts: &str, line_truncation: bool) -> (&'a str, &'a str) {
    let records: Vec<_> = text.split_inclusive('\n').collect();
    let Some(index) = records.iter().rposition(|raw| !blank(raw)) else {
        return (text, "");
    };
    if index < 2 || !blank(records[index - 1]) || blank(records[index - 2]) {
        return (text, "");
    }
    let raw = records[index];
    let line = raw.strip_suffix('\n').unwrap_or(raw);
    let line = if raw.ends_with('\n') {
        line.strip_suffix('\r').unwrap_or(line)
    } else {
        line
    };
    let Some(notice) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return (text, "");
    };
    if !valid_notice(notice, counts, line_truncation) {
        return (text, "");
    }
    let body_len = records[..index - 1].iter().map(|raw| raw.len()).sum();
    text.split_at(body_len)
}

fn blank(raw: &str) -> bool {
    matches!(raw, "\n" | "\r\n")
}

fn valid_notice(notice: &str, counts: &str, line_truncation: bool) -> bool {
    let mut rest = notice;
    if let Some((count, suffix)) = rest.split_once(counts)
        && positive_count(count).is_some()
    {
        rest = suffix;
        if let Some(suggestion) = rest.strip_prefix(". Use limit=") {
            let Some((more, suffix)) = suggestion.split_once(" for more, or refine pattern") else {
                return false;
            };
            if !matches!(
                positive_count(count).and_then(|n| n.checked_mul(2)).zip(positive_count(more)),
                Some((expected, actual)) if expected == actual
            ) {
                return false;
            }
            rest = suffix;
        } else if counts.starts_with(" matches") {
            return false;
        }
        if rest.is_empty() {
            return true;
        }
        let Some(suffix) = rest.strip_prefix(". ") else {
            return false;
        };
        rest = suffix;
    }
    if let Some(suffix) = rest.strip_prefix("50.0KB limit reached") {
        if suffix.is_empty() {
            return true;
        }
        let Some(suffix) = suffix.strip_prefix(". ") else {
            return false;
        };
        rest = suffix;
    }
    line_truncation && rest == "Some lines truncated to 500 chars. Use read tool to see full lines"
}

fn positive_count(text: &str) -> Option<u64> {
    if text.starts_with('0') || text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}
