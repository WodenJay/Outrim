//! Conservative flat-list rewriting. Resolution remains in the actual shell.
mod segments;
use crate::{
    Processor, command,
    execution::Context,
    protocol::{
        Diagnostics, Host, ObservationMessageType, ObservationOutcome, RecordObservationRequest,
        Source, Tool, VERSION,
    },
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
    #[serde(default)]
    pub tool: Option<Tool>,
}

#[derive(Serialize)]
pub struct Response {
    pub version: u32,
    pub id: String,
    pub command: Option<String>,
    pub reason: Option<String>,
    pub read: bool,
}

pub fn rewrite(request: Request, executable: &Path, processor: &Processor) -> Response {
    let command = replacement(&request, executable, processor);
    // MSYS Bash silently truncates native Windows argv at 8191 bytes. Leave
    // headroom for shell options rather than risk executing a truncated rewrite.
    if cfg!(windows)
        && request.dialect == "bash"
        && command.as_ref().is_some_and(|command| command.len() > 8000)
    {
        record_plan(
            processor,
            &request,
            None,
            false,
            Some("rewritten_command_limit"),
        );
        return Response {
            version: VERSION,
            read: false,
            id: request.id,
            command: None,
            reason: Some("rewritten_command_limit".into()),
        };
    }
    Response {
        version: VERSION,
        read: processor.has_shell_read(request.host, &request.session, &request.tool_call_id),
        id: request.id,
        reason: command.is_none().then(|| {
            if !matches!(request.dialect.as_str(), "bash" | "powershell") {
                "unknown_dialect"
            } else if segments::segments(&request.command, request.dialect == "powershell")
                .is_none()
            {
                "unsupported_syntax"
            } else {
                "unsupported_command"
            }
            .into()
        }),
        command,
    }
}

fn replacement(request: &Request, executable: &Path, processor: &Processor) -> Option<String> {
    if request.version != VERSION || request.kind != "rewrite" {
        return None;
    }
    let powershell = match request.dialect.as_str() {
        "bash" => false,
        "powershell" => true,
        _ => {
            record_plan(processor, request, None, false, Some("unknown_dialect"));
            return None;
        }
    };
    let Some(segments) = segments::segments(&request.command, powershell) else {
        record_plan(processor, request, None, false, Some("unsupported_syntax"));
        return None;
    };
    let compound = segments.len() > 1;
    // Only a whole, single read command can be correlated with the host result.
    let read = (!compound)
        .then(|| literals(&request.command, powershell))
        .flatten()
        .and_then(|words| crate::processing::reads::shell_selection(&words));
    let mut output = String::new();
    let mut end = 0;
    let mut changed = false;
    for (index, segment) in segments.into_iter().enumerate() {
        let separator = &request.command[end..segment.range.start];
        // PowerShell chain operands must be pipelines, not `if` statements. Flat
        // chains are left-associative; `if` preserves $? when its branch is skipped.
        let conditional = powershell && matches!(separator, "&&" | "||");
        if conditional {
            output.push_str(if separator == "&&" {
                "; if ($?) { "
            } else {
                "; if (-not $?) { "
            });
        } else {
            output.push_str(separator);
        }
        let original = &request.command[segment.range.clone()];
        let unit_id = if compound {
            format!("{}:{}", request.unit_id, index + 1)
        } else {
            request.unit_id.clone()
        };
        if let Some(replacement) = segment
            .host_output
            .then(|| {
                simple(
                    request,
                    original,
                    &unit_id,
                    executable,
                    powershell,
                    processor,
                    read.clone(),
                )
            })
            .flatten()
        {
            record_plan(processor, request, Some(&unit_id), true, None);
            output.push_str(&replacement);
            changed = true;
        } else {
            record_plan(
                processor,
                request,
                Some(&unit_id),
                false,
                Some(
                    if !segment.host_output || literals(original, powershell).is_none() {
                        "unsupported_syntax"
                    } else {
                        "unsupported_command"
                    },
                ),
            );
            output.push_str(original);
        }
        if conditional {
            output.push_str(" }");
        }
        end = segment.range.end;
    }
    output.push_str(&request.command[end..]);
    changed.then_some(output)
}

fn record_plan(
    processor: &Processor,
    request: &Request,
    unit: Option<&str>,
    planned: bool,
    reason: Option<&str>,
) {
    processor.record_observation(RecordObservationRequest {
        message_type: ObservationMessageType::RecordObservation,
        version: VERSION,
        id: request.id.clone(),
        host: request.host,
        session: Some(request.session.clone()),
        tool_call_id: Some(request.tool_call_id.clone()),
        tool: Tool {
            name: "shell".into(),
            input: serde_json::Value::Null,
            source: Source::Unknown,
            exit_code: None,
        },
        original_text: None,
        final_text: None,
        metadata_only: true,
        diagnostics: Some(Diagnostics {
            unit_id: unit.map(str::to_owned),
            wrapping: Some(if planned { "planned" } else { "rejected" }.into()),
            requested_dialect: Some(request.dialect.clone()),
            attempt_id: Some(
                serde_json::json!([request.host, request.session, request.tool_call_id])
                    .to_string(),
            ),
            ..Default::default()
        }),
        outcome: ObservationOutcome::Unsupported,
        reason: reason.map(str::to_owned),
        applied_rule: None,
        original_path: None,
        candidates: vec![],
        parts: vec![],
        execution: None,
    });
}

fn simple(
    request: &Request,
    original: &str,
    unit_id: &str,
    executable: &Path,
    powershell: bool,
    processor: &Processor,
    read: Option<crate::processing::reads::Selection>,
) -> Option<String> {
    let words = literals(original.trim(), powershell)?;
    let program = words.first()?;
    let name = command::program_name(program);
    // Shells, builtins and interactive editors cannot be captured safely.
    // Interpreters with arguments may opt in below, but gain no builtin source.
    if matches!(
        name.as_str(),
        "python" | "python3" | "node" | "deno" | "ruby" | "perl" | "lua"
    ) && words.len() == 1
    {
        return None;
    }
    if matches!(
        name.as_str(),
        "outrim"
            | "bash"
            | "sh"
            | "zsh"
            | "fish"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "irb"
            | "vim"
            | "nvim"
            | "vi"
            | "emacs"
            | "nano"
            | "less"
            | "more"
            | "cd"
            | "echo"
            | "printf"
            | "read"
            | "exec"
            | "source"
            | "get-content"
    ) {
        return None;
    }
    let builtin_program = matches!(
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
            | "sed"
    );
    let argv: Vec<_> = words.iter().map(OsString::from).collect();
    let source = command::source(&argv);
    let builtin_capture = read.is_some()
        || (builtin_program
            && (!matches!(source, Source::Unknown)
                || matches!(name.as_str(), "git" | "jq" | "cat" | "head")));
    if words.iter().any(|word| {
        let flag = word.split('=').next().unwrap_or(word);
        matches!(
            flag,
            "--interactive"
                | "--patch"
                | "--watch"
                | "--watchAll"
                | "--follow"
                | "--pdb"
                | "--trace"
                | "--ui"
                | "--inspect"
                | "--inspect-brk"
        ) || (flag.starts_with('-')
            && !flag.starts_with("--")
            && flag[1..].contains(['i', 'p', 'w', 'f', 'F']))
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
        unit_id: unit_id.to_owned(),
        source: Some(source),
        recording: Some(request.recording),
        tool: request.tool.clone().map(|mut tool| {
            tool.source = source;
            tool
        }),
        command: Some(words.clone()),
        configured_only: !builtin_capture,
        read_receipt: read.and_then(|selection| {
            processor.prepare_shell_read(
                request.host,
                &request.session,
                &request.tool_call_id,
                selection,
            )
        }),
    };
    let quote = |word: &str| quote(word, powershell);
    let encoded = quote(&context.encode());
    // Fallback observations carry no input and never process output.
    let fallback_context = Context {
        tool: None,
        command: None,
        ..context
    };
    let fallback_context = quote(&fallback_context.encode());
    let context = encoded;
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
        // Process.Start avoids changing the shell's LASTEXITCODE; all observer
        // output is redirected and failures remain local to the guard.
        let observer = if request.recording {
            format!(
                "& {{ try {{ $i=[System.Diagnostics.ProcessStartInfo]::new(); $i.FileName={executable}; $i.Arguments='observe-fallback ' + {fallback_context}; $i.UseShellExecute=$false; $i.RedirectStandardOutput=$true; $i.RedirectStandardError=$true; $i.CreateNoWindow=$true; $p=[System.Diagnostics.Process]::Start($i); $p.WaitForExit(); $p.Dispose() }} catch {{}} }}; "
            )
        } else {
            String::new()
        };
        Some(format!(
            "if (({resolve}).CommandType -eq 'Application'{platform_guard}) {{ & {executable} exec --context {context} -- ({resolve}).Source {args} }} else {{ {observer}{original} }}"
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
        let observer = if request.recording {
            format!("({executable} observe-fallback {fallback_context}) >/dev/null 2>&1 || :; ")
        } else {
            String::new()
        };
        Some(format!(
            "if [[ $(builtin type -t -- {target}) == file{platform_guard} ]]; then {executable} exec --context {context} -- \"$(builtin command -v -- {target})\" {args}; else {observer}{original}; fi"
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
                // PowerShell doubled quotes vary with native argument-passing mode.
                if powershell && chars.peek() == Some(&'"') {
                    return None;
                }
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
