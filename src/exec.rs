//! Process supervision and bounded byte capture. Compression never reruns a command.
use outrim::{
    execution::{Context, Execution},
    protocol::{Host, OutputStream},
};
use std::time::{Duration, Instant};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    process::{Command, ExitStatus, Stdio},
    thread,
};
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix::Supervisor;
#[cfg(windows)]
use windows::Supervisor;

const CAPTURE_LIMIT: usize = 8 * 1024 * 1024;

pub(crate) fn run(
    arguments: &[OsString],
    no_compress: bool,
    timeout: Option<Duration>,
) -> io::Result<ExitStatus> {
    let mut command = Command::new(&arguments[0]);
    command.args(&arguments[1..]).stdin(Stdio::inherit());
    let supervisor = Supervisor::new(&mut command)?;
    if no_compress {
        let mut child = supervisor.spawn(&mut command)?;
        return wait(&supervisor, &mut child, timeout);
    }
    // Random invocation identity is not a fabricated host session or tool call.
    let identity = format!(
        "exec-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let execution = super::user_home().ok().map(|home| {
        Execution::new(
            &home,
            arguments,
            Context {
                unit_id: identity,
                host: Host::Exec,
                session: None,
                tool_call_id: None,
                source: None,
                recording: None,
            },
        )
    });
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = supervisor.spawn(&mut command)?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || capture(stdout, io::stdout()));
    let err = thread::spawn(move || capture(stderr, io::stderr()));
    let status = wait(&supervisor, &mut child, timeout)?;
    // Join and forward independently: a closed stdout consumer must not hide stderr.
    let stdout = out
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"))
        .and_then(|result| result);
    let stderr = err
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"))
        .and_then(|result| result);
    let stdout = stdout.and_then(|capture| {
        deliver(
            execution.as_ref(),
            OutputStream::Stdout,
            capture,
            status.code(),
            io::stdout(),
        )
    });
    let stderr = stderr.and_then(|capture| {
        deliver(
            execution.as_ref(),
            OutputStream::Stderr,
            capture,
            status.code(),
            io::stderr(),
        )
    });
    for result in [stdout, stderr, io::stdout().flush(), io::stderr().flush()] {
        if let Err(error) = result
            && error.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(error);
        }
    }
    Ok(status)
}

fn wait(
    supervisor: &Supervisor,
    child: &mut std::process::Child,
    timeout: Option<Duration>,
) -> io::Result<ExitStatus> {
    let start = Instant::now();
    let mut stopping = None;
    loop {
        let signal = supervisor.cancelled();
        if stopping.is_none()
            && (signal != 0 || timeout.is_some_and(|limit| start.elapsed() >= limit))
        {
            supervisor.stop(child, if signal == 0 { 15 } else { signal });
            stopping = Some(Instant::now());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                supervisor.cleanup(child);
                return Ok(status);
            }
            Err(error) => {
                supervisor.cleanup(child);
                let _ = child.wait();
                return Err(error);
            }
            Ok(None) => {}
        }
        if stopping.is_some_and(|start| start.elapsed() >= Duration::from_millis(150)) {
            supervisor.cleanup(child);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn exit(status: ExitStatus) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
            std::process::exit(128 + signal);
        }
    }
    std::process::exit(status.code().unwrap_or(1));
}

// Crossing the per-stream byte limit permanently switches that stream to direct output.
struct Capture {
    bytes: Vec<u8>,
    bypass: bool,
}

fn capture(mut input: impl Read, mut output: impl Write) -> io::Result<Capture> {
    let mut captured = Vec::new();
    let mut direct = false;
    let mut block = [0; 16 * 1024];
    loop {
        let length = match input.read(&mut block) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if length == 0 {
            break;
        }
        if !direct && captured.len() + length > CAPTURE_LIMIT {
            output.write_all(&captured)?;
            captured.clear();
            direct = true;
        }
        if direct {
            output.write_all(&block[..length])?;
            output.flush()?;
        } else {
            captured.extend_from_slice(&block[..length]);
        }
    }
    Ok(Capture {
        bytes: captured,
        bypass: direct,
    })
}

fn deliver(
    execution: Option<&Execution>,
    stream: OutputStream,
    capture: Capture,
    exit_code: Option<i32>,
    mut output: impl Write,
) -> io::Result<()> {
    if capture.bypass {
        if let Some(execution) = execution {
            execution.bypass(stream, exit_code, "capture_limit");
        }
        return Ok(());
    }
    if capture
        .bytes
        .iter()
        .any(|byte| byte.is_ascii_control() && !b"\t\r\n\x1b".contains(byte))
    {
        output.write_all(&capture.bytes)?;
        if let Some(execution) = execution {
            execution.bypass(stream, exit_code, "non_text");
        }
        return Ok(());
    }
    match String::from_utf8(capture.bytes) {
        Ok(text) => match execution {
            Some(execution) => execution.deliver(stream, text, exit_code, output),
            None => output.write_all(text.as_bytes()),
        },
        Err(error) => {
            output.write_all(error.as_bytes())?;
            if let Some(execution) = execution {
                execution.bypass(stream, exit_code, "non_text");
            }
            Ok(())
        }
    }
}
