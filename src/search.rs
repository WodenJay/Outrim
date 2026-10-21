use std::collections::HashMap;

pub(crate) fn group_search_hits(text: &str) -> Option<String> {
    let records: Vec<_> = text.split_inclusive('\n').collect();
    let trailing_blank_count = records
        .iter()
        .rev()
        .take_while(|record| matches!(**record, "\n" | "\r\n"))
        .count();
    let record_count = records.len() - trailing_blank_count;
    let parsed: Vec<_> = records[..record_count]
        .iter()
        .map(|record| parse_search_record(record))
        .collect::<Option<_>>()?;
    let mut has_hit = false;
    let mut ordered = false;
    for (index, record) in parsed.iter().enumerate() {
        match record {
            SearchRecord::Line(line) => {
                has_hit |= !line.context;
                ordered |= line.context;
            }
            SearchRecord::Separator(_) => {
                if index == 0
                    || !matches!(parsed[index - 1], SearchRecord::Line(_))
                    || !matches!(parsed.get(index + 1), Some(SearchRecord::Line(_)))
                {
                    return None;
                }
                ordered = true;
            }
            SearchRecord::Limit(_) => {
                if index + 1 != parsed.len() {
                    return None;
                }
                ordered = true;
            }
        }
    }
    if !has_hit {
        return None;
    }

    let mut grouped = if ordered {
        group_in_order(&parsed)?
    } else {
        group_by_path(parsed)?
    };
    for record in &records[record_count..] {
        grouped.push_str(record);
    }
    Some(grouped)
}

// Extended results preserve global order, including separators and notices.
fn group_in_order(records: &[SearchRecord<'_>]) -> Option<String> {
    let mut grouped = String::new();
    let mut previous_path = None;
    let mut repeated_path = false;
    for (index, record) in records.iter().enumerate() {
        match record {
            SearchRecord::Line(line) => {
                if previous_path == Some(line.path) {
                    repeated_path = true;
                } else {
                    grouped.push_str(line.path);
                    grouped.push(':');
                    grouped.push_str(line.line_ending.unwrap_or("\n"));
                }
                grouped.push_str("  ");
                grouped.push_str(line.line_number_text);
                grouped.push_str(line.suffix_after_line);
                grouped.push_str(line.line_ending.unwrap_or(""));
                previous_path = Some(line.path);
            }
            SearchRecord::Separator(raw) => {
                if let Some(SearchRecord::Line(next)) = records.get(index + 1)
                    && previous_path == Some(next.path)
                {
                    grouped.push_str("  ");
                }
                grouped.push_str(raw);
            }
            SearchRecord::Limit(raw) => grouped.push_str(raw),
        }
    }
    repeated_path.then_some(grouped)
}

// Pure hit results retain their original cross-file grouping behavior.
fn group_by_path(records: Vec<SearchRecord<'_>>) -> Option<String> {
    let hit_count = records.len();
    let mut groups: Vec<SearchGroup<'_>> = Vec::new();
    let mut group_by_path = HashMap::new();
    let mut repeated_path = false;

    for record in records {
        let SearchRecord::Line(hit) = record else {
            unreachable!("pure hit results contain only lines");
        };
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

    let mut grouped = String::new();
    let mut emitted_hits = 0;
    for group in groups {
        grouped.push_str(group.path);
        grouped.push(':');
        grouped.push_str(group.hits[0].line_ending.unwrap_or("\n"));
        for hit in group.hits {
            grouped.push_str("  ");
            grouped.push_str(hit.line_number_text);
            grouped.push_str(hit.suffix_after_line);
            match hit.line_ending {
                Some(line_ending) => grouped.push_str(line_ending),
                None if emitted_hits + 1 < hit_count => grouped.push('\n'),
                None => {}
            }
            emitted_hits += 1;
        }
    }
    Some(grouped)
}

enum SearchRecord<'a> {
    Line(SearchLine<'a>),
    Separator(&'a str),
    Limit(&'a str),
}

struct SearchGroup<'a> {
    path: &'a str,
    hits: Vec<SearchLine<'a>>,
}

struct SearchLine<'a> {
    path: &'a str,
    line_number_text: &'a str,
    // Optional column and content stay attached to the validated line number.
    suffix_after_line: &'a str,
    line_ending: Option<&'a str>,
    context: bool,
}

pub(crate) fn is_search_record(raw: &str) -> bool {
    parse_search_record(raw).is_some()
}

fn parse_search_record(raw: &str) -> Option<SearchRecord<'_>> {
    let (record, line_ending) = match raw.strip_suffix('\n') {
        Some(record) => match record.strip_suffix('\r') {
            Some(record) => (record, Some("\r\n")),
            None => (record, Some("\n")),
        },
        None => (raw, None),
    };
    if record == "--" {
        return Some(SearchRecord::Separator(raw));
    }
    if let Some(count) = record
        .strip_prefix('[')
        .and_then(|text| text.strip_suffix(" matches limit reached.]"))
        && !count.is_empty()
        && count.bytes().all(|byte| byte.is_ascii_digit())
        && count.bytes().any(|byte| byte != b'0')
    {
        return Some(SearchRecord::Limit(raw));
    }

    let mut line = None;
    for (separator, character) in record.char_indices() {
        if !matches!(character, ':' | '-') {
            continue;
        }
        let context = character == '-';
        let number_start = separator + 1;
        let Some(number_end) = digit_run_end(record, number_start) else {
            continue;
        };
        let suffix_delimiter = record.as_bytes().get(number_end).copied();
        if if context {
            suffix_delimiter != Some(b'-')
        } else {
            number_end != record.len() && suffix_delimiter != Some(b':')
        } {
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
        if line.is_some() {
            return None;
        }
        line = Some(SearchLine {
            path,
            line_number_text,
            suffix_after_line: &record[number_end..],
            line_ending,
            context,
        });
    }
    line.map(SearchRecord::Line)
}

fn digit_run_end(text: &str, start: usize) -> Option<usize> {
    let end = text[start..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(text.len(), |offset| start + offset);
    (end > start).then_some(end)
}

pub(crate) fn valid_search_path(path: &str) -> bool {
    // Separator or extension distinguishes paths from prose and diagnostics.
    let has_directory_separator = path.contains('/') || path.contains('\\');
    let path_like = has_directory_separator || path.contains('.');
    let contains_whitespace = path.chars().any(char::is_whitespace);
    if path.is_empty() || !path_like || contains_whitespace || path.chars().any(char::is_control) {
        return false;
    }
    !path.contains(':') || is_drive_relative_path(path)
}

fn is_drive_relative_path(path: &str) -> bool {
    matches!(path.find(':'), Some(1)
        if path.as_bytes()[0].is_ascii_alphabetic() && !path[2..].contains(':'))
}
