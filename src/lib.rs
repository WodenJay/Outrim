//! Single-output ingress processing. The caller supplies the current user's home,
//! not an independently configurable original-output directory.

pub mod bridge;
mod json;
pub mod protocol;
mod rules;
mod storage;

use protocol::{Disposition, Request, Response, VERSION};
use std::path::Path;

pub struct Processor {
    originals: storage::Originals,
}

impl Processor {
    pub fn new(user_home: &Path) -> Self {
        Self {
            originals: storage::Originals::new(user_home),
        }
    }

    pub fn process(&self, request: Request) -> Response {
        let mut response = Response {
            version: VERSION,
            id: request.id,
            text: request.text,
            disposition: Disposition::NoMatch,
            applied_rule: None,
            original_path: None,
        };
        if request.version != VERSION {
            response.disposition = Disposition::FailedOpen;
            return response;
        }
        if rules::protected(&request.tool) {
            response.disposition = Disposition::Protected;
            return response;
        }
        let candidates = if let Some(tail) = rules::log_tail(&request.tool, &response.text) {
            vec![("log_tail", tail)]
        } else if let Some(json_candidates) = json::candidates(&request.tool, &response.text) {
            json_candidates
        } else {
            return response;
        };
        if candidates
            .iter()
            .all(|(_, text)| text.chars().count() >= response.text.chars().count())
        {
            response.disposition = Disposition::NotShorter;
            return response;
        }
        let original_length = response.text.chars().count();
        let result = (|| -> std::io::Result<_> {
            let file = self.originals.reserve(request.host, &request.session)?;
            let path = file
                .path()
                .to_str()
                .ok_or_else(|| std::io::Error::other("non-Unicode original path"))?;
            let best = candidates
                .into_iter()
                .map(|(rule, candidate)| {
                    let separator = if candidate.ends_with('\n') { "" } else { "\n" };
                    let text = format!(
                        "{candidate}{separator}[outrim]: The full output is available at {path}"
                    );
                    (rule, text)
                })
                .filter(|(_, text)| text.chars().count() < original_length)
                .min_by_key(|(_, text)| text.chars().count());
            let Some((rule, text)) = best else {
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
            }
            Ok(None) => response.disposition = Disposition::NotShorter,
            Err(_error) => response.disposition = Disposition::FailedOpen,
        }
        response
    }
}
