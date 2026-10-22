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
    while i < lines.len() {
        let line = &view[i];
        if banner(line) {
            in_file = false;
            records = false;
        }
        if heading(line) && (i == 0 || view[i - 1].is_empty()) {
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
            // Diagnostic bodies may themselves contain reporter-shaped text.
            // Only a new explicit file heading re-establishes ownership.
            records = false;
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
    !path.is_empty()
        && !path.chars().any(char::is_whitespace)
        && [
            ".test.ts",
            ".test.tsx",
            ".test.js",
            ".test.jsx",
            ".spec.ts",
            ".spec.tsx",
            ".spec.js",
            ".spec.jsx",
        ]
        .iter()
        .any(|ext| path.ends_with(ext))
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
