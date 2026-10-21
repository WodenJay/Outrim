use super::transaction::{self, Plan};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

#[derive(Serialize, Deserialize)]
struct Active {
    pid: u32,
    work: PathBuf,
}

pub(super) struct Task {
    root: PathBuf,
    lock: File,
}

impl Task {
    pub fn acquire(executable: &Path) -> io::Result<Option<Self>> {
        let name = executable.file_name().unwrap().to_string_lossy();
        let root = executable.with_file_name(format!(".{name}.outrim-update"));
        match fs::create_dir(&root) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&root)?.file_type().is_dir() {
                    return Err(io::Error::other(
                        "update workspace is not a regular directory",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        let lock = open_lock(&root)?;
        match lock.try_lock() {
            Ok(()) => (),
            Err(TryLockError::WouldBlock) => {
                pending();
                return Ok(None);
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        if let Ok(bytes) = fs::read(root.join("active.json")) {
            let active: Active = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            // Covers the short lock hand-off between the preparing process and helper.
            if alive(active.pid) {
                pending();
                return Ok(None);
            }
            if !root.join("result.txt").exists() {
                eprintln!("Previous update was interrupted; checking the installation again.");
            }
            cleanup_work(&active.work);
            fs::remove_file(root.join("active.json"))?;
        }
        let result_path = root.join("result.txt");
        if let Ok(result) = fs::read_to_string(&result_path) {
            eprintln!("Previous background update: {result}");
            fs::remove_file(result_path)?;
        }
        Ok(Some(Self { root, lock }))
    }

    pub fn workspace(&self) -> io::Result<tempfile::TempDir> {
        tempfile::tempdir_in(&self.root)
    }

    pub fn handoff(
        self,
        executable: &Path,
        work: tempfile::TempDir,
        plan: &Plan,
    ) -> io::Result<()> {
        let plan_path = work.path().join("plan.json");
        write_json(&plan_path, plan)?;
        let helper = work
            .path()
            .join(format!("helper{}", std::env::consts::EXE_SUFFIX));
        fs::copy(executable, &helper)?;
        let mut child = spawn(&helper, &self.root, work.path())?;
        if let Err(error) = write_json(
            &self.root.join("active.json"),
            &Active {
                pid: child.id(),
                work: work.path().to_path_buf(),
            },
        ) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let work = work.keep();
        drop(self.lock);
        // Acknowledge actual helper ownership, not just successful process creation.
        while !work.join("ready").exists() {
            if let Some(status) = child.try_wait()? {
                return Err(io::Error::other(format!(
                    "update helper exited before accepting the task: {status}"
                )));
            }
            thread::sleep(Duration::from_millis(10));
        }
        pending();
        for target in &plan.targets {
            if let Some(agent) = target.agent {
                println!(
                    "After completion, restart {} to activate the updated integration.",
                    agent.name()
                );
            }
        }
        Ok(())
    }
}

fn pending() {
    println!(
        "Update prepared; a background helper is waiting for files to be released. Update is NOT complete. Existing sessions will not be stopped."
    );
}

pub fn apply(root: &Path, work: &Path) -> io::Result<()> {
    let lock = open_lock(root)?;
    lock.lock()?;
    let mut plan: Plan =
        serde_json::from_slice(&fs::read(work.join("plan.json"))?).map_err(io::Error::other)?;
    fs::write(work.join("ready"), b"ready")?;
    let result = loop {
        match plan.commit() {
            Err(error) if transaction::busy(&error) => thread::sleep(Duration::from_millis(250)),
            result => break result,
        }
    };
    let message = match &result {
        Ok(()) => "completed on disk; restart the affected Agents to activate".to_string(),
        Err(error) => format!("FAILED: {error}"),
    };
    fs::write(root.join("result.txt"), message)?;
    plan.cleanup();
    // On Windows the helper itself can only be removed on the next invocation.
    // Keep active.json with its PID so abandoned staging can be cleaned then.
    result
}

fn open_lock(root: &Path) -> io::Result<File> {
    let path = root.join("lock");
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && !metadata.is_file()
    {
        return Err(io::Error::other("update lock is not a regular file"));
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    staged.write_all(&serde_json::to_vec(value).map_err(io::Error::other)?)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn cleanup_work(work: &Path) {
    if let Ok(bytes) = fs::read(work.join("plan.json"))
        && let Ok(plan) = serde_json::from_slice::<Plan>(&bytes)
    {
        plan.cleanup();
    }
    let _ = fs::remove_dir_all(work);
}

#[cfg(windows)]
struct Child {
    handle: windows_sys::Win32::Foundation::HANDLE,
    pid: u32,
}

#[cfg(windows)]
impl Child {
    fn id(&self) -> u32 {
        self.pid
    }
    fn try_wait(&mut self) -> io::Result<Option<u32>> {
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };
        // SAFETY: handle is a live process handle owned by this value.
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            status => Ok(Some(status)),
        }
    }
    fn kill(&mut self) -> io::Result<()> {
        // SAFETY: only the helper created by this updater can be terminated on handoff failure.
        if unsafe { windows_sys::Win32::System::Threading::TerminateProcess(self.handle, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn wait(&mut self) -> io::Result<()> {
        // SAFETY: handle remains live throughout the wait.
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle, u32::MAX);
        }
        Ok(())
    }
}
#[cfg(windows)]
impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: this value uniquely owns the process handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn spawn(helper: &Path, root: &Path, work: &Path) -> io::Result<Child> {
    use std::{ffi::OsString, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOW,
    };
    let application: Vec<u16> = helper.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut arguments = OsString::from("\"");
    arguments.push(helper);
    arguments.push("\" __upgrade-apply \"");
    arguments.push(root);
    arguments.push("\" \"");
    arguments.push(work);
    arguments.push("\"");
    let mut arguments: Vec<u16> = arguments.encode_wide().chain(Some(0)).collect();
    // SAFETY: Win32 accepts zero-initialized structures with cb set, NUL-terminated
    // command lines, and null optional pointers. Handle inheritance is explicitly off:
    // even unrelated inherited caller pipes must not keep the foreground attached.
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
        Ok(Child {
            handle: process.hProcess,
            pid: process.dwProcessId,
        })
    }
}

#[cfg(unix)]
fn spawn(helper: &Path, root: &Path, work: &Path) -> io::Result<std::process::Child> {
    let mut command = Command::new(helper);
    command
        .arg("__upgrade-apply")
        .arg(root)
        .arg(work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and does not touch Rust state after fork.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn()
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    // SAFETY: the process handle is checked and closed exactly once.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return io::Error::last_os_error().raw_os_error() == Some(5);
        }
        let running = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
        CloseHandle(process);
        running
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal zero only checks whether the process exists.
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
