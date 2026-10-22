use crate::{
    protocol::{Candidate, Disposition, Host, Response},
    storage,
};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub(super) struct Originals {
    root: PathBuf,
}

impl Originals {
    pub(super) fn new(home: &Path) -> Self {
        Self {
            root: home.join(".outrim/outputs"),
        }
    }

    // Final cost and original persistence are one decision: the hint needs the
    // reserved path, but the original is kept only when the final text is shorter.
    pub(super) fn compress(
        &self,
        host: Host,
        session: &str,
        candidates: Vec<(&'static str, String)>,
        response: &mut Response,
    ) {
        let original_length = response.text.chars().count();
        response.candidates = candidates
            .iter()
            .map(|(rule, text)| Candidate {
                rule: (*rule).into(),
                body_chars: text.chars().count() as u64,
                final_chars: None,
                reason: "not_shorter".into(),
            })
            .collect();
        if response
            .candidates
            .iter()
            .all(|candidate| candidate.body_chars >= original_length as u64)
        {
            response.disposition = Disposition::NotShorter;
            return;
        }
        let result = (|| -> io::Result<_> {
            let file = self.reserve(host, session)?;
            let path = file
                .path()
                .to_str()
                .ok_or_else(|| io::Error::other("non-Unicode original path"))?;
            let best = candidates
                .into_iter()
                .enumerate()
                .map(|(index, (rule, candidate))| {
                    let separator = if candidate.ends_with('\n') { "" } else { "\n" };
                    let text = format!(
                        "{candidate}{separator}[outrim]: The full output is available at {path}"
                    );
                    let final_chars = text.chars().count();
                    response.candidates[index].final_chars = Some(final_chars as u64);
                    response.candidates[index].reason = if final_chars < original_length {
                        "smaller_or_equal_final_selected".into()
                    } else {
                        "not_shorter_with_hint".into()
                    };
                    (rule, text, final_chars)
                })
                .filter(|(_, _, final_chars)| *final_chars < original_length)
                .min_by_key(|(_, _, final_chars)| *final_chars);
            let Some((rule, text, _)) = best else {
                return Ok(None);
            };
            let path = storage::commit(file, &response.text)?;
            Ok(Some((text, path, rule)))
        })();
        match result {
            Ok(Some((text, path, rule))) => {
                response.text = text;
                response.original_path = Some(path.to_string_lossy().into_owned());
                response.applied_rule = Some(rule.into());
                response.disposition = Disposition::Compressed;
                for candidate in &mut response.candidates {
                    if candidate.rule == rule {
                        candidate.reason = "selected".into();
                    }
                }
            }
            Ok(None) => response.disposition = Disposition::NotShorter,
            Err(_) => {
                response.disposition = Disposition::FailedOpen;
                response.reason = Some("original_storage_failed".into());
                for candidate in &mut response.candidates {
                    candidate.reason = "storage_failed".into();
                }
            }
        }
    }

    pub(super) fn omit_read(
        &self,
        host: Host,
        session: &str,
        selection: &super::reads::Selection,
        response: &mut Response,
    ) {
        let result = (|| -> io::Result<_> {
            let file = self.reserve(host, session)?;
            let path = file
                .path()
                .to_str()
                .ok_or_else(|| io::Error::other("non-Unicode original path"))?;
            let text = format!(
                "[outrim]: Already read {}:{}; unchanged, so omitted. Reread: read \"{path}\".",
                selection.path, selection.label
            );
            if text.chars().count() >= response.text.chars().count() {
                return Ok(None);
            }
            let path = storage::commit(file, &response.text)?;
            Ok(Some((text, path)))
        })();
        match result {
            Ok(Some((text, path))) => {
                response.text = text;
                response.original_path = Some(path.to_string_lossy().into_owned());
                response.applied_rule = Some("read_dedup".into());
                response.disposition = Disposition::Compressed;
                response.reason = Some("already_read_unchanged".into());
                response.candidates.clear();
            }
            Ok(None) => response.disposition = Disposition::NotShorter,
            Err(_) => {
                response.disposition = Disposition::FailedOpen;
                response.reason = Some("original_storage_failed".into());
            }
        }
    }

    fn reserve(&self, host: Host, session: &str) -> io::Result<NamedTempFile> {
        let host = match host {
            Host::Pi => "pi",
            Host::Opencode => "opencode",
            Host::Exec => "exec",
        };
        // Session IDs are opaque and unbounded; never use their bytes as a path
        // component. This fingerprint only groups files, not their identity:
        // random file names prevent overwrites even if two fingerprints collide.
        // Readers retain exact paths, so no hash algorithm is a recovery contract.
        let mut hasher = DefaultHasher::new();
        session.hash(&mut hasher);
        let session = format!("s-{:016x}", hasher.finish());
        let root = std::path::absolute(&self.root)?;
        let base = root
            .parent()
            .ok_or_else(|| io::Error::other("Invalid output root"))?;
        for dir in [
            base.to_path_buf(),
            root.clone(),
            root.join(host),
            root.join(host).join(&session),
        ] {
            storage::private_directory(&dir)?;
        }
        tempfile::Builder::new()
            .prefix("o-")
            .suffix(".txt")
            .tempfile_in(root.join(host).join(session))
    }
}
