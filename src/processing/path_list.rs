use super::search_hits::{is_search_record, valid_search_path};

pub(crate) fn group_prefixes(text: &str) -> Option<String> {
    let records: Vec<_> = text.split_inclusive('\n').collect();
    let path_count = records
        .iter()
        .rposition(|record| !matches!(*record, "\n" | "\r\n"))
        .map_or(0, |index| index + 1);
    let paths = records[..path_count]
        .iter()
        .map(|raw| {
            let path = raw.strip_suffix('\n').unwrap_or(raw);
            let path = if raw.ends_with('\n') {
                path.strip_suffix('\r').unwrap_or(path)
            } else {
                path
            };
            (valid_search_path(path) && !is_search_record(raw)).then_some(path)
        })
        .collect::<Option<Vec<_>>>()?;

    let mut output = String::new();
    let mut grouped = false;
    let mut index = 0;
    while index < paths.len() {
        let prefix = paths.get(index + 1).and_then(|next| {
            // Only separator positions strictly inside both paths may be cut.
            paths[index]
                .bytes()
                .zip(next.bytes())
                .take_while(|(left, right)| left == right)
                .enumerate()
                .filter(|(offset, byte)| {
                    matches!(byte.0, b'/' | b'\\')
                        && offset + 1 < paths[index].len()
                        && offset + 1 < next.len()
                })
                .map(|(offset, _)| &paths[index][..offset + 1])
                .last()
        });
        if let Some(prefix) = prefix {
            let end = index
                + paths[index..]
                    .iter()
                    .take_while(|path| path.starts_with(prefix) && path.len() > prefix.len())
                    .count();
            let ending = &records[index][paths[index].len()..];
            let count = end - index;
            // Suffixes retain their own terminators; only the header adds one.
            if prefix.chars().count() * (count - 1) > 2 * count + ending.chars().count() {
                output.push_str(prefix);
                output.push_str(ending);
                for raw in &records[index..end] {
                    output.push_str("  ");
                    output.push_str(&raw[prefix.len()..]);
                }
                grouped = true;
                index = end;
                continue;
            }
        }
        output.push_str(records[index]);
        index += 1;
    }
    for raw in &records[path_count..] {
        output.push_str(raw);
    }
    grouped.then_some(output)
}
