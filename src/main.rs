use std::{
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

mod cli;
mod upgrade;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 3 && arguments[0] == "__upgrade-apply" {
        return finish(upgrade::apply(
            Path::new(&arguments[1]),
            Path::new(&arguments[2]),
        ));
    }
    if arguments.len() == 2 && arguments[0] == "__upgrade-render" {
        return finish(upgrade::render(Path::new(&arguments[1])));
    }
    let command = match cli::parse(&arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    match command {
        cli::Command::Help(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        cli::Command::Version => {
            println!("outrim {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        cli::Command::Install { agent, remove } => finish(run_install(agent, remove)),
        cli::Command::Upgrade => finish(user_home().and_then(|home| {
            upgrade::run(
                &configured_pi_agent_dir(&home),
                &configured_opencode_config_dir(&home),
            )
        })),
        cli::Command::Gain(operation) => finish(run_gain(operation)),
        cli::Command::Bridge(recording) => finish(run_bridge(recording)),
    }
}

fn user_home() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let variable = "USERPROFILE";
    #[cfg(not(windows))]
    let variable = "HOME";
    std::env::var_os(variable)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Home directory unavailable"))
}

fn configured_pi_agent_dir(user_home: &Path) -> PathBuf {
    let configured_dir = std::env::var_os("PI_CODING_AGENT_DIR");
    outrim::install::pi_agent_dir(user_home, configured_dir.as_deref())
}

fn configured_opencode_config_dir(user_home: &Path) -> PathBuf {
    let configured_home = std::env::var_os("XDG_CONFIG_HOME");
    outrim::install::opencode_config_dir(user_home, configured_home.as_deref())
}

fn run_install(agent: cli::Agent, remove: bool) -> io::Result<()> {
    let home = user_home()?;
    let (name, directory) = match agent {
        cli::Agent::Pi => ("Pi", configured_pi_agent_dir(&home)),
        cli::Agent::OpenCode => ("OpenCode", configured_opencode_config_dir(&home)),
    };
    if remove {
        let path = match agent {
            cli::Agent::Pi => outrim::install::uninstall_pi_extension(&directory)?,
            cli::Agent::OpenCode => outrim::install::uninstall_opencode_plugin(&directory)?,
        };
        if let Some(path) = path {
            println!(
                "Removed {name} integration: {}\nRestart {name}.",
                path.display()
            );
        } else {
            println!("{name} integration not installed.");
        }
    } else {
        let executable = std::env::current_exe()?;
        let path = match agent {
            cli::Agent::Pi => outrim::install::install_pi_extension(&directory, &executable)?,
            cli::Agent::OpenCode => {
                outrim::install::install_opencode_plugin(&directory, &executable)?
            }
        };
        println!(
            "Installed {name} integration: {}\nRestart {name}.",
            path.display()
        );
    }
    Ok(())
}

fn run_gain(operation: cli::Gain) -> io::Result<()> {
    let home = user_home()?;
    let report = match operation {
        cli::Gain::Summary => outrim::gain::render(&home),
        cli::Gain::Detail(session) => outrim::gain::render_detail(&home, session.as_deref()),
        cli::Gain::Record(enabled) => outrim::gain::set_recording(&home, enabled),
    }?;
    print!("{report}");
    Ok(())
}

fn run_bridge(recording: Option<bool>) -> io::Result<()> {
    let home = user_home()?;
    let processor = match recording {
        Some(enabled) => outrim::Processor::with_recording(&home, enabled),
        None => outrim::Processor::new(&home),
    };
    outrim::bridge::run(
        &processor,
        io::stdin().lock(),
        io::stdout().lock(),
        io::stderr().lock(),
    )
}

fn finish(result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("outrim: {error}");
            ExitCode::FAILURE
        }
    }
}
