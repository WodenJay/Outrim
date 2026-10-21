use std::{
    io,
    mem::zeroed,
    os::unix::process::CommandExt,
    process::{Child, Command},
    sync::atomic::{AtomicI32, Ordering},
};

static CANCELLED: AtomicI32 = AtomicI32::new(0);
extern "C" fn signal(signal: i32) {
    CANCELLED.store(signal, Ordering::Relaxed);
}

pub(super) struct Supervisor {
    actions: Vec<(i32, libc::sigaction)>,
    foreground: Option<libc::pid_t>,
}

impl Supervisor {
    pub(super) fn new(command: &mut Command) -> io::Result<Self> {
        let mut supervisor = Self {
            actions: Vec::new(),
            foreground: None,
        };
        unsafe {
            for number in [libc::SIGINT, libc::SIGTERM, libc::SIGTTOU] {
                let mut action: libc::sigaction = zeroed();
                action.sa_sigaction = if number == libc::SIGTTOU {
                    libc::SIG_IGN
                } else {
                    signal as *const () as usize
                };
                libc::sigemptyset(&mut action.sa_mask);
                let mut previous = zeroed();
                if libc::sigaction(number, &action, &mut previous) != 0 {
                    return Err(io::Error::last_os_error());
                }
                supervisor.actions.push((number, previous));
            }
            let group = libc::tcgetpgrp(libc::STDIN_FILENO);
            let handoff = group > 0 && group == libc::getpgrp();
            if handoff {
                supervisor.foreground = Some(group);
            }
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // A separate process group must own terminal stdin, not receive SIGTTIN.
                if handoff && libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
                Ok(())
            });
        }
        Ok(supervisor)
    }
    pub(super) fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        command.spawn()
    }
    pub(super) fn cancelled(&self) -> i32 {
        CANCELLED.load(Ordering::Relaxed)
    }
    pub(super) fn stop(&self, child: &Child, signal: i32) {
        unsafe {
            libc::kill(-(child.id() as i32), signal);
        }
    }
    pub(super) fn cleanup(&self, child: &Child) {
        self.stop(child, libc::SIGKILL);
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        unsafe {
            if let Some(group) = self.foreground {
                libc::tcsetpgrp(libc::STDIN_FILENO, group);
            }
            for (number, previous) in self.actions.iter().rev() {
                libc::sigaction(*number, previous, std::ptr::null_mut());
            }
        }
    }
}
