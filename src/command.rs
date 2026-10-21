//! Classification of an already separated argv; never parse or execute shell text.
use crate::protocol::Source;
use std::ffi::OsString;

pub fn source(arguments: &[OsString]) -> Source {
    let words: Vec<_> = arguments
        .iter()
        .map(|arg| arg.to_str().unwrap_or(""))
        .collect();
    let Some(program) = words.first() else {
        return Source::Unknown;
    };
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let program = program
        .strip_suffix(".exe")
        .unwrap_or(program)
        .to_ascii_lowercase();
    let rest = &words[1..];
    let first = rest.first().copied().unwrap_or("");
    match program.as_str() {
        "rg" | "grep" | "find" | "fd" | "ls" => Source::Search,
        "git" if first == "grep" || rest == ["ls-files"] => Source::Search,
        "cargo" => match first {
            "test" => Source::TestLog,
            "build" | "check" | "clippy" => Source::BuildLog,
            "run" => Source::RuntimeLog,
            _ => Source::Unknown,
        },
        "bun" | "npm" | "pnpm" | "yarn" => match rest {
            ["test", ..] | ["run", "test", ..] => Source::TestLog,
            ["build", ..] | ["run", "build", ..] => Source::BuildLog,
            ["start", ..] | ["run", "dev" | "start", ..] => Source::RuntimeLog,
            _ => Source::Unknown,
        },
        "pytest" | "jest" | "vitest" => Source::TestLog,
        "go" if first == "test" => Source::TestLog,
        "go" if first == "build" => Source::BuildLog,
        "tsc" | "make" => Source::BuildLog,
        "vite" if first == "build" => Source::BuildLog,
        "cmake" if first == "--build" => Source::BuildLog,
        "gh" if rest.starts_with(&["run", "watch"]) => Source::RuntimeLog,
        _ => Source::Unknown,
    }
}

/// Interpreter names alone cannot establish the source of their output.
pub fn uncertain_shell(arguments: &[OsString]) -> bool {
    arguments.first().is_some_and(|program| {
        let program = program.to_string_lossy();
        let name = program.rsplit(['/', '\\']).next().unwrap_or(&program);
        matches!(
            name.trim_end_matches(".exe").to_ascii_lowercase().as_str(),
            "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh" | "cmd" | "pwsh" | "powershell"
        )
    })
}
