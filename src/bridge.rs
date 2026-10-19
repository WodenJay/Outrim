//! Persistent newline-delimited JSON transport. Each response is flushed before
//! reading the next request; request IDs, not arrival order, identify results.

use crate::{
    Processor,
    protocol::{ErrorResponse, ProtocolError, Request, VERSION},
};
use std::io::{self, BufRead, Write};

pub fn run(
    processor: &Processor,
    input: impl BufRead,
    mut output: impl Write,
    mut diagnostics: impl Write,
) -> io::Result<()> {
    for line in input.split(b'\n') {
        let line = line?;
        let request = match serde_json::from_slice::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                writeln!(diagnostics, "outrim: invalid request: {error}")?;
                let value = serde_json::from_slice::<serde_json::Value>(&line);
                let id = value
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get("id"))
                    .and_then(|id| id.as_str())
                    .map(str::to_owned);
                let response = ErrorResponse {
                    version: VERSION,
                    id,
                    error: if value.is_ok() {
                        ProtocolError::InvalidRequest
                    } else {
                        ProtocolError::InvalidJson
                    },
                };
                serde_json::to_writer(&mut output, &response)?;
                writeln!(output)?;
                output.flush()?;
                continue;
            }
        };
        let response = processor.process(request);
        serde_json::to_writer(&mut output, &response)?;
        writeln!(output)?;
        output.flush()?;
    }
    Ok(())
}
