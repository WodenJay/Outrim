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
            Message::Process(request) => {
                let response = processor.process(request);
                write_response(&mut output, &response)?;
            }
            Message::RecordObservation(observation) => {
                let response = DeliveryAcknowledgement {
                    version: VERSION,
                    id: observation.id.clone(),
                    acknowledged: processor.record_observation(observation),
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

enum Message {
    Process(Request),
    RecordDelivery(RecordDeliveryRequest),
    RecordObservation(RecordObservationRequest),
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
        Some(Value::String(kind)) if kind == "record_observation" => serde_json::from_value(value)
            .map(Message::RecordObservation)
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
