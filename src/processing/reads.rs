//! Session-local exact read baselines. Only adopted, unchanged bodies establish a baseline.
use crate::protocol::{Host, Request, Response};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Selection {
    pub path: String,
    pub key: String,
    pub label: String,
}

#[derive(Deserialize)]
pub struct ReadRequest {
    #[serde(flatten)]
    pub request: Request,
    pub selection: Selection,
    #[serde(default)]
    pub policy: Option<crate::policy::Selection>,
}

#[derive(Default)]
pub(super) struct Reads {
    delivered: HashMap<String, String>,
    pending: HashMap<String, (String, Option<String>)>,
    shells: HashMap<String, (NamedTempFile, Selection)>,
}

impl Reads {
    pub(super) fn reset(&mut self) {
        self.delivered.clear();
        self.pending.clear();
        self.shells.clear();
    }

    pub(super) fn prepare_shell(
        &mut self,
        host: Host,
        session: &str,
        call: &str,
        selection: Selection,
    ) -> Option<String> {
        let file = NamedTempFile::new().ok()?;
        let path = file.path().to_str()?.to_owned();
        self.shells
            .insert(shell_key(host, session, call), (file, selection));
        Some(path)
    }

    pub(super) fn take_shell(
        &mut self,
        host: Host,
        session: &str,
        call: &str,
        text: &str,
    ) -> Option<Selection> {
        let (file, mut selection) = self.shells.remove(&shell_key(host, session, call))?;
        let receipt: Receipt = serde_json::from_slice(&std::fs::read(file.path()).ok()?).ok()?;
        if receipt.text != text {
            return None;
        }
        selection.path = receipt.path.to_string_lossy().into_owned();
        Some(selection)
    }

    pub(super) fn has_shell(&self, host: Host, session: &str, call: &str) -> bool {
        self.shells.contains_key(&shell_key(host, session, call))
    }

    pub(super) fn key(host: Host, session: &str, selection: &Selection) -> Option<String> {
        let path = std::fs::canonicalize(&selection.path).ok()?;
        if !path.is_file() {
            return None;
        }
        serde_json::to_string(&(host, session, path, &selection.key)).ok()
    }

    pub(super) fn identical(&self, key: &str, text: &str) -> bool {
        self.delivered
            .get(key)
            .is_some_and(|previous| previous == text)
    }

    pub(super) fn stage(&mut self, key: String, response: &Response, complete: bool) {
        self.pending.insert(
            response.id.clone(),
            (key, complete.then(|| response.text.clone())),
        );
    }

    pub(super) fn adopt(&mut self, id: &str) {
        if let Some((key, text)) = self.pending.remove(id) {
            if let Some(text) = text {
                self.delivered.insert(key, text);
            } else {
                self.delivered.remove(&key);
            }
        }
    }
}

fn shell_key(host: Host, session: &str, call: &str) -> String {
    serde_json::to_string(&(host, session, call)).unwrap()
}

pub(super) fn text_read(text: &str) -> bool {
    !text.is_empty()
        && !text.contains(char::REPLACEMENT_CHARACTER)
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

// Used only for runtime-verified shell executions; the bridge owns these temporary receipts.
#[derive(Deserialize, Serialize)]
pub struct Receipt {
    pub path: PathBuf,
    pub text: String,
}

pub(crate) fn shell_selection(words: &[String]) -> Option<Selection> {
    let program = crate::command::program_name(words.first()?);
    let mut args = &words[1..];
    let (key, label) = match program.as_str() {
        "cat" => ("whole".to_owned(), "whole file".to_owned()),
        "head" | "tail" => {
            let mut count = "10";
            if args.first().is_some_and(|a| a == "-n") {
                count = args.get(1)?;
                args = &args[2..];
            } else if let Some(value) = args.first().and_then(|a| a.strip_prefix("-n")) {
                count = value;
                args = &args[1..];
            } else if let Some(value) = args.first().and_then(|a| a.strip_prefix('-'))
                && value.bytes().all(|b| b.is_ascii_digit())
                && !value.is_empty()
            {
                count = value;
                args = &args[1..];
            }
            let from = program == "tail" && count.starts_with('+');
            let value = count.strip_prefix('+').filter(|_| from).unwrap_or(count);
            let n = positive(value)?;
            if from {
                (format!("from:{n}"), format!("from line {n}"))
            } else {
                (
                    format!("{program}:{n}"),
                    format!(
                        "{} {n} lines",
                        if program == "head" { "first" } else { "last" }
                    ),
                )
            }
        }
        "sed" => {
            if args.first()? != "-n" {
                return None;
            }
            let action = args.get(1)?.strip_suffix('p')?;
            args = &args[2..];
            if let Some((start, end)) = action.split_once(',') {
                let start = positive(start)?;
                if end == "$" {
                    (format!("sed:{start}:end"), format!("{start}–end"))
                } else {
                    let end = positive(end)?;
                    if end < start {
                        return None;
                    }
                    (format!("sed:{start}:{end}"), format!("{start}–{end}"))
                }
            } else {
                let n = positive(action)?;
                (format!("sed:{n}"), n.to_string())
            }
        }
        _ => return None,
    };
    if args.first().is_some_and(|a| a == "--") {
        args = &args[1..];
    }
    if args.len() != 1 || args[0].is_empty() || args[0].starts_with('-') {
        return None;
    }
    Some(Selection {
        path: args[0].clone(),
        key: format!("shell:{key}"),
        label,
    })
}

fn positive(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|n| *n > 0)
}
