use super::fold;

pub(super) fn compact(lines: &[&str], view: &[String], tests: bool) -> Option<String> {
    if view.iter().any(|line| line.contains('\x1b')) {
        return None;
    }
    let mut result = String::new();
    let mut i = 0;
    let mut progress = true;
    while i < lines.len() {
        if progress && progress_line(&view[i]) {
            let start = i;
            while i < lines.len() && progress_line(&view[i]) {
                i += 1;
            }
            result.push_str(&fold(&lines[start..i], "observed Cargo progress entries"));
        } else if tests && test_count(&view[i]).is_some() {
            if let Some(end) = section_end(view, i) {
                result.push_str(lines[i]);
                i += 1;
                let mut records = true;
                while i < end {
                    if records && success(&view[i]) {
                        let start = i;
                        while i < end && success(&view[i]) {
                            i += 1;
                        }
                        result.push_str(&fold(&lines[start..i], "successful tests"));
                    } else {
                        // Once diagnostics/custom printing start, reporter-looking
                        // body lines have uncertain ownership. Keep the rest opaque.
                        if !test_record(&view[i]) {
                            records = false;
                        }
                        result.push_str(lines[i]);
                        i += 1;
                    }
                }
            } else {
                // A nested heading may be quoted failure output. Without a
                // trustworthy end to this section, leave its remainder opaque.
                result.extend(lines[i..].iter().copied());
                break;
            }
        } else {
            let line = &view[i];
            if !line.is_empty()
                && !line.starts_with("    Finished ")
                && !line.starts_with("     Running ")
                && !line.trim_start().starts_with("Doc-tests ")
            {
                progress = false;
            }
            result.push_str(lines[i]);
            i += 1;
        }
    }
    Some(result)
}

fn progress_line(line: &str) -> bool {
    let Some(line) = line.strip_prefix("   ") else {
        return false;
    };
    let line = line.trim_start();
    let Some(rest) = ["Compiling ", "Checking ", "Downloading "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
    else {
        return false;
    };
    let mut words = rest.split_whitespace();
    let Some(name) = words.next() else {
        return false;
    };
    let Some(version) = words.next() else {
        return false;
    };
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && version
            .strip_prefix('v')
            .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()) && v.contains('.'))
        && (words.clone().next().is_none()
            || words.next().is_some_and(|w| w.starts_with('(')) && rest.ends_with(')'))
}

fn test_count(line: &str) -> Option<usize> {
    line.strip_prefix("running ")?
        .strip_suffix(" tests")
        .or_else(|| line.strip_prefix("running ")?.strip_suffix(" test"))?
        .parse()
        .ok()
}

fn success(line: &str) -> bool {
    line.strip_prefix("test ")
        .and_then(|s| s.strip_suffix(" ... ok"))
        .is_some_and(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
}

fn test_record(line: &str) -> bool {
    success(line)
        || line.starts_with("test ")
            && (line.ends_with(" ... FAILED") || line.contains(" ... ignored"))
}

fn section_end(view: &[String], start: usize) -> Option<usize> {
    let count = test_count(&view[start])?;
    let mut records = 0;
    for (end, line) in view.iter().enumerate().skip(start + 1) {
        if test_count(line).is_some() {
            return None;
        }
        if line.starts_with("test result: ") {
            let summary = line.strip_prefix("test result: ")?;
            let summary = summary
                .strip_prefix("ok. ")
                .or_else(|| summary.strip_prefix("FAILED. "))?;
            let mut totals = summary.split(';');
            let passed = totals
                .next()?
                .strip_suffix(" passed")?
                .parse::<usize>()
                .ok()?;
            let failed = totals
                .next()?
                .trim()
                .strip_suffix(" failed")?
                .parse::<usize>()
                .ok()?;
            let ignored = totals
                .next()?
                .trim()
                .strip_suffix(" ignored")?
                .parse::<usize>()
                .ok()?;
            let measured = totals
                .next()?
                .trim()
                .strip_suffix(" measured")?
                .parse::<usize>()
                .ok()?;
            let total = passed
                .checked_add(failed)?
                .checked_add(ignored)?
                .checked_add(measured)?;
            return (records == count && total == count).then_some(end);
        }
        if test_record(line) {
            records += 1;
        }
    }
    None
}
