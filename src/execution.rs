//! Stream delivery shared by explicit execution and future host execution adapters.
use crate::{Processor, command, gain, protocol::*};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::Path,
};

/// Facts supplied by a host. Absent session/call/source facts stay absent.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub unit_id: String,
    pub host: Host,
    pub session: Option<String>,
    pub tool_call_id: Option<String>,
    pub source: Option<Source>,
    pub recording: Option<bool>,
}

impl Context {
    /// A shell-neutral transport: legacy PowerShell strips quotes in native arguments.
    pub fn encode(&self) -> String {
        serde_json::to_vec(self)
            .expect("serializable execution context")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn decode(value: &str) -> Option<Self> {
        if !value.len().is_multiple_of(2) || !value.is_ascii() {
            return None;
        }
        let bytes: Option<Vec<u8>> = (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
            .collect();
        serde_json::from_slice(&bytes?).ok()
    }
}

pub struct Execution {
    processor: Processor,
    context: Context,
    tool: Tool,
    recording: bool,
}

impl Execution {
    pub fn new(home: &Path, arguments: &[OsString], context: Context) -> Self {
        let recording = context
            .recording
            .unwrap_or_else(|| gain::recording_enabled(home).unwrap_or(false));
        let tool = Tool {
            name: "exec".into(),
            input: serde_json::json!({"argv": arguments.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>()}),
            source: context.source.unwrap_or_else(|| command::source(arguments)),
            exit_code: None,
        };
        Self {
            processor: Processor::with_recording(home, recording),
            context,
            tool,
            recording,
        }
    }

    /// Record why an unmeasured byte stream bypassed processing, never its content.
    pub fn bypass(&self, stream: OutputStream, exit_code: Option<i32>, reason: &str) {
        let mut tool = self.tool.clone();
        tool.exit_code = exit_code;
        self.processor.record_observation(RecordObservationRequest {
            message_type: ObservationMessageType::RecordObservation,
            version: VERSION,
            id: format!("{}-{stream:?}", self.context.unit_id),
            host: self.context.host,
            session: self.context.session.clone(),
            tool_call_id: self.context.tool_call_id.clone(),
            tool,
            original_text: None,
            final_text: None,
            outcome: ObservationOutcome::Unsupported,
            applied_rule: None,
            original_path: None,
            candidates: vec![],
            reason: Some(reason.into()),
            parts: vec![],
            execution: Some(ExecutionMetadata {
                unit_id: self.context.unit_id.clone(),
                stream,
                recording: self.recording,
            }),
        });
    }

    /// Only complete UTF-8 streams reach this seam. Count a delivery only after writing it.
    pub fn deliver(
        &self,
        stream: OutputStream,
        text: String,
        exit_code: Option<i32>,
        mut output: impl Write,
    ) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut tool = self.tool.clone();
        tool.exit_code = exit_code;
        let id = format!("{}-{stream:?}", self.context.unit_id);
        let response = self.processor.process(Request {
            version: VERSION,
            id: id.clone(),
            host: self.context.host,
            session: self
                .context
                .session
                .clone()
                .unwrap_or_else(|| self.context.unit_id.clone()),
            tool: tool.clone(),
            text: text.clone(),
        });
        output.write_all(response.text.as_bytes())?;
        output.flush()?;
        self.processor.record_delivery(RecordDeliveryRequest {
            message_type: DeliveryMessageType::RecordDelivery,
            version: VERSION,
            id: id.clone(),
            disposition: response.disposition,
            applied_rule: response.applied_rule.clone(),
            before_chars: text.chars().count() as u64,
            after_chars: response.text.chars().count() as u64,
        });
        let outcome = match response.disposition {
            Disposition::Compressed => ObservationOutcome::Compressed,
            Disposition::Protected => ObservationOutcome::Protected,
            Disposition::RecoverySkipped => ObservationOutcome::RecoverySkipped,
            Disposition::NotShorter => ObservationOutcome::NotShorter,
            Disposition::NoMatch => ObservationOutcome::NoMatch,
            Disposition::FailedOpen => ObservationOutcome::FailedOpen,
        };
        self.processor.record_observation(RecordObservationRequest {
            message_type: ObservationMessageType::RecordObservation,
            version: VERSION,
            id,
            host: self.context.host,
            session: self.context.session.clone(),
            tool_call_id: self.context.tool_call_id.clone(),
            tool,
            original_text: Some(text),
            final_text: Some(response.text),
            outcome,
            applied_rule: response.applied_rule,
            original_path: response.original_path,
            candidates: response.candidates,
            reason: response.reason,
            parts: vec![],
            execution: Some(ExecutionMetadata {
                unit_id: self.context.unit_id.clone(),
                stream,
                recording: self.recording,
            }),
        });
        Ok(())
    }
}
