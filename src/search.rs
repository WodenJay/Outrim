use std::collections::HashMap;

pub(crate) fn group_search_hits(text: &str) -> Option<String> {
    let hits: Vec<_> = text
        .split_inclusive('\n')
        .map(parse_search_hit)
        .collect::<Option<_>>()?;
    let hit_count = hits.len();
    let mut groups: Vec<SearchGroup<'_>> = Vec::new();
    let mut group_by_path = HashMap::new();
    let mut repeated_path = false;

    for hit in hits {
        let group_index = if let Some(index) = group_by_path.get(hit.path) {
            repeated_path = true;
            *index
        } else {
            let index = groups.len();
            groups.push(SearchGroup {
                path: hit.path,
                hits: Vec::new(),
            });
            group_by_path.insert(hit.path, index);
            index
        };
        groups[group_index].hits.push(hit);
    }

    if !repeated_path {
        return None;
    }

    let mut grouped = String::with_capacity(text.len());
    let mut emitted_hits = 0;
    for group in groups {
        grouped.push_str(group.path);
        grouped.push(':');
        if group.hits[0].line_ending.is_empty() {
            grouped.push('\n');
        } else {
            grouped.push_str(group.hits[0].line_ending);
        }
        for hit in group.hits {
            grouped.push_str("  ");
            grouped.push_str(hit.line_number_text);
            if hit.suffix_after_line.is_empty() {
                grouped.push(':');
            } else {
                grouped.push_str(hit.suffix_after_line);
            }
            if !hit.line_ending.is_empty() {
                grouped.push_str(hit.line_ending);
            } else if emitted_hits + 1 < hit_count {
                grouped.push('\n');
            }
            emitted_hits += 1;
        }
    }
    Some(grouped)
}

struct SearchGroup<'a> {
    path: &'a str,
    hits: Vec<SearchHit<'a>>,
}

struct SearchHit<'a> {
    path: &'a str,
    line_number_text: &'a str,
    // Optional column and hit content stay attached to the validated line number.
    suffix_after_line: &'a str,
    line_ending: &'a str,
}

fn parse_search_hit(record: &str) -> Option<SearchHit<'_>> {
    let (record, line_ending) = match record.strip_suffix('\n') {
        Some(record) => match record.strip_suffix('\r') {
            Some(record) => (record, "\r\n"),
            None => (record, "\n"),
        },
        None => (record, ""),
    };

    let mut hit = None;
    for (separator, character) in record.char_indices() {
        if character != ':' {
            continue;
        }
        let number_start = separator + 1;
        let Some(number_end) = digit_run_end(record, number_start) else {
            continue;
        };
        if number_end != record.len() && record.as_bytes().get(number_end) != Some(&b':') {
            continue;
        }

        let path = &record[..separator];
        let line_number_text = &record[number_start..number_end];
        let Some(line_number) = line_number_text.parse::<u64>().ok() else {
            continue;
        };
        if !valid_search_path(path) || line_number == 0 {
            continue;
        }
        if is_drive_relative_path(&record[..number_end]) || hit.is_some() {
            return None;
        }
        hit = Some(SearchHit {
            path,
            line_number_text,
            suffix_after_line: &record[number_end..],
            line_ending,
        });
    }
    hit
}

fn digit_run_end(text: &str, start: usize) -> Option<usize> {
    let end = text[start..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(text.len(), |offset| start + offset);
    (end > start).then_some(end)
}

fn valid_search_path(path: &str) -> bool {
    let has_directory_separator = path.contains('/') || path.contains('\\');
    let path_like =
        has_directory_separator || path.chars().any(char::is_alphabetic) || path.contains('.');
    let unqualified_whitespace = !has_directory_separator && path.chars().any(char::is_whitespace);
    if path.is_empty() || !path_like || unqualified_whitespace || path.chars().any(char::is_control)
    {
        return false;
    }
    !path.contains(':') || is_drive_relative_path(path)
}

fn is_drive_relative_path(path: &str) -> bool {
    matches!(path.find(':'), Some(1)
        if path.as_bytes()[0].is_ascii_alphabetic() && !path[2..].contains(':'))
}
