use std::ffi::{OsStr, OsString};

pub(crate) enum Command {
    Help(&'static str),
    Version,
    Install { agent: Agent, remove: bool },
    Upgrade,
    Gain(Gain),
    Bridge(Option<bool>),
}

pub(crate) enum Gain {
    Summary,
    Detail(Option<String>),
    Record(bool),
}

pub(crate) enum Agent {
    Pi,
    OpenCode,
}

const TOP: &str = "Usage: outrim <command>";
const INSTALL: &str = "Usage: outrim install --agent <pi|opencode>";
const UNINSTALL: &str = "Usage: outrim uninstall --agent <pi|opencode>";
const UPGRADE: &str = "Usage: outrim upgrade";
const GAIN: &str =
    "Usage:\n  outrim gain\n  outrim gain --detail [--session <id>]\n  outrim gain record <on|off>";
const DETAIL: &str = "Usage: outrim gain --detail [--session <id>]";
const RECORD: &str = "Usage: outrim gain record <on|off>";
const BRIDGE: &str = "Usage: outrim bridge";
const RECORDING: &str = "Usage: outrim bridge [--recording <on|off>]";

pub(crate) fn parse(args: &[OsString]) -> Result<Command, String> {
    if let Some(text) = help(args) {
        return Ok(Command::Help(text));
    }
    let command = &args[0];
    let rest = &args[1..];
    match command.to_str() {
        Some("--version") => {
            no_extra(rest, TOP)?;
            Ok(Command::Version)
        }
        Some("install" | "uninstall") => {
            let remove = command == "uninstall";
            let usage = if remove { UNINSTALL } else { INSTALL };
            let Some(option) = rest.first() else {
                return Err(error("missing --agent", usage));
            };
            if option != "--agent" {
                return Err(unexpected(option, usage));
            }
            let value = value(rest.get(1), "--agent", usage)?;
            let agent = match value.to_str() {
                Some("pi") => Agent::Pi,
                Some("opencode") => Agent::OpenCode,
                _ => {
                    return Err(error(
                        format!(
                            "invalid agent '{}'; expected pi or opencode",
                            value.to_string_lossy()
                        ),
                        usage,
                    ));
                }
            };
            no_extra(&rest[2..], usage)?;
            Ok(Command::Install { agent, remove })
        }
        Some("upgrade" | "update") => {
            no_extra(rest, UPGRADE)?;
            Ok(Command::Upgrade)
        }
        Some("gain") => {
            let operation = match rest.first().and_then(|arg| arg.to_str()) {
                None if rest.is_empty() => Gain::Summary,
                Some("record") => {
                    let Some(setting) = rest.get(1) else {
                        return Err(error("expected on or off", RECORD));
                    };
                    let enabled = recording(setting, RECORD)?;
                    no_extra(&rest[2..], RECORD)?;
                    Gain::Record(enabled)
                }
                Some("--detail") => {
                    if let Some(option) = rest.get(1) {
                        if option != "--session" {
                            return Err(unexpected(option, DETAIL));
                        }
                        let session = value(rest.get(2), "--session", DETAIL)?;
                        let session = session
                            .to_str()
                            .ok_or_else(|| unexpected(session, DETAIL))?;
                        no_extra(&rest[3..], DETAIL)?;
                        Gain::Detail(Some(session.to_owned()))
                    } else {
                        Gain::Detail(None)
                    }
                }
                Some("--session") => {
                    value(rest.get(1), "--session", DETAIL)?;
                    return Err(error("--session requires --detail", DETAIL));
                }
                _ => {
                    let token = &rest[0];
                    return Err(if is_option(token) {
                        unexpected(token, GAIN)
                    } else {
                        error(
                            format!("unknown command '{}'", token.to_string_lossy()),
                            RECORD,
                        )
                    });
                }
            };
            Ok(Command::Gain(operation))
        }
        Some("bridge") => {
            if rest.first().is_some_and(|arg| arg == "--recording") {
                let setting = value(rest.get(1), "--recording", RECORDING)?;
                let enabled = recording(setting, RECORDING)?;
                no_extra(&rest[2..], RECORDING)?;
                Ok(Command::Bridge(Some(enabled)))
            } else {
                no_extra(rest, BRIDGE)?;
                Ok(Command::Bridge(None))
            }
        }
        _ => Err(if is_option(command) {
            unexpected(command, TOP)
        } else {
            error(
                format!("unknown command '{}'", command.to_string_lossy()),
                TOP,
            )
        }),
    }
}

fn error(message: impl std::fmt::Display, usage: &str) -> String {
    format!("error: {message}\n{usage}")
}

fn is_option(token: &OsStr) -> bool {
    token.to_string_lossy().starts_with('-')
}

fn unexpected(token: &OsStr, usage: &str) -> String {
    error(
        format!(
            "{} '{}'",
            if is_option(token) {
                "unknown option"
            } else {
                "unexpected argument"
            },
            token.to_string_lossy()
        ),
        usage,
    )
}

fn no_extra(args: &[OsString], usage: &str) -> Result<(), String> {
    match args.first() {
        Some(token) => Err(unexpected(token, usage)),
        None => Ok(()),
    }
}

fn value<'a>(token: Option<&'a OsString>, option: &str, usage: &str) -> Result<&'a OsStr, String> {
    token
        .filter(|token| !is_option(token))
        .map(OsString::as_os_str)
        .ok_or_else(|| error(format!("missing value for '{option}'"), usage))
}

fn recording(value: &OsStr, usage: &str) -> Result<bool, String> {
    match value.to_str() {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ if is_option(value) => Err(unexpected(value, usage)),
        _ => Err(error(
            format!(
                "invalid recording setting '{}'; expected on or off",
                value.to_string_lossy()
            ),
            usage,
        )),
    }
}

fn help(args: &[OsString]) -> Option<&'static str> {
    let level = if args.is_empty() {
        args
    } else if args
        .last()
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        &args[..args.len() - 1]
    } else {
        return None;
    };
    let level: Vec<_> = level.iter().map(|arg| arg.to_str()).collect();
    match level.as_slice() {
        [] => Some(
            "Usage: outrim <command>\n\nCommands:\n  install    Install an agent integration\n  uninstall  Remove an agent integration\n  upgrade    Update outrim and integrations (alias: update)\n  gain       Show compression savings\n  bridge     Run the agent bridge\n\nOptions:\n  -h, --help  Show help\n  --version   Show version",
        ),
        [Some("install")] => {
            Some("Install an agent integration.\n\nUsage: outrim install --agent <pi|opencode>")
        }
        [Some("uninstall")] => {
            Some("Remove an agent integration.\n\nUsage: outrim uninstall --agent <pi|opencode>")
        }
        [Some("upgrade" | "update")] => {
            Some("Update outrim and installed integrations.\n\nUsage: outrim upgrade")
        }
        [Some("gain")] => Some(
            "Show compression savings.\n\nUsage:\n  outrim gain\n  outrim gain --detail [--session <id>]\n  outrim gain record <on|off>\n\nOptions:\n  --detail        Show recorded sessions\n  --session <id>  Show session records",
        ),
        [Some("gain"), Some("record")] => Some(
            "Save tool arguments and full output locally.\n\nUsage: outrim gain record <on|off>",
        ),
        [Some("bridge")] => Some("Run the agent bridge over stdin/stdout.\n\nUsage: outrim bridge"),
        _ => None,
    }
}
