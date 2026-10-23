use outrim::install::Agent;
use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

mod cli;
mod exec;
mod upgrade;

pub(crate) fn run() -> ExitCode {
    outrim::bookkeeping::enable_resident();
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 3 && arguments[0] == "__bookkeeping" {
        return finish(outrim::bookkeeping::run(
            Path::new(&arguments[1]),
            &arguments[2].to_string_lossy(),
        ));
    }
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
        cli::Command::Upgrade => finish(user_home().and_then(|home| upgrade::run(&home))),
        cli::Command::Gain(operation) => finish(run_gain(operation)),
        cli::Command::Bridge(recording) => finish(run_bridge(recording)),
        cli::Command::ObserveFallback(context) => {
            if let Ok(home) = user_home() {
                outrim::execution::Execution::new(&home, &[], context).record_fallback();
            }
            std::process::ExitCode::SUCCESS
        }
        cli::Command::Exec {
            no_compress,
            context,
            timeout,
            arguments,
        } => match exec::run(&arguments, no_compress, timeout, context) {
            Ok(status) => exec::exit(status),
            Err(error) => finish(Err(error)),
        },
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

fn configured_agent_dir(agent: Agent, user_home: &Path) -> PathBuf {
    let configured = std::env::var_os(agent.config_variable());
    agent.directory(user_home, configured.as_deref())
}

fn run_install(agent: Agent, remove: bool) -> io::Result<()> {
    let home = user_home()?;
    let name = agent.name();
    let directory = configured_agent_dir(agent, &home);
    if remove {
        let path = agent.uninstall(&directory)?;
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
        let path = agent.install(&directory, &executable)?;
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
        cli::Gain::Clear => {
            if !io::stdin().is_terminal() {
                return Err(io::Error::other(
                    "gain clear requires an interactive terminal",
                ));
            }
            if confirm_gain_clear(&home, io::stdin().lock(), io::stderr().lock())? {
                outrim::gain::clear(&home)?;
                Ok("Savings and diagnostic records cleared.\n".into())
            } else {
                Ok("Cancelled.\n".into())
            }
        }
    }?;
    print!("{report}");
    Ok(())
}

fn confirm_gain_clear(
    home: &Path,
    mut input: impl BufRead,
    mut output: impl Write,
) -> io::Result<bool> {
    writeln!(output, "Exit hosts using Outrim before clearing.")?;
    writeln!(output, "Delete cumulative savings and diagnostic records:")?;
    writeln!(output, "  {}", home.join(".outrim/gain.json").display())?;
    writeln!(output, "  {}", home.join(".outrim/records").display())?;
    writeln!(output, "Keep recovery originals and recording settings.")?;
    write!(output, "Type yes to clear: ")?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(answer.trim_end_matches(['\r', '\n']) == "yes")
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

#[cfg(test)]
mod tests {
    use super::confirm_gain_clear;
    use std::{io::Cursor, path::Path};

    #[test]
    fn clear_requires_explicit_yes() {
        for (answer, expected) in [
            ("yes\n", true),
            ("yes\r\n", true),
            ("\n", false),
            ("", false),
            ("y\n", false),
            ("YES\n", false),
            ("no\n", false),
        ] {
            assert_eq!(
                confirm_gain_clear(Path::new("home"), Cursor::new(answer), Vec::new()).unwrap(),
                expected
            );
        }
    }
}
