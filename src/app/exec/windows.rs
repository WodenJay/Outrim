use std::{
    io,
    mem::{size_of, zeroed},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command},
    sync::atomic::{AtomicI32, Ordering},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT,
            CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
        },
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};

static CANCELLED: AtomicI32 = AtomicI32::new(0);
unsafe extern "system" fn control(event: u32) -> i32 {
    if matches!(
        event,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        CANCELLED.store(2, Ordering::Relaxed);
        1
    } else {
        0
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct Supervisor {
    job: Handle,
}
impl Supervisor {
    pub(super) fn new(command: &mut Command) -> io::Result<Self> {
        // The process cannot run (and spawn escaping children) before job assignment.
        command.creation_flags(CREATE_SUSPENDED);
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Handle(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if SetConsoleCtrlHandler(Some(control), 1) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { job })
        }
    }

    pub(super) fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let mut child = command.spawn()?;
        let result = unsafe {
            if AssignProcessToJobObject(self.job.0, child.as_raw_handle()) == 0 {
                Err(io::Error::last_os_error())
            } else {
                resume_primary_thread(child.id())
            }
        };
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(child)
    }

    pub(super) fn cancelled(&self) -> i32 {
        CANCELLED.load(Ordering::Relaxed)
    }
    pub(super) fn stop(&self, _child: &Child, _signal: i32) {
        unsafe {
            TerminateJobObject(self.job.0, 124);
        }
    }
    pub(super) fn cleanup(&self, child: &Child) {
        self.stop(child, 0);
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        unsafe {
            SetConsoleCtrlHandler(Some(control), 0);
        }
    }
}

/// std keeps Windows argv quoting and executable resolution correct, but does not
/// expose the primary-thread handle. A suspended new process has one thread;
/// locate it by process ID, open it, and resume only after containment succeeds.
unsafe fn resume_primary_thread(process_id: u32) -> io::Result<()> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut entry: THREADENTRY32 = zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut found = Thread32First(snapshot.0, &mut entry);
        while found != 0 {
            if entry.th32OwnerProcessID == process_id {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = Handle(thread);
                if ResumeThread(thread.0) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            found = Thread32Next(snapshot.0, &mut entry);
        }
        Err(io::Error::other(
            "Suspended process primary thread unavailable",
        ))
    }
}
