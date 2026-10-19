//! Single-output ingress processing. The caller supplies the current user's home,
//! not an independently configurable original-output directory.

pub mod bridge;
pub mod install;
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
        let Some(tail) = rules::log_tail(&request.tool, &response.text) else {
            return response;
        };
        if tail.chars().count() >= response.text.chars().count() {
            response.disposition = Disposition::NotShorter;
            return response;
        }
        let result = (|| -> std::io::Result<_> {
            let file = self.originals.reserve(request.host, &request.session)?;
            let path = file
                .path()
                .to_str()
                .ok_or_else(|| std::io::Error::other("non-Unicode original path"))?;
            let separator = if tail.ends_with('\n') { "" } else { "\n" };
            let candidate =
                format!("{tail}{separator}[outrim]: The full output is available at {path}");
            if candidate.chars().count() >= response.text.chars().count() {
                return Ok(None);
            }
            let path = storage::commit(file, &response.text)?;
            Ok(Some((candidate, path)))
        })();
        match result {
            Ok(Some((text, path))) => {
                response.text = text;
                response.original_path = Some(path.to_string_lossy().into_owned());
                response.applied_rule = Some("log_tail".into());
                response.disposition = Disposition::Compressed;
            }
            Ok(None) => response.disposition = Disposition::NotShorter,
            Err(_error) => response.disposition = Disposition::FailedOpen,
        }
        response
    }
}
