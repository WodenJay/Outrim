use std::{ffi::OsStr, io, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if matches(&arguments, &["bridge"]) {
        return run_bridge();
    }
    if matches(&arguments, &["install", "--agent", "pi"]) {
        return install_pi();
    }
    if matches(&arguments, &["uninstall", "--agent", "pi"]) {
        return uninstall_pi();
    }
    eprintln!(
        "Usage:\n  outrim bridge\n  outrim install --agent pi\n  outrim uninstall --agent pi"
    );
    ExitCode::from(2)
}

fn matches(arguments: &[std::ffi::OsString], expected: &[&str]) -> bool {
    arguments.len() == expected.len()
        && arguments
            .iter()
            .zip(expected)
            .all(|(argument, expected)| argument.as_os_str() == OsStr::new(expected))
}

fn user_home() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let variable = "USERPROFILE";
    #[cfg(not(windows))]
    let variable = "HOME";
    std::env::var_os(variable)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "user home is unavailable"))
}

fn install_pi() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return failed(error),
    };
    let configured_dir = std::env::var_os("PI_CODING_AGENT_DIR");
    let agent_dir = outrim::install::pi_agent_dir(&home, configured_dir.as_deref());
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return failed(error),
    };
    match outrim::install::install_pi_extension(&agent_dir, &executable) {
        Ok(path) => {
            println!(
                "Pi integration is ready at {}. Restart Pi to activate.",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => failed(error),
    }
}

fn uninstall_pi() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return failed(error),
    };
    let configured_dir = std::env::var_os("PI_CODING_AGENT_DIR");
    let agent_dir = outrim::install::pi_agent_dir(&home, configured_dir.as_deref());
    match outrim::install::uninstall_pi_extension(&agent_dir) {
        Ok(Some(path)) => {
            println!(
                "Removed Pi integration at {}. Restart Pi to unload.",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("Pi integration is not installed.");
            ExitCode::SUCCESS
        }
        Err(error) => failed(error),
    }
}

fn run_bridge() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return failed(error),
    };
    let processor = outrim::Processor::new(&home);
    match outrim::bridge::run(
        &processor,
        io::stdin().lock(),
        io::stdout().lock(),
        io::stderr().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => failed(error),
    }
}

fn failed(error: io::Error) -> ExitCode {
    eprintln!("outrim: {error}");
    ExitCode::FAILURE
}
