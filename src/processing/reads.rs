//! Session-local read evidence. Only adopted source text establishes a baseline.
use crate::protocol::{Host, Request, Response};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Selection {
    pub path: String,
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub body: Option<Body>,
}

/// Verified host formatting, separate from source lines. The rendered pieces must
/// reproduce the actual tool result exactly before any evidence can be reused.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Body {
    pub start: u64,
    #[serde(default)]
    pub end_relative: bool,
    #[serde(default)]
    pub prefix: String,
    pub lines: Vec<Line>,
    #[serde(default)]
    pub suffix: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Line {
    pub text: String,
    pub rendered: String,
}

#[derive(Default)]
struct Baseline {
    exact: HashMap<String, String>,
    lines: BTreeMap<(bool, u64), String>,
}

struct Pending {
    file: String,
    selection: String,
    text: Option<String>,
    retain_exact: bool,
    lines: Vec<((bool, u64), String)>,
}

pub(super) struct Comparison {
    pub body: Option<Body>,
    pub omitted: Vec<bool>,
    pub conflict: bool,
    pub identical: bool,
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
    delivered: HashMap<String, Baseline>,
    pending: HashMap<String, Pending>,
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
        selection.body = shell_body(&selection.key, text);
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
        serde_json::to_string(&(host, session, path)).ok()
    }

    pub(super) fn compare(&mut self, key: &str, selection: &Selection, text: &str) -> Comparison {
        let body = selection.body.clone().filter(|body| body.valid(text));
        let baseline = self.delivered.get(key);
        let identical = baseline
            .and_then(|b| b.exact.get(&selection.key))
            .is_some_and(|old| old == text);
        let mut omitted = vec![false; body.as_ref().map_or(0, |b| b.lines.len())];
        let mut conflict = false;
        if let (Some(baseline), Some(body)) = (baseline, &body) {
            for (index, line) in body.lines.iter().enumerate() {
                if let Some(old) = baseline.lines.get(&body.position(index)) {
                    if old != &line.text {
                        conflict = true;
                    } else {
                        omitted[index] = true;
                    }
                }
            }
        } else if let Some(old) = baseline.and_then(|b| b.exact.get(&selection.key)) {
            conflict = old != text;
        }
        if conflict {
            self.delivered.remove(key);
            // A file conflict is an epoch change: older unadopted results cannot
            // revive its evidence even if their acknowledgements arrive later.
            self.pending.retain(|_, pending| pending.file != key);
            omitted.fill(false);
        }
        Comparison {
            body,
            omitted,
            conflict,
            identical: identical && !conflict,
        }
    }

    pub(super) fn stage(
        &mut self,
        key: String,
        selection: &Selection,
        response: &Response,
        original: &str,
        comparison: &Comparison,
    ) {
        let complete = response.text == original;
        let ranged = response.applied_rule.as_deref() == Some("read_dedup");
        let mut lines = Vec::new();
        if let Some(body) = &comparison.body {
            for (index, line) in body.lines.iter().enumerate() {
                if complete || (ranged && !comparison.omitted[index]) {
                    lines.push((body.position(index), line.text.clone()));
                }
            }
        }
        self.pending.insert(
            response.id.clone(),
            Pending {
                file: key,
                selection: selection.key.clone(),
                text: complete.then(|| original.to_owned()),
                retain_exact: ranged,
                lines,
            },
        );
    }

    pub(super) fn adopt(&mut self, id: &str) {
        if let Some(pending) = self.pending.remove(id) {
            let conflict = self.delivered.get(&pending.file).is_some_and(|baseline| {
                pending.lines.iter().any(|(position, text)| {
                    baseline.lines.get(position).is_some_and(|old| old != text)
                }) || (pending.lines.is_empty()
                    && pending.text.as_ref().is_some_and(|text| {
                        baseline
                            .exact
                            .get(&pending.selection)
                            .is_some_and(|old| old != text)
                    }))
            });
            if conflict {
                self.delivered.remove(&pending.file);
                self.pending.retain(|_, old| old.file != pending.file);
            }
            let baseline = self.delivered.entry(pending.file).or_default();
            if let Some(text) = pending.text {
                baseline.exact.insert(pending.selection, text);
            } else if !pending.retain_exact {
                baseline.exact.remove(&pending.selection);
            }
            baseline.lines.extend(pending.lines);
        }
    }
}

impl Body {
    fn valid(&self, text: &str) -> bool {
        self.start > 0
            && !self.lines.is_empty()
            && self.start.checked_add(self.lines.len() as u64).is_some()
            && (!self.end_relative || self.start == self.lines.len() as u64)
            && self.lines.iter().all(|line| {
                !line.rendered.is_empty()
                    && !line.text.is_empty()
                    && line.text.split_inclusive('\n').count() == 1
            })
            && format!(
                "{}{}{}",
                self.prefix,
                self.lines
                    .iter()
                    .map(|line| line.rendered.as_str())
                    .collect::<String>(),
                self.suffix
            ) == text
    }

    fn position(&self, index: usize) -> (bool, u64) {
        (
            self.end_relative,
            if self.end_relative {
                self.start - index as u64
            } else {
                self.start + index as u64
            },
        )
    }

    pub(super) fn omit(&self, selection: &Selection, omitted: &[bool]) -> String {
        let mut output = self.prefix.clone();
        let mut index = 0;
        while index < self.lines.len() {
            if !omitted[index] {
                output.push_str(&self.lines[index].rendered);
                index += 1;
                continue;
            }
            let start = index;
            while index < self.lines.len() && omitted[index] {
                index += 1;
            }
            let range = if self.end_relative {
                format!("last {} lines", self.position(start).1)
            } else {
                format!("{}–{}", self.position(start).1, self.position(index - 1).1)
            };
            output.push_str(&format!(
                "[outrim]: Already read {}:{range}; verified unchanged, so omitted.\n",
                selection.path
            ));
        }
        output.push_str(&self.suffix);
        output
    }
}

fn shell_body(key: &str, text: &str) -> Option<Body> {
    let parts: Vec<_> = key.strip_prefix("shell:")?.split(':').collect();
    let (start, end_relative) = match parts.as_slice() {
        ["whole"] | ["head", _] => (1, false),
        ["from", start] | ["sed", start] | ["sed", start, _] => (positive(start)?, false),
        ["tail", _] => (text.split_inclusive('\n').count() as u64, true),
        _ => return None,
    };
    Some(Body {
        start,
        end_relative,
        prefix: String::new(),
        suffix: String::new(),
        lines: text
            .split_inclusive('\n')
            .map(|line| Line {
                text: line.into(),
                rendered: line.into(),
            })
            .collect(),
    })
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
        body: None,
    })
}

fn positive(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().filter(|n| *n > 0)
}
