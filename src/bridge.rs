//! Persistent newline-delimited JSON transport. Each response is flushed before
//! reading the next message; request IDs identify process and delivery replies.

use crate::{
    Processor,
    protocol::{
        DeliveryAcknowledgement, ErrorResponse, ProtocolError, RecordDeliveryRequest,
        RecordObservationRequest, Request, VERSION,
    },
};
use serde_json::Value;
use std::io::{self, BufRead, Write};

pub fn run(
    processor: &Processor,
    input: impl BufRead,
    mut output: impl Write,
    mut diagnostics: impl Write,
) -> io::Result<()> {
    for line in input.split(b'\n') {
        let line = line?;
        let message = match parse_message(&line) {
            Ok(message) => message,
            Err(failure) => {
                writeln!(diagnostics, "outrim: invalid request: {}", failure.details)?;
                let response = ErrorResponse {
                    version: VERSION,
                    id: failure.id,
                    error: failure.error,
                };
                write_response(&mut output, &response)?;
                continue;
            }
        };

        match message {
            Message::RecordRecovery(recovery) => {
                let response = DeliveryAcknowledgement {
                    version: VERSION,
                    id: recovery.id.clone(),
                    acknowledged: processor.record_recovery(recovery),
                };
                write_response(&mut output, &response)?;
            }
            Message::Rewrite(request) => {
                let executable = std::env::current_exe()?;
                write_response(
                    &mut output,
                    &crate::rewrite::rewrite(request, &executable, processor),
                )?;
            }
            Message::ResolvePolicy(batch) => {
                let selection =
                    processor.selection(batch.request.context.as_ref(), &batch.request.tool);
                write_response(
                    &mut output,
                    &serde_json::json!({
                        "version": VERSION, "id": batch.id, "selection": selection
                    }),
                )?;
            }
            Message::ProcessParts(batch) => {
                let selection = batch.selection.unwrap_or_else(|| {
                    processor.selection(batch.request.context.as_ref(), &batch.request.tool)
                });
                let parts: Vec<_> = batch
                    .texts
                    .into_iter()
                    .map(|text| {
                        let mut request = batch.request.clone();
                        request.text = text;
                        processor.process_selected(request, &selection)
                    })
                    .collect();
                write_response(
                    &mut output,
                    &serde_json::json!({
                        "version": VERSION, "id": batch.request.id, "parts": parts
                    }),
                )?;
            }
            Message::Process(request) => {
                let response = processor.process(request);
                write_response(&mut output, &response)?;
            }
            Message::RecordObservation(observation) => {
                let response = DeliveryAcknowledgement {
                    version: VERSION,
                    id: observation.id.clone(),
                    acknowledged: processor.record_observation(*observation),
                };
                write_response(&mut output, &response)?;
            }
            Message::RecordDelivery(delivery) => {
                let response = DeliveryAcknowledgement {
                    version: VERSION,
                    id: delivery.id.clone(),
                    acknowledged: processor.record_delivery(delivery),
                };
                write_response(&mut output, &response)?;
            }
        }
    }
    Ok(())
}

fn write_response(output: &mut impl Write, response: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *output, response)?;
    writeln!(output)?;
    output.flush()
}

#[derive(serde::Deserialize)]
struct ProcessParts {
    request: Request,
    texts: Vec<String>,
    #[serde(default)]
    selection: Option<crate::policy::Selection>,
}

#[derive(serde::Deserialize)]
struct ResolvePolicy {
    id: String,
    request: Request,
}

enum Message {
    ResolvePolicy(ResolvePolicy),
    ProcessParts(ProcessParts),
    Rewrite(crate::rewrite::Request),
    Process(Request),
    RecordDelivery(RecordDeliveryRequest),
    RecordRecovery(crate::protocol::RecoveryRequest),
    RecordObservation(Box<RecordObservationRequest>),
}

struct ParseFailure {
    id: Option<String>,
    error: ProtocolError,
    details: String,
}

fn parse_message(line: &[u8]) -> Result<Message, ParseFailure> {
    let value: Value = serde_json::from_slice(line).map_err(|error| ParseFailure {
        id: None,
        error: ProtocolError::InvalidJson,
        details: error.to_string(),
    })?;
    let id = value.get("id").and_then(Value::as_str).map(str::to_owned);

    match value.get("type") {
        None => serde_json::from_value(value)
            .map(Message::Process)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) if kind == "resolve_policy" => serde_json::from_value(value)
            .map(Message::ResolvePolicy)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) if kind == "process_parts" => serde_json::from_value(value)
            .map(Message::ProcessParts)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) if kind == "rewrite" => serde_json::from_value(value)
            .map(Message::Rewrite)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) if kind == "record_observation" => serde_json::from_value(value)
            .map(Message::RecordObservation)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) if kind == "record_recovery" => serde_json::from_value(value)
            .map(Message::RecordRecovery)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) if kind == "record_delivery" => serde_json::from_value(value)
            .map(Message::RecordDelivery)
            .map_err(|error| ParseFailure {
                id,
                error: ProtocolError::InvalidRequest,
                details: error.to_string(),
            }),
        Some(Value::String(kind)) => Err(ParseFailure {
            id,
            error: ProtocolError::InvalidRequest,
            details: format!("Unsupported message type '{kind}'"),
        }),
        Some(_) => Err(ParseFailure {
            id,
            error: ProtocolError::InvalidRequest,
            details: "Message type must be a string".into(),
        }),
    }
}
