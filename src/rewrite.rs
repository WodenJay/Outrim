//! Conservative literal simple-command rewriting. Resolution remains in the actual shell.
use crate::{
    command,
    execution::Context,
    protocol::{Host, Source, VERSION},
};
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u32,
    pub id: String,
    pub host: Host,
    pub session: String,
    pub tool_call_id: String,
    pub unit_id: String,
    pub recording: bool,
    pub dialect: String,
    pub command: String,
}

#[derive(Serialize)]
pub struct Response {
    pub version: u32,
    pub id: String,
    pub command: Option<String>,
}

pub fn rewrite(request: Request, executable: &Path) -> Response {
    let command = replacement(&request, executable);
    Response {
        version: VERSION,
        id: request.id,
        command,
    }
}

fn replacement(request: &Request, executable: &Path) -> Option<String> {
    if request.version != VERSION || request.kind != "rewrite" {
        return None;
    }
    let powershell = match request.dialect.as_str() {
        "bash" => false,
        "powershell" => true,
        _ => return None,
    };
    let words = literals(&request.command, powershell)?;
    let program = words.first()?;
    let name = command::program_name(program);
    // Bound capture eligibility as well as grammar: no shells, launchers, REPLs or editors.
    if !matches!(
        name.as_str(),
        "cargo"
            | "bun"
            | "npm"
            | "pnpm"
            | "yarn"
            | "pytest"
            | "jest"
            | "vitest"
            | "go"
            | "tsc"
            | "make"
            | "vite"
            | "cmake"
            | "gh"
            | "rg"
            | "grep"
            | "find"
            | "fd"
            | "ls"
            | "git"
            | "jq"
            | "cat"
            | "head"
            | "tail"
    ) {
        return None;
    }
    let argv: Vec<_> = words.iter().map(OsString::from).collect();
    let source = command::source(&argv);
    if matches!(source, Source::Unknown)
        && !matches!(name.as_str(), "git" | "jq" | "cat" | "head" | "tail")
    {
        return None;
    }
    if words.iter().any(|word| {
        matches!(
            word.as_str(),
            "-i" | "--interactive" | "-p" | "--patch" | "--watch" | "-w" | "-f" | "--follow"
        )
    }) {
        return None;
    }
    if name == "git"
        && !words.get(1).is_some_and(|word| {
            matches!(
                word.as_str(),
                "grep" | "ls-files" | "status" | "diff" | "show" | "log"
            )
        })
    {
        return None;
    }
    if name == "jq"
        && words.iter().any(|word| {
            matches!(
                word.as_str(),
                "--raw-output" | "-r" | "--join-output" | "-j"
            )
        })
    {
        return None;
    }
    let context = Context {
        host: request.host,
        session: Some(request.session.clone()),
        tool_call_id: Some(request.tool_call_id.clone()),
        unit_id: request.unit_id.clone(),
        source: Some(source),
        recording: Some(request.recording),
    };
    let quote = |word: &str| quote(word, powershell);
    let context = quote(&context.encode());
    let executable = executable.to_str()?;
    let executable = quote(&if powershell {
        executable.to_owned()
    } else {
        executable.replace('\\', "/")
    });
    if powershell && program.contains(['*', '?', '[', ']']) {
        return None;
    }
    let target = quote(program);
    let args = words[1..]
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    if powershell {
        // Qualified resolution does not mistake a same-named function/alias/cmdlet for an application.
        let resolve = format!(
            "Microsoft.PowerShell.Core\\Get-Command -Name {target} -ErrorAction SilentlyContinue"
        );
        let platform_guard = if cfg!(windows) {
            format!(" -and ({resolve}).Source.EndsWith('.exe')")
        } else {
            String::new()
        };
        Some(format!(
            "if (({resolve}).CommandType -eq 'Application'{platform_guard}) {{ & {executable} exec --context {context} -- ({resolve}).Source {args} }} else {{ {} }}",
            request.command
        ))
    } else {
        // Resolve again only after proving this is a file, not a builtin, alias or function.
        let platform_guard = if cfg!(windows) {
            // MSYS often omits .exe from command -v. File identity distinguishes
            // that spelling from a script shadowing a same-named .exe.
            format!(
                " && ( $(builtin command -v -- {target}) == *.exe || $(builtin command -v -- {target}) -ef $(builtin command -v -- {target}).exe )"
            )
        } else {
            String::new()
        };
        Some(format!(
            "if [[ $(builtin type -t -- {target}) == file{platform_guard} ]]; then {executable} exec --context {context} -- \"$(builtin command -v -- {target})\" {args}; else {}; fi",
            request.command
        ))
    }
}

fn quote(word: &str, powershell: bool) -> String {
    format!(
        "'{}'",
        word.replace('\'', if powershell { "''" } else { "'\\''" })
    )
}

// Only literal words are accepted; expansions, globbing, redirection and composition fail open.
fn literals(command: &str, powershell: bool) -> Option<Vec<String>> {
    let command = command.trim_start_matches([' ', '\t']);
    let called = powershell && command.starts_with("& ");
    let command = if called { &command[2..] } else { command };
    let mut chars = command.chars().peekable();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    while let Some(c) = chars.next() {
        if c == '\0' || c == '\r' || c == '\n' || matches!(c, '‘' | '’' | '“' | '”') {
            return None;
        }
        if quote == Some('\'') {
            if c == '\'' {
                if powershell && chars.peek() == Some(&'\'') {
                    chars.next();
                    word.push('\'');
                } else {
                    quote = None;
                }
            } else {
                word.push(c);
            }
        } else if (c == '\\' && !powershell) || (c == '`' && powershell) {
            let next = chars.next()?;
            if next == '\n' || next == '\r' || next == '\0' {
                return None;
            }
            if powershell && next.is_alphanumeric() {
                return None;
            }
            if quote == Some('"') && !powershell && !matches!(next, '$' | '`' | '"' | '\\') {
                word.push('\\');
            }
            word.push(next);
            started = true;
        } else if quote == Some('"') {
            if c == '"' {
                quote = None;
            } else if c == '$' || (!powershell && c == '`') {
                return None;
            } else {
                word.push(c);
            }
        } else if c == '\'' || c == '"' {
            // Quoted PowerShell command heads execute only with the call operator.
            if powershell && !called && words.is_empty() {
                return None;
            }
            quote = Some(c);
            started = true;
        } else if c == ' ' || c == '\t' {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else if "|&;<>()[]{}#@$`*?!~,%^".contains(c) || c.is_whitespace() {
            return None;
        } else {
            word.push(c);
            started = true;
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    Some(words)
}
