//! Opt-in local facts. Each bridge owns one append-only instance; content paths
//! are relative to that instance and independent of recovery-file lifetimes.
use crate::{
    protocol::{RecordObservationRequest, VERSION},
    storage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) struct Recorder {
    home: PathBuf,
    enabled: bool,
    instance: Mutex<Option<Instance>>,
}

struct Instance {
    directory: PathBuf,
    id: String,
    sequence: u64,
    events: File,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Event {
    pub format_version: u32,
    pub event_id: String,
    pub instance_id: String,
    pub sequence: u64,
    pub timestamp_unix_ms: u128,
    pub host: crate::protocol::Host,
    pub session: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub source: crate::protocol::Source,
    pub exit_code: Option<i32>,
    pub outrim_version: String,
    pub build: Option<String>,
    pub rules_config: String,
    pub outcome: crate::protocol::ObservationOutcome,
    pub reason: Option<String>,
    pub applied_rule: Option<String>,
    pub candidates: Vec<crate::protocol::Candidate>,
    #[serde(default)]
    pub parts: Vec<crate::protocol::ProcessingPart>,
    pub before_chars: Option<usize>,
    pub after_chars: Option<usize>,
    pub original_path: Option<String>,
    pub original_references: Vec<OriginalReference>,
    pub content: Content,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Content {
    pub arguments: String,
    pub original: Option<String>,
    pub r#final: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct OriginalReference {
    pub original_path: String,
    pub record_id: Option<String>,
}

impl Recorder {
    pub(crate) fn new(home: &Path, enabled: bool) -> Self {
        Self {
            home: home.into(),
            enabled,
            instance: Mutex::new(None),
        }
    }

    pub(crate) fn record(&self, request: RecordObservationRequest) -> io::Result<()> {
        if request.version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported observation version",
            ));
        }
        if !self.enabled {
            return Ok(());
        }
        let mut guard = self
            .instance
            .lock()
            .map_err(|_| io::Error::other("Recording lock poisoned"))?;
        if guard.is_none() {
            *guard = Some(Instance::new(&self.home)?);
        }
        let instance = guard.as_mut().unwrap();
        instance.sequence += 1;
        let original_references = references(&self.home, &request.tool.input)?;
        let arguments = instance.save(&serde_json::to_string(&request.tool.input)?, ".json")?;
        let original = request
            .original_text
            .as_deref()
            .map(|text| instance.save(text, ".txt"))
            .transpose()?;
        let final_content = if request.final_text == request.original_text {
            original.clone()
        } else {
            request
                .final_text
                .as_deref()
                .map(|text| instance.save(text, ".txt"))
                .transpose()?
        };
        let event = Event {
            format_version: 1,
            event_id: format!("{}-{}", instance.id, instance.sequence),
            instance_id: instance.id.clone(),
            sequence: instance.sequence,
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
            original_path: request.original_path,
            original_references,
            content: Content {
                arguments,
                original,
                r#final: final_content,
            },
        };
        // Serialize the entire line first; never emit a partially serialized JSON object.
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        instance.events.write_all(&line)?;
        instance.events.flush()
    }
}

impl Instance {
    fn new(home: &Path) -> io::Result<Self> {
        let root = home.join(".outrim").join("records");
        storage::private_directory(&home.join(".outrim"))?;
        storage::private_directory(&root)?;
        let directory = tempfile::Builder::new()
            .prefix("r-")
            .tempdir_in(root)?
            .keep();
        storage::private_directory(&directory)?;
        let id = directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let file = tempfile::NamedTempFile::new_in(&directory)?;
        let events = file
            .persist(directory.join("events.jsonl"))
            .map_err(|error| error.error)?;
        Ok(Self {
            directory,
            id,
            sequence: 0,
            events,
        })
    }

    fn save(&self, text: &str, suffix: &str) -> io::Result<String> {
        let file = tempfile::Builder::new()
            .prefix("c-")
            .suffix(suffix)
            .tempfile_in(&self.directory)?;
        let path = storage::commit(file, text)?;
        Ok(path.file_name().unwrap().to_string_lossy().into_owned())
    }
}

pub(crate) fn events(home: &Path) -> io::Result<Vec<(PathBuf, Event)>> {
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
        let text = match fs::read_to_string(directory.join("events.jsonl")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        // A live writer may not yet have finished its last line.
        for line in text
            .split_terminator('\n')
            .take(text.bytes().filter(|byte| *byte == b'\n').count())
        {
            let event: Event = serde_json::from_str(line)?;
            if event.format_version != 1 {
                return Err(io::Error::other("Unsupported recording format"));
            }
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
