//! Stream delivery shared by explicit execution and future host execution adapters.
use crate::{Processor, command, protocol::*, recording};
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
    #[serde(default)]
    pub tool: Option<Tool>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Rewritten unknown programs must not acquire builtin capture by default.
    #[serde(default)]
    pub configured_only: bool,
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
    selection: crate::policy::Selection,
}

impl Execution {
    pub fn new(home: &Path, arguments: &[OsString], context: Context) -> Self {
        let recording = context
            .recording
            .unwrap_or_else(|| recording::enabled(home).unwrap_or(false));
        let tool = context.tool.clone().unwrap_or_else(|| Tool {
            name: "exec".into(),
            input: serde_json::json!({"argv": arguments.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>()}),
            source: context.source.unwrap_or_else(|| command::source(arguments)),
            exit_code: None,
        });
        let processor = Processor::with_recording(home, recording);
        let call = std::env::current_dir()
            .ok()
            .map(|cwd| crate::policy::CallContext {
                cwd,
                command: Some(context.command.clone().unwrap_or_else(|| {
                    arguments
                        .iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect()
                })),
            });
        let selection = processor.selection(call.as_ref(), &tool);
        Self {
            processor,
            selection,
            context,
            tool,
            recording,
        }
    }

    pub fn captures(&self) -> bool {
        match &self.selection {
            crate::policy::Selection::Off => false,
            crate::policy::Selection::Builtin => !self.context.configured_only,
            crate::policy::Selection::Methods(_) => true,
        }
    }

    pub fn record_fallback(&self) {
        self.processor.record_observation(RecordObservationRequest {
            message_type: ObservationMessageType::RecordObservation,
            version: VERSION,
            id: self.context.unit_id.clone(),
            host: self.context.host,
            session: self.context.session.clone(),
            tool_call_id: self.context.tool_call_id.clone(),
            tool: Tool {
                name: "shell".into(),
                input: serde_json::Value::Null,
                source: Source::Unknown,
                exit_code: None,
            },
            original_text: None,
            final_text: None,
            metadata_only: true,
            diagnostics: Some(Diagnostics {
                wrapping: Some("runtime_bypassed".into()),
                unit_id: Some(self.context.unit_id.clone()),
                execution_state: Some("runtime_fallback".into()),
                ..Default::default()
            }),
            outcome: ObservationOutcome::Unsupported,
            reason: Some("runtime_resolution_fallback".into()),
            applied_rule: None,
            original_path: None,
            candidates: vec![],
            parts: vec![],
            execution: None,
        });
    }

    /// Record why an unmeasured byte stream bypassed processing, never its content.
    pub fn bypass(
        &self,
        stream: OutputStream,
        exit_code: Option<i32>,
        reason: &str,
        mut diagnostics: Diagnostics,
    ) {
        diagnostics.execution_state = Some("executed".into());
        diagnostics.unit_id = Some(self.context.unit_id.clone());
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
            metadata_only: true,
            diagnostics: Some(diagnostics),
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
        output: impl Write,
    ) -> io::Result<()> {
        let diagnostics = Diagnostics {
            output_origin: Some("native_capture".into()),
            completeness: Some("complete".into()),
            observed_bytes: Some(text.len() as u64),
            observed_chars: Some(text.chars().count() as u64),
            total_bytes: Some(text.len() as u64),
            ..Diagnostics::default()
        };
        self.deliver_captured(stream, text, exit_code, output, diagnostics)
    }

    pub fn deliver_captured(
        &self,
        stream: OutputStream,
        text: String,
        exit_code: Option<i32>,
        mut output: impl Write,
        mut diagnostics: Diagnostics,
    ) -> io::Result<()> {
        diagnostics.execution_state = Some("executed".into());
        diagnostics.unit_id = Some(self.context.unit_id.clone());
        if text.is_empty() {
            self.bypass(stream, exit_code, "empty_output", diagnostics);
            return Ok(());
        }
        let mut tool = self.tool.clone();
        tool.exit_code = exit_code;
        let id = format!("{}-{stream:?}", self.context.unit_id);
        let response = self.processor.process_selected(
            Request {
                context: None,
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
            },
            &self.selection,
        );
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
            metadata_only: false,
            diagnostics: Some(diagnostics),
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
