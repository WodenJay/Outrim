use super::fold;

pub(super) fn compact(lines: &[&str], view: &[String]) -> Option<String> {
    if view.iter().any(|line| line.contains('\x1b')) {
        return None;
    }
    // A launcher/script name alone does not identify a reporter. Its versioned
    // banner and file headings must both be present in this supplied stream.
    if !view.iter().any(|line| banner(line)) {
        return None;
    }
    let mut result = String::new();
    let mut i = 0;
    let mut in_file = false;
    let mut records = false;
    let mut opaque = false;
    while i < lines.len() {
        let line = &view[i];
        if summary(line) {
            opaque = false;
            in_file = false;
            records = false;
        }
        if banner(line) && !opaque {
            in_file = false;
            records = false;
        }
        if !opaque && heading(line) && (i == 0 || view[i - 1].is_empty()) {
            in_file = true;
            records = true;
        } else if in_file && records && pass(line) {
            let start = i;
            while i < lines.len() && pass(&view[i]) {
                i += 1;
            }
            result.push_str(&fold(&lines[start..i], "successful tests"));
            continue;
        } else if in_file && !non_success(line) {
            // Even file headings can be quoted by diagnostics. Once custom
            // printing begins, only a native cycle summary restores ownership.
            records = false;
            opaque |= !line.is_empty();
        }
        result.push_str(lines[i]);
        i += 1;
    }
    Some(result)
}

fn banner(line: &str) -> bool {
    line.strip_prefix("bun test v")
        .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()) && v.contains('.'))
}

fn heading(line: &str) -> bool {
    let Some(path) = line.strip_suffix(':') else {
        return false;
    };
    let Some((stem, extension)) = path.rsplit_once('.') else {
        return false;
    };
    !path.chars().any(char::is_whitespace)
        && matches!(
            extension,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts"
        )
        && [".test", ".spec", "_test", "_spec"]
            .iter()
            .any(|suffix| stem.ends_with(suffix))
}

fn summary(line: &str) -> bool {
    let words: Vec<_> = line.split_whitespace().collect();
    matches!(words.as_slice(), ["Ran", tests, "test" | "tests", "across", files, "file." | "files.", time]
        if tests.parse::<usize>().is_ok() && files.parse::<usize>().is_ok()
            && time.starts_with('[') && time.ends_with(']'))
}

fn pass(line: &str) -> bool {
    line.strip_prefix("(pass) ")
        .or_else(|| line.strip_prefix("✓ "))
        .is_some_and(|name| !name.trim().is_empty())
}

fn non_success(line: &str) -> bool {
    ["(fail) ", "(skip) ", "(todo) ", "✗ ", "» ", "✎ "]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}
