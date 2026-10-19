use std::{io, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 || arguments[0] != "bridge" {
        eprintln!("Usage: outrim bridge");
        return ExitCode::from(2);
    }
    #[cfg(windows)]
    let home_variable = "USERPROFILE";
    #[cfg(not(windows))]
    let home_variable = "HOME";
    let Some(home) = std::env::var_os(home_variable).filter(|home| !home.is_empty()) else {
        eprintln!("outrim: user home is unavailable");
        return ExitCode::FAILURE;
    };
    let processor = outrim::Processor::new(&PathBuf::from(home));
    match outrim::bridge::run(
        &processor,
        io::stdin().lock(),
        io::stdout().lock(),
        io::stderr().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("outrim: {error}");
            ExitCode::FAILURE
        }
    }
}
