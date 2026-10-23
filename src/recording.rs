//! Opt-in local facts. Each caller scope owns one append-only instance; content paths
//! are relative to that instance and independent of recovery-file lifetimes.
mod report;
mod settings;

pub use report::render_detail;
pub(crate) use settings::enabled;
pub use settings::set_recording;

use crate::{
    protocol::{RecordObservationRequest, VERSION},
    storage, tokens,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(unix)]
use std::fs::File;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) struct Recorder {
    home: PathBuf,
    enabled: bool,
    instance_id: String,
}

#[derive(Serialize, Deserialize)]
struct Event {
    format_version: u32,
    event_id: String,
    instance_id: String,
    sequence: u64,
    timestamp_unix_ms: u128,
    host: crate::protocol::Host,
    session: Option<String>,
    tool_call_id: Option<String>,
    tool_name: String,
    source: crate::protocol::Source,
    exit_code: Option<i32>,
    outrim_version: String,
    build: Option<String>,
    rules_config: String,
    outcome: crate::protocol::ObservationOutcome,
    reason: Option<String>,
    applied_rule: Option<String>,
    candidates: Vec<crate::protocol::Candidate>,
    #[serde(default)]
    parts: Vec<crate::protocol::ProcessingPart>,
    before_chars: Option<usize>,
    after_chars: Option<usize>,
    token_encoding: String,
    before_tokens: Option<u64>,
    after_tokens: Option<u64>,
    original_path: Option<String>,
    original_references: Vec<OriginalReference>,
    content: Content,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution: Option<crate::protocol::ExecutionMetadata>,
    #[serde(default)]
    metadata_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostics: Option<crate::protocol::Diagnostics>,
}

#[derive(Serialize, Deserialize)]
struct Content {
    arguments: Option<String>,
    original: Option<String>,
    r#final: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct OriginalReference {
    original_path: String,
    record_id: Option<String>,
}

impl Recorder {
    pub(crate) fn new(home: &Path, enabled: bool, scope: &str) -> Self {
        Self {
            home: home.into(),
            enabled,
            instance_id: format!("r-{scope}"),
        }
    }

    pub(crate) fn record(
        &self,
        event_id: &str,
        request: RecordObservationRequest,
        counts: Option<tokens::Counts>,
    ) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if request.version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported observation version",
            ));
        }
        storage::private_directory(&self.home.join(".outrim"))?;
        let _lock = storage::lock(&self.home.join(".outrim/gain.lock"))?;
        // Explicit recovery identities survive bridge restarts; ordinary deliveries
        // remain scoped to their caller even when correlation IDs are reused.
        if request
            .diagnostics
            .as_ref()
            .is_some_and(|d| d.recovery.is_some())
        {
            for (directory, event) in events(&self.home)? {
                if event.event_id == event_id {
                    OpenOptions::new()
                        .write(true)
                        .open(directory.join("events.jsonl"))?
                        .sync_all()?;
                    return sync_directory(&directory);
                }
            }
        }
        let root = self.home.join(".outrim/records");
        let directory = root.join(&self.instance_id);
        for path in [self.home.join(".outrim"), root, directory.clone()] {
            storage::private_directory(&path)?;
            sync_directory(path.parent().unwrap())?;
        }
        let index_path = directory.join("events.jsonl");
        let bytes = match fs::read(&index_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        // Validate all complete lines before repairing an interrupted tail.
        let (mut recorded, complete_len) = complete_events(&bytes)?;
        let tail = &bytes[complete_len..];
        let recovered = if tail.is_empty() {
            None
        } else {
            match serde_json::from_slice::<Event>(tail) {
                Ok(event) => {
                    validate_event(&event)?;
                    Some(event)
                }
                Err(error) if error.is_eof() => None,
                Err(error) => return Err(error.into()),
            }
        };
        let mut index = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&index_path)?;
        if let Some(event) = recovered {
            // Preserve a valid object whose terminating newline was interrupted.
            index.seek(SeekFrom::End(0))?;
            index.write_all(b"\n")?;
            recorded.push(event);
        } else if !tail.is_empty() {
            index.set_len(complete_len as u64)?;
        }
        if recorded.iter().any(|event| event.event_id == event_id) {
            // Retry the durability barrier even when the complete line exists.
            index.sync_all()?;
            return sync_directory(&directory);
        }
        let sequence = recorded
            .iter()
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Recording sequence exhausted"))?;
        let original_references = references(&self.home, &request.tool.input)?;
        let arguments = if request.metadata_only {
            None
        } else {
            Some(save_content(
                &directory,
                event_id,
                "arguments.json",
                &serde_json::to_string(&request.tool.input)?,
            )?)
        };
        let original = request
            .original_text
            .as_deref()
            .filter(|_| !request.metadata_only)
            .map(|text| save_content(&directory, event_id, "original.txt", text))
            .transpose()?;
        let final_content = if request.final_text == request.original_text {
            original.clone()
        } else {
            request
                .final_text
                .as_deref()
                .filter(|_| !request.metadata_only)
                .map(|text| save_content(&directory, event_id, "final.txt", text))
                .transpose()?
        };
        let (before_tokens, after_tokens) = if request.metadata_only {
            (None, None)
        } else if let Some(counts) = counts {
            (Some(counts.before), Some(counts.after))
        } else if let (Some(original), Some(final_text)) =
            (&request.original_text, &request.final_text)
        {
            let counts = tokens::Counts::new(original, final_text);
            (Some(counts.before), Some(counts.after))
        } else {
            (
                request.original_text.as_deref().map(tokens::count),
                request.final_text.as_deref().map(tokens::count),
            )
        };
        let event = Event {
            format_version: 1,
            event_id: event_id.into(),
            instance_id: self.instance_id.clone(),
            sequence,
            timestamp_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_millis(),
            host: request.host,
            session: request.session,
            tool_call_id: request.tool_call_id,
            tool_name: request.tool.name,
            source: request.tool.source,
            exit_code: request.tool.exit_code,
            outrim_version: env!("CARGO_PKG_VERSION").into(),
            build: option_env!("OUTRIM_BUILD_ID").map(str::to_owned),
            rules_config: format!("builtin/{}", env!("CARGO_PKG_VERSION")),
            outcome: request.outcome,
            reason: request.reason,
            applied_rule: request.applied_rule,
            candidates: request.candidates,
            parts: request.parts,
            before_chars: request
                .original_text
                .as_deref()
                .map(|text| text.chars().count()),
            after_chars: request
                .final_text
                .as_deref()
                .map(|text| text.chars().count()),
            token_encoding: tokens::ENCODING.into(),
            before_tokens,
            after_tokens,
            original_path: request.original_path,
            original_references,
            execution: request.execution,
            metadata_only: request.metadata_only,
            diagnostics: request.diagnostics,
            content: Content {
                arguments,
                original,
                r#final: final_content,
            },
        };
        // Serialize the entire line first; never emit a partially serialized JSON object.
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        index.seek(SeekFrom::End(0))?;
        index.write_all(&line)?;
        index.sync_all()?;
        sync_directory(&directory)
    }
}

fn save_content(directory: &Path, event_id: &str, kind: &str, text: &str) -> io::Result<String> {
    let name = format!("c-{event_id}-{kind}");
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    storage::replace(file, &directory.join(&name))?;
    sync_directory(directory)?;
    Ok(name)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn validate_event(event: &Event) -> io::Result<()> {
    if event.format_version != 1 {
        return Err(io::Error::other("Unsupported recording format"));
    }
    Ok(())
}

fn complete_events(bytes: &[u8]) -> io::Result<(Vec<Event>, usize)> {
    let mut events = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            let event: Event = serde_json::from_slice(&bytes[start..index])?;
            validate_event(&event)?;
            events.push(event);
            start = index + 1;
        }
    }
    Ok((events, start))
}

fn events(home: &Path) -> io::Result<Vec<(PathBuf, Event)>> {
    let root = home.join(".outrim").join("records");
    let instances = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut directories = instances
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    directories.sort();
    let mut result = Vec::new();
    for directory in directories {
        if !directory.is_dir() {
            continue;
        }
        let bytes = match fs::read(directory.join("events.jsonl")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        // A live writer may not yet have finished its last line, even its UTF-8.
        for event in complete_events(&bytes)?.0 {
            result.push((directory.clone(), event));
        }
    }
    Ok(result)
}

fn references(home: &Path, input: &Value) -> io::Result<Vec<OriginalReference>> {
    let mut strings = Vec::new();
    input_strings(input, &mut strings);
    if !strings
        .iter()
        .any(|text| text.replace('\\', "/").contains(".outrim/outputs/"))
    {
        return Ok(Vec::new());
    }
    let mut paths = BTreeMap::new();
    collect_originals(&home.join(".outrim/outputs"), &mut paths)?;
    for (_, event) in events(home)? {
        for path in event.original_path.into_iter().chain(
            event
                .parts
                .into_iter()
                .filter_map(|part| part.original_path),
        ) {
            paths.insert(
                path.replace('\\', "/"),
                OriginalReference {
                    original_path: path,
                    record_id: Some(event.event_id.clone()),
                },
            );
        }
    }
    Ok(paths
        .into_iter()
        .filter(|(path, _)| {
            strings
                .iter()
                .any(|value| directly_contains_path(&value.replace('\\', "/"), path))
        })
        .map(|(_, reference)| reference)
        .collect())
}

fn input_strings<'a>(value: &'a Value, strings: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => strings.push(text),
        Value::Array(values) => {
            for value in values {
                input_strings(value, strings);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                input_strings(value, strings);
            }
        }
        _ => {}
    }
}

fn directly_contains_path(value: &str, path: &str) -> bool {
    value.match_indices(path).any(|(index, _)| {
        let boundary = |c: char| c.is_whitespace() || "\"'`=;()[]{}<>|&,".contains(c);
        (index == 0 || value[..index].chars().next_back().is_some_and(boundary))
            && (index + path.len() == value.len()
                || value[index + path.len()..]
                    .chars()
                    .next()
                    .is_some_and(boundary))
    })
}

fn collect_originals(
    directory: &Path,
    paths: &mut BTreeMap<String, OriginalReference>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_originals(&entry.path(), paths)?;
        } else if kind.is_file() {
            let path = entry.path().to_string_lossy().into_owned();
            paths.insert(
                path.replace('\\', "/"),
                OriginalReference {
                    original_path: path,
                    record_id: None,
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> RecordObservationRequest {
        serde_json::from_value(serde_json::json!({
            "type": "record_observation",
            "version": VERSION,
            "id": "observation",
            "host": "exec",
            "tool": {"name": "shell", "input": {}, "source": "unknown"},
            "original_text": "original é",
            "final_text": "short",
            "outcome": "compressed",
            "candidates": []
        }))
        .unwrap()
    }

    #[test]
    fn retries_are_scoped_and_clear_does_not_restore_old_events() {
        let home = tempfile::tempdir().unwrap();
        let scope = "a".repeat(64);
        let event_id = "b".repeat(64);
        let recorder = Recorder::new(home.path(), true, &scope);
        recorder.record(&event_id, request(), None).unwrap();
        let directory = home
            .path()
            .join(".outrim/records")
            .join(format!("r-{scope}"));
        let index = directory.join("events.jsonl");
        let before = fs::read(&index).unwrap();
        Recorder::new(home.path(), true, &scope)
            .record(&event_id, request(), None)
            .unwrap();
        assert_eq!(before, fs::read(&index).unwrap());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 4);
        Recorder::new(home.path(), true, &"c".repeat(64))
            .record(&event_id, request(), None)
            .unwrap();
        assert_eq!(events(home.path()).unwrap().len(), 2);

        // Exercise the same deletion under the same lock used by gain::clear.
        {
            let _lock = storage::lock(&home.path().join(".outrim/gain.lock")).unwrap();
            fs::remove_dir_all(home.path().join(".outrim/records")).unwrap();
        }
        let next_id = "d".repeat(64);
        recorder.record(&next_id, request(), None).unwrap();
        let recorded = events(home.path()).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1.event_id, next_id);
        assert_eq!(recorded[0].1.sequence, 1);
    }

    #[test]
    fn interrupted_tail_preserves_complete_events_and_corruption_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let scope = "a".repeat(64);
        let recorder = Recorder::new(home.path(), true, &scope);
        let first = "b".repeat(64);
        recorder.record(&first, request(), None).unwrap();
        let index = home
            .path()
            .join(format!(".outrim/records/r-{scope}/events.jsonl"));
        let before = fs::read(&index).unwrap();
        let mut interrupted = before.clone();
        interrupted.extend_from_slice(b"{\"format_version\":1,\"event_id\":\"partial");
        fs::write(&index, interrupted).unwrap();
        recorder.record(&"c".repeat(64), request(), None).unwrap();
        let repaired = fs::read(&index).unwrap();
        assert!(repaired.starts_with(&before));
        assert_eq!(events(home.path()).unwrap().len(), 2);

        // A complete JSON object without its newline must not be discarded.
        fs::write(&index, &repaired[..repaired.len() - 1]).unwrap();
        recorder.record(&"c".repeat(64), request(), None).unwrap();
        assert_eq!(fs::read(&index).unwrap(), repaired);

        let mut corrupt = repaired;
        corrupt.extend_from_slice(
            b"not json
",
        );
        fs::write(&index, &corrupt).unwrap();
        assert!(recorder.record(&first, request(), None).is_err());
        assert_eq!(fs::read(&index).unwrap(), corrupt);
    }

    #[test]
    fn disabled_recording_leaves_no_directory() {
        let home = tempfile::tempdir().unwrap();
        Recorder::new(home.path(), false, &"a".repeat(64))
            .record(&"b".repeat(64), request(), None)
            .unwrap();
        assert!(!home.path().join(".outrim").exists());
    }
}
