use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

mod upgrade;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(help) = help(&arguments) {
        println!("{help}");
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 3 && arguments[0] == "__upgrade-apply" {
        return match upgrade::apply(Path::new(&arguments[1]), Path::new(&arguments[2])) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail_with_error(error),
        };
    }
    if arguments.len() == 2 && arguments[0] == "__upgrade-render" {
        return match upgrade::render(Path::new(&arguments[1])) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail_with_error(error),
        };
    }
    if matches_command(&arguments, &["--version"]) {
        println!("outrim {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if matches_command(&arguments, &["upgrade"]) || matches_command(&arguments, &["update"]) {
        let result = user_home().and_then(|home| {
            upgrade::run(
                &configured_pi_agent_dir(&home),
                &configured_opencode_config_dir(&home),
            )
        });
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail_with_error(error),
        };
    }
    if matches_command(&arguments, &["bridge"]) {
        return run_bridge(None);
    }
    // Adapters pass their host-startup snapshot, also when restarting a bridge.
    if matches_command(&arguments, &["bridge", "--recording", "on"])
        || matches_command(&arguments, &["bridge", "--recording", "off"])
    {
        return run_bridge(Some(arguments[2] == "on"));
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
            "outrim upgrade (alias: update)\noutrim gain [--detail [--session <session-id>]]\noutrim gain record on|off\nFull local recording is off by default. On authorizes saving complete tool arguments and texts, which may contain source code, paths and secrets. Restart hosts to apply; running instances keep their startup setting. Off does not delete historical data."
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
        "Usage:\n  outrim upgrade (alias: update)\n  outrim bridge\n  outrim gain\n  outrim install --agent pi\n  outrim uninstall --agent pi\n  outrim install --agent opencode\n  outrim uninstall --agent opencode"
    );
    ExitCode::from(2)
}

fn help(arguments: &[std::ffi::OsString]) -> Option<&'static str> {
    let level = if arguments.is_empty() {
        arguments
    } else if arguments.last().is_some_and(|arg| arg == "-h" || arg == "--help") {
        &arguments[..arguments.len() - 1]
    } else {
        return None;
    };
    if level.is_empty() {
        Some("Usage: outrim <command>\n\nCommands:\n  install    Install an agent integration\n  uninstall  Remove an agent integration\n  upgrade    Update outrim and integrations (alias: update)\n  gain       Show compression savings\n  bridge     Run the agent bridge\n\nOptions:\n  -h, --help  Show help\n  --version   Show version")
    } else if matches_command(level, &["install"]) {
        Some("Install an agent integration.\n\nUsage: outrim install --agent <pi|opencode>")
    } else if matches_command(level, &["uninstall"]) {
        Some("Remove an agent integration.\n\nUsage: outrim uninstall --agent <pi|opencode>")
    } else if matches_command(level, &["upgrade"]) || matches_command(level, &["update"]) {
        Some("Update outrim and installed integrations.\n\nUsage: outrim upgrade")
    } else if matches_command(level, &["gain"]) {
        Some("Show compression savings.\n\nUsage:\n  outrim gain\n  outrim gain --detail [--session <id>]\n  outrim gain record <on|off>\n\nOptions:\n  --detail        Show recorded sessions\n  --session <id>  Show session records")
    } else if matches_command(level, &["gain", "record"]) {
        Some("Save tool arguments and full output locally.\n\nUsage: outrim gain record <on|off>")
    } else if matches_command(level, &["bridge"]) {
        Some("Run the agent bridge over stdin/stdout.\n\nUsage: outrim bridge")
    } else {
        None
    }
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

fn run_bridge(recording: Option<bool>) -> ExitCode {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return fail_with_error(error),
    };
    let processor = match recording {
        Some(enabled) => outrim::Processor::with_recording(&home, enabled),
        None => outrim::Processor::new(&home),
    };
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
