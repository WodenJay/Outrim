//! Reliable synchronous bookkeeping, with an opportunistic resident tokenizer.
//! No output is queued on disk. Both paths use the same durable, idempotent stores.
use crate::{gain, protocol::*, recording, storage, tokens};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::OnceCell,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const WIRE_VERSION: u32 = 1;
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FRAME: usize = 128 * 1024 * 1024;
const ENDPOINT: &str = "bookkeeping.json";

pub(crate) struct Client {
    home: PathBuf,
    recording: bool,
    scope: OnceCell<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    version: u32,
    build: String,
    scope: String,
    event: String,
    recording: bool,
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
enum Operation {
    Delivery(RecordDeliveryRequest),
    Observation(Box<RecordObservationRequest>),
    Recovery(RecoveryRequest),
    Stop,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    gain: Option<String>,
    recording: Option<String>,
}

impl Outcome {
    fn report(&self) -> bool {
        let (items, error) = match (&self.gain, &self.recording) {
            (None, None) => return true,
            (Some(error), None) => ("gain", error.clone()),
            (None, Some(error)) => ("recording", error.clone()),
            (Some(gain), Some(recording)) => ("gain/recording", format!("{gain}; {recording}")),
        };
        // Captured stderr may not end with a newline (including recovery paths).
        eprintln!("\n[outrim] Failed to save {items}: {error}");
        false
    }
}

impl Client {
    pub(crate) fn new(home: &Path, recording: bool) -> Self {
        Self {
            home: home.into(),
            recording,
            scope: OnceCell::new(),
        }
    }

    pub(crate) fn delivery(&self, request: RecordDeliveryRequest) -> bool {
        self.submit(Operation::Delivery(request))
    }

    pub(crate) fn observation(&self, request: RecordObservationRequest) -> bool {
        if !self.recording {
            return true;
        }
        self.submit(Operation::Observation(Box::new(request)))
    }

    pub(crate) fn recovery(&self, request: RecoveryRequest) -> bool {
        self.submit(Operation::Recovery(request))
    }

    fn submit(&self, operation: Operation) -> bool {
        let wants_gain = !matches!(operation, Operation::Observation(_));
        let wants_recording = self.recording
            && match &operation {
                Operation::Delivery(delivery) => delivery.observation.is_some(),
                _ => true,
            };
        let result = (|| {
            if self.scope.get().is_none() {
                let _ = self.scope.set(random_id()?);
            }
            let scope = self.scope.get().unwrap().clone();
            let event = if let Operation::Recovery(recovery) = &operation {
                // Correlation IDs are not identities. Recovery has an explicit logical
                // identity, scoped to its originating host/session/call, unlike deliveries.
                digest(&serde_json::to_vec(&(
                    recovery.host,
                    &recovery.session,
                    &recovery.tool_call_id,
                    &recovery.event_id,
                    "recovery",
                ))?)
            } else {
                random_id()?
            };
            let submission = Submission {
                version: WIRE_VERSION,
                build: build_id().unwrap_or_default(),
                scope,
                event,
                recording: self.recording,
                operation,
            };
            // A lost response is ambiguous, not a reason to assign a new event ID.
            Ok::<_, io::Error>(
                remote(&self.home, &submission).unwrap_or_else(|_| apply(&self.home, submission)),
            )
        })();
        match result {
            Ok(outcome) => outcome.report(),
            Err(error) => Outcome {
                gain: wants_gain.then(|| error.to_string()),
                recording: wants_recording.then(|| error.to_string()),
            }
            .report(),
        }
    }
}

fn apply(home: &Path, submission: Submission) -> Outcome {
    let store = gain::Store::new(home);
    let recorder = recording::Recorder::new(home, submission.recording, &submission.scope);
    let mut outcome = Outcome::default();
    match submission.operation {
        Operation::Delivery(mut delivery) => {
            let counts = tokens::Counts::new(&delivery.original_text, &delivery.final_text);
            outcome.gain = store
                .record(&submission.event, &delivery, counts)
                .err()
                .map(|e| e.to_string());
            if let Some(observation) = delivery.observation.take() {
                let same_text = observation.original_text.as_deref()
                    == Some(&delivery.original_text)
                    && observation.final_text.as_deref() == Some(&delivery.final_text);
                outcome.recording = recorder
                    .record(&submission.event, *observation, same_text.then_some(counts))
                    .err()
                    .map(|e| e.to_string());
            }
        }
        Operation::Observation(observation) => {
            outcome.recording = recorder
                .record(&submission.event, *observation, None)
                .err()
                .map(|e| e.to_string());
        }
        Operation::Recovery(recovery) => {
            let cost = tokens::count(&recovery.text);
            outcome.gain = store
                .record_recovery(&submission.event, &recovery, cost)
                .err()
                .map(|e| e.to_string());
            let observation = RecordObservationRequest {
                message_type: ObservationMessageType::RecordObservation,
                version: recovery.version,
                id: recovery.id,
                host: recovery.host,
                session: Some(recovery.session),
                tool_call_id: Some(recovery.tool_call_id),
                tool: Tool {
                    name: "recovery".into(),
                    input: serde_json::json!(recovery.original_paths),
                    source: Source::Unknown,
                    exit_code: None,
                },
                original_text: None,
                final_text: None,
                outcome: ObservationOutcome::RecoverySkipped,
                applied_rule: None,
                original_path: None,
                candidates: vec![],
                reason: None,
                parts: vec![],
                execution: None,
                metadata_only: true,
                diagnostics: Some(Diagnostics {
                    recovery: Some(RecoveryDetails {
                        event_id: recovery.event_id,
                        tokens: cost,
                        attribution: recovery.attribution,
                        original_paths: recovery.original_paths,
                    }),
                    ..Default::default()
                }),
            };
            outcome.recording = recorder
                .record(&submission.event, observation, None)
                .err()
                .map(|e| e.to_string());
        }
        Operation::Stop => {}
    }
    outcome
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    version: u32,
    build: String,
    port: u16,
    secret: String,
    pid: u32,
}

fn endpoint(home: &Path) -> io::Result<Endpoint> {
    let file = File::open(home.join(".outrim").join(ENDPOINT))?;
    serde_json::from_reader(file.take(4096)).map_err(io::Error::other)
}

fn remote(home: &Path, submission: &Submission) -> io::Result<Outcome> {
    // Restrict the discovery capability before reading it or sending any text.
    storage::private_directory(&home.join(".outrim"))?;
    if let Ok(current) = endpoint(home) {
        if current.version == WIRE_VERSION
            && current.build == submission.build
            && let Ok(result) = exchange(&current, submission)
        {
            return Ok(result);
        }
        // Only the authenticated old worker can act on this request. An unknown
        // wire version is never asked to parse a new-format bookkeeping event.
        if current.version == WIRE_VERSION {
            let stop = Submission {
                version: WIRE_VERSION,
                build: current.build.clone(),
                scope: String::new(),
                event: String::new(),
                recording: false,
                operation: Operation::Stop,
            };
            let _ = exchange(&current, &stop);
        }
    }
    let _startup = storage::lock(&home.join(".outrim/bookkeeping-start.lock"))?;
    if let Ok(current) = endpoint(home)
        && current.version == WIRE_VERSION
        && current.build == submission.build
        && let Ok(result) = exchange(&current, submission)
    {
        return Ok(result);
    }
    // An unresponsive/incompatible live worker must not cause repeated spawns.
    let worker_lock = open_lock(&home.join(".outrim/bookkeeping-worker.lock"))?;
    worker_lock
        .try_lock()
        .map_err(|_| io::Error::other("Resident unavailable"))?;
    drop(worker_lock);
    spawn(home, &submission.build)?;
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(current) = endpoint(home)
            && current.version == WIRE_VERSION
            && current.build == submission.build
            && let Ok(result) = exchange(&current, submission)
        {
            return Ok(result);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Resident startup timed out",
    ))
}

fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn spawn(home: &Path, build: &str) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    // Library consumers without the CLI entry point use the synchronous adapter.
    if !executable.file_stem().is_some_and(|name| name == "outrim") {
        return Err(io::Error::other("Resident executable unavailable"));
    }
    #[cfg(windows)]
    let executable = {
        let directory = home.join(".outrim/workers");
        storage::private_directory(&directory)?;
        let target = directory.join(format!("worker-{build}.exe"));
        if !target.exists() {
            let mut copy = tempfile::NamedTempFile::new_in(&directory)?;
            io::copy(&mut File::open(&executable)?, copy.as_file_mut())?;
            copy.as_file().sync_all()?;
            copy.persist_noclobber(&target).map_err(|e| e.error)?;
        }
        // The resident maps a private copy, never the installed executable being upgraded.
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path != target {
                let _ = fs::remove_file(path);
            }
        }
        target
    };
    launch(&executable, home, build)
}

#[cfg(unix)]
fn launch(executable: &Path, home: &Path, build: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(executable);
    command
        .arg("__bookkeeping")
        .arg(home)
        .arg(build)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and only changes the child's session.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    command.spawn()?;
    Ok(())
}

#[cfg(windows)]
fn launch(executable: &Path, home: &Path, build: &str) -> io::Result<()> {
    use std::{ffi::OsString, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOW,
    };
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut arguments = OsString::from("\"");
    arguments.push(executable);
    arguments.push("\" __bookkeeping \"");
    arguments.push(home);
    arguments.push("\" \"");
    arguments.push(build);
    arguments.push("\"");
    let mut arguments: Vec<u16> = arguments.encode_wide().chain(Some(0)).collect();
    // SAFETY: Win32 receives valid NUL-terminated buffers and initialized structs.
    // Explicitly disable ALL handle inheritance, including unrelated caller pipes.
    unsafe {
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            application.as_ptr(),
            arguments.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        windows_sys::Win32::Foundation::CloseHandle(process.hThread);
        windows_sys::Win32::Foundation::CloseHandle(process.hProcess);
    }
    Ok(())
}

/// Private CLI entry point; not a host/session bridge.
#[doc(hidden)]
pub fn run(home: &Path, build: &str) -> io::Result<()> {
    storage::private_directory(&home.join(".outrim"))?;
    let lock = open_lock(&home.join(".outrim/bookkeeping-worker.lock"))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let current = Endpoint {
        version: WIRE_VERSION,
        build: build.into(),
        port: listener.local_addr()?.port(),
        secret: random_id()?,
        pid: std::process::id(),
    };
    let mut file = tempfile::NamedTempFile::new_in(home.join(".outrim"))?;
    serde_json::to_writer(&mut file, &current)?;
    file.flush()?;
    storage::replace(file, &home.join(".outrim").join(ENDPOINT))?;
    for connection in listener.incoming() {
        let Ok(mut stream) = connection else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(TIMEOUT));
        let _ = stream.set_write_timeout(Some(TIMEOUT));
        if authenticate(&mut stream, &current.secret, false).is_err() {
            continue;
        }
        let Ok(submission) = read_frame::<Submission>(&mut stream) else {
            continue;
        };
        if submission.version != WIRE_VERSION || submission.build != build {
            continue;
        }
        if matches!(submission.operation, Operation::Stop) {
            let _ = write_frame(&mut stream, &Outcome::default());
            break;
        }
        if !valid_id(&submission.scope) || !valid_id(&submission.event) {
            continue;
        }
        let result = apply(home, submission);
        let _ = write_frame(&mut stream, &result);
    }
    let _ = fs::remove_file(home.join(".outrim").join(ENDPOINT));
    Ok(())
}

fn exchange(endpoint: &Endpoint, submission: &Submission) -> io::Result<Outcome> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(200))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    authenticate(&mut stream, &endpoint.secret, true)?;
    write_frame(&mut stream, submission)?;
    read_frame(&mut stream)
}

// Mutual challenge/response: never send text to an unauthenticated server,
// including when another user has rebound a stale loopback port. Nonces prevent replay.
fn authenticate(stream: &mut TcpStream, secret: &str, client: bool) -> io::Result<()> {
    if !valid_id(secret) {
        return Err(io::Error::other("Invalid resident capability"));
    }
    let mut client_nonce = [0; 32];
    let mut server_nonce = [0; 32];
    let mut received = [0; 32];
    if client {
        getrandom::fill(&mut client_nonce).map_err(io::Error::other)?;
        stream.write_all(&client_nonce)?;
        stream.read_exact(&mut server_nonce)?;
        stream.read_exact(&mut received)?;
        verify(
            &received,
            &proof(secret, &client_nonce, &server_nonce, b"server"),
        )?;
        stream.write_all(&proof(secret, &client_nonce, &server_nonce, b"client"))
    } else {
        stream.read_exact(&mut client_nonce)?;
        getrandom::fill(&mut server_nonce).map_err(io::Error::other)?;
        stream.write_all(&server_nonce)?;
        stream.write_all(&proof(secret, &client_nonce, &server_nonce, b"server"))?;
        stream.read_exact(&mut received)?;
        verify(
            &received,
            &proof(secret, &client_nonce, &server_nonce, b"client"),
        )
    }
}

fn proof(secret: &str, client: &[u8; 32], server: &[u8; 32], role: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(secret)
        .chain_update(client)
        .chain_update(server)
        .chain_update(role)
        .finalize()
        .into()
}

fn verify(left: &[u8; 32], right: &[u8; 32]) -> io::Result<()> {
    if left
        .iter()
        .zip(right)
        .fold(0, |difference, (a, b)| difference | (a ^ b))
        == 0
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Resident authentication failed",
        ))
    }
}

fn write_frame(stream: &mut TcpStream, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("Bookkeeping message too large"));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)
}

fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut TcpStream) -> io::Result<T> {
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MAX_FRAME {
        return Err(io::Error::other("Bookkeeping message too large"));
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn random_id() -> io::Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(hex(&bytes))
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn valid_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn build_id() -> io::Result<String> {
    let executable = std::env::current_exe()?;
    let metadata = executable.metadata()?;
    Ok(digest(
        format!(
            "{WIRE_VERSION}:{}:{:?}:{}:{:?}",
            env!("CARGO_PKG_VERSION"),
            executable,
            metadata.len(),
            metadata.modified()?
        )
        .as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lost_commit_reply_falls_back_without_duplicate_gain_or_recording() {
        let home = tempfile::tempdir().unwrap();
        storage::private_directory(&home.path().join(".outrim")).unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = Endpoint {
            version: WIRE_VERSION,
            build: build_id().unwrap(),
            port: listener.local_addr().unwrap().port(),
            secret: random_id().unwrap(),
            pid: std::process::id(),
        };
        fs::write(
            home.path().join(".outrim/bookkeeping.json"),
            serde_json::to_vec(&endpoint).unwrap(),
        )
        .unwrap();
        let worker_home = home.path().to_owned();
        // The real transport and stores, failing only at the commit/reply seam.
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            authenticate(&mut stream, &endpoint.secret, false).unwrap();
            let submission = read_frame::<Submission>(&mut stream).unwrap();
            let result = apply(&worker_home, submission);
            assert!(result.gain.is_none() && result.recording.is_none());
            // Commit succeeded; close without an acknowledgement.
        });
        let delivery = serde_json::from_value(json!({
            "type":"record_delivery", "version":2, "id":"same-id",
            "disposition":"compressed", "applied_rule":"rule",
            "original_text":"hello world", "final_text":"hello",
            "observation": {
                "type":"record_observation", "version":2, "id":"facts", "host":"exec",
                "tool":{"name":"shell", "input":{"command":"private arguments"}, "source":"unknown"},
                "original_text":"hello world", "final_text":"hello", "outcome":"compressed", "candidates":[]
            }
        })).unwrap();
        assert!(Client::new(home.path(), true).delivery(delivery));
        worker.join().unwrap();
        let stats: serde_json::Value =
            serde_json::from_slice(&fs::read(home.path().join(".outrim/gain.json")).unwrap())
                .unwrap();
        assert_eq!(stats["outcomes"]["compressed"], 1);
        assert_eq!(stats["rules"]["rule"]["before_tokens"], 2);
        let directories = fs::read_dir(home.path().join(".outrim/records"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(directories.len(), 1);
        let index = fs::read_to_string(directories[0].path().join("events.jsonl")).unwrap();
        assert_eq!(index.lines().count(), 1);
        assert!(index.contains("o200k_base"));
    }
}
