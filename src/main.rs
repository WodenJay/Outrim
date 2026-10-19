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
    if matches_command(&arguments, &["gain"]) {
        return run_gain(&arguments[1..]);
    }
    if matches_command(&arguments, &["gain", "record", "on"])
        || matches_command(&arguments, &["gain", "record", "off"])
        || matches_command(&arguments, &["gain", "--detail"])
        || (arguments.len() == 4
            && matches_command(&arguments[..3], &["gain", "--detail", "--session"]))
    {
        return run_gain(&arguments[1..]);
    }
    if matches_command(&arguments, &["--help"]) || matches_command(&arguments, &["gain", "--help"])
    {
        println!(
            "outrim gain [--detail [--session <session-id>]]\noutrim gain record on|off\nFull local recording is off by default. On authorizes saving complete tool arguments and texts, which may contain source code, paths and secrets. Restart hosts to apply; running instances keep their startup setting. Off does not delete historical data."
        );
        return ExitCode::SUCCESS;
    }
    if matches_command(&arguments, &["install", "--agent", "pi"]) {
        return install_pi();
    }
    if matches_command(&arguments, &["uninstall", "--agent", "pi"]) {
        return uninstall_pi();
    }
    if matches_command(&arguments, &["install", "--agent", "opencode"]) {
        return install_opencode();
    }
    if matches_command(&arguments, &["uninstall", "--agent", "opencode"]) {
        return uninstall_opencode();
    }
    eprintln!(
        "Usage:\n  outrim bridge\n  outrim gain\n  outrim install --agent pi\n  outrim uninstall --agent pi\n  outrim install --agent opencode\n  outrim uninstall --agent opencode"
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

fn configured_opencode_config_dir(user_home: &Path) -> PathBuf {
    let configured_home = std::env::var_os("XDG_CONFIG_HOME");
    outrim::install::opencode_config_dir(user_home, configured_home.as_deref())
}

fn install_opencode() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let config_dir = configured_opencode_config_dir(&home);
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return fail_with_error(error),
    };
    match outrim::install::install_opencode_plugin(&config_dir, &executable) {
        Ok(path) => {
            println!(
                "OpenCode integration is ready at {}. Restart OpenCode to activate.",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => fail_with_error(error),
    }
}

fn uninstall_opencode() -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let config_dir = configured_opencode_config_dir(&home);
    match outrim::install::uninstall_opencode_plugin(&config_dir) {
        Ok(Some(path)) => {
            println!(
                "Removed OpenCode integration at {}. Restart OpenCode to unload.",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("OpenCode integration is not installed.");
            ExitCode::SUCCESS
        }
        Err(error) => fail_with_error(error),
    }
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

fn run_gain(arguments: &[std::ffi::OsString]) -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let report = if arguments.first().is_some_and(|arg| arg == "record") {
        outrim::gain::set_recording(&home, arguments[1] == "on")
    } else if arguments.first().is_some_and(|arg| arg == "--detail") {
        outrim::gain::render_detail(&home, arguments.get(2).and_then(|s| s.to_str()))
    } else {
        outrim::gain::render(&home)
    };
    match report {
        Ok(report) => {
            print!("{report}");
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
