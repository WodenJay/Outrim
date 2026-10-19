use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if matches_command(&arguments, &["bridge"]) {
        return run_bridge();
    }
    if matches_command(&arguments, &["install", "--agent", "pi"]) {
        return install_pi();
    }
    if matches_command(&arguments, &["uninstall", "--agent", "pi"]) {
        return uninstall_pi();
    }
    eprintln!(
        "Usage:\n  outrim bridge\n  outrim install --agent pi\n  outrim uninstall --agent pi"
    );
    ExitCode::from(2)
}

fn matches_command(arguments: &[std::ffi::OsString], expected: &[&str]) -> bool {
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

fn configured_pi_agent_dir(user_home: &Path) -> PathBuf {
    let configured_dir = std::env::var_os("PI_CODING_AGENT_DIR");
    outrim::install::pi_agent_dir(user_home, configured_dir.as_deref())
}

fn install_pi() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let agent_dir = configured_pi_agent_dir(&home);
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fail_with_error(error),
    };
    match outrim::install::install_pi_extension(&agent_dir, &executable) {
        Ok(path) => {
            println!(
                "Pi integration is ready at {}. Restart Pi to activate.",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => fail_with_error(error),
    }
}

fn uninstall_pi() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let agent_dir = configured_pi_agent_dir(&home);
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
        Err(error) => fail_with_error(error),
    }
}

fn run_bridge() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let processor = outrim::Processor::new(&home);
    match outrim::bridge::run(
        &processor,
        io::stdin().lock(),
        io::stdout().lock(),
        io::stderr().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail_with_error(error),
    }
}

fn fail_with_error(error: io::Error) -> ExitCode {
    eprintln!("outrim: {error}");
    ExitCode::FAILURE
}
