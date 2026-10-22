//! Single-output ingress processing. The caller supplies the current user's home,
//! not an independently configurable original-output directory.

mod compression;
mod json;
mod log_compaction;
mod path_list;
mod protection;
pub mod reads;
mod search_footer;
mod search_hits;

use crate::{
    gain,
    protocol::{self, Disposition, Request, Response, Source, VERSION},
    recording,
};
use std::{cell::RefCell, path::Path};

pub struct Processor {
    originals: compression::Originals,
    gain: gain::Store,
    recorder: recording::Recorder,
    reads: RefCell<reads::Reads>,
}

impl Processor {
    pub fn new(user_home: &Path) -> Self {
        Self::with_recording(user_home, recording::enabled(user_home).unwrap_or(false))
    }

    /// Use the host's startup setting, including when it restarts its bridge.
    pub fn with_recording(user_home: &Path, enabled: bool) -> Self {
        Self {
            originals: compression::Originals::new(user_home),
            gain: gain::Store::new(user_home),
            recorder: recording::Recorder::new(user_home, enabled),
            reads: RefCell::default(),
        }
    }

    pub(crate) fn record_recovery(&self, recovery: protocol::RecoveryRequest) -> bool {
        if self.gain.record_recovery(&recovery).is_err() {
            return false;
        }
        self.recorder
            .record(protocol::RecordObservationRequest {
                message_type: protocol::ObservationMessageType::RecordObservation,
                version: VERSION,
                id: recovery.id.clone(),
                host: recovery.host,
                session: Some(recovery.session.clone()),
                tool_call_id: Some(recovery.tool_call_id.clone()),
                tool: protocol::Tool {
                    name: "recovery".into(),
                    input: serde_json::json!(recovery.original_paths),
                    source: Source::Unknown,
                    exit_code: None,
                },
                original_text: None,
                final_text: None,
                outcome: protocol::ObservationOutcome::RecoverySkipped,
                applied_rule: None,
                original_path: None,
                candidates: vec![],
                reason: None,
                parts: vec![],
                execution: None,
                metadata_only: true,
                diagnostics: Some(protocol::Diagnostics {
                    recovery: Some(recovery),
                    ..Default::default()
                }),
            })
            .is_ok()
    }

    pub(crate) fn record_delivery(&self, delivery: protocol::RecordDeliveryRequest) -> bool {
        self.gain.record(delivery).is_ok()
    }

    pub(crate) fn record_observation(
        &self,
        observation: protocol::RecordObservationRequest,
    ) -> bool {
        self.recorder.record(observation).is_ok()
    }

    pub(crate) fn prepare_shell_read(
        &self,
        host: protocol::Host,
        session: &str,
        call: &str,
        selection: reads::Selection,
    ) -> Option<String> {
        self.reads
            .borrow_mut()
            .prepare_shell(host, session, call, selection)
    }

    pub(crate) fn process_shell_read(&self, request: Request, call: &str) -> Response {
        let selection =
            self.reads
                .borrow_mut()
                .take_shell(request.host, &request.session, call, &request.text);
        if let Some(selection) = selection {
            return self.process_read(reads::ReadRequest { request, selection });
        }
        // Unverified/fallback shell output must not be processed a second time.
        Response {
            version: VERSION,
            id: request.id,
            text: request.text,
            disposition: Disposition::NoMatch,
            applied_rule: None,
            original_path: None,
            candidates: vec![],
            reason: Some("unverified_shell_read".into()),
        }
    }

    pub(crate) fn has_shell_read(&self, host: protocol::Host, session: &str, call: &str) -> bool {
        self.reads.borrow().has_shell(host, session, call)
    }

    pub(crate) fn reset_reads(&self) {
        self.reads.borrow_mut().reset();
    }

    pub(crate) fn deliver_read(&self, id: &str) {
        self.reads.borrow_mut().adopt(id);
    }

    pub(crate) fn process_read(&self, read: reads::ReadRequest) -> Response {
        let reads::ReadRequest { request, selection } = read;
        let eligible = request.version == VERSION
            && !protection::recovery_reference(&request.tool.input)
            && request.tool.exit_code.is_none_or(|code| code == 0);
        let key = eligible
            .then(|| reads::Reads::key(request.host, &request.session, &selection))
            .flatten();
        let textual = reads::text_read(&request.text);
        let identical = textual
            && key
                .as_ref()
                .is_some_and(|key| self.reads.borrow().identical(key, &request.text));
        let text = request.text.clone();
        let host = request.host;
        let session = request.session.clone();
        let mut response = self.process(request);
        if identical {
            // Source/config protection remains intact for ordinary compression, but exact
            // duplicate reads can omit the body with an always-readable original.
            response.text = text.clone();
            response.applied_rule = None;
            response.original_path = None;
            response.reason = None;
            response.candidates.clear();
            self.originals
                .omit_read(host, &session, &selection, &mut response);
        } else if let Some(key) = key {
            self.reads
                .borrow_mut()
                .stage(key, &response, textual && response.text == text);
        }
        response
    }

    pub fn process(&self, request: Request) -> Response {
        let mut response = Response {
            candidates: Vec::new(),
            reason: None,
            version: VERSION,
            id: request.id,
            text: request.text,
            disposition: Disposition::NoMatch,
            applied_rule: None,
            original_path: None,
        };
        if request.version != VERSION {
            response.disposition = Disposition::FailedOpen;
            response.reason = Some("unsupported_protocol_version".into());
            return response;
        }
        if protection::recovery_reference(&request.tool.input) {
            response.disposition = Disposition::RecoverySkipped;
            response.reason = Some("original_output_reference".into());
            return response;
        }
        // Search paths are query scope only when the output parses as hit records.
        let search_candidate = if matches!(request.tool.source, Source::Search) {
            search_hits::group_search_hits(&response.text)
        } else {
            None
        };
        if protection::protected(&request.tool) && search_candidate.is_none() {
            response.disposition = Disposition::Protected;
            response.reason = Some("source_or_configuration_protection".into());
            return response;
        }
        let candidates =
            if let Some(compacted) = log_compaction::compact(&request.tool, &response.text) {
                vec![("log_compaction", compacted)]
            } else if let Some(grouped) = search_candidate {
                vec![("search_hit_groups", grouped)]
            } else if let Some(grouped) = matches!(request.tool.source, Source::Search)
                .then(|| path_list::group_prefixes(&response.text))
                .flatten()
            {
                vec![("path_prefix_groups", grouped)]
            } else if let Some(json_candidates) = json::candidates(&request.tool, &response.text) {
                json_candidates
            } else {
                response.reason = Some(
                    if matches!(request.tool.source, Source::Search) {
                        "parse_failed"
                    } else {
                        "source_not_applicable"
                    }
                    .into(),
                );
                return response;
            };
        let json_rejection = candidates
            .iter()
            .any(|(rule, _)| *rule == "json_compact")
            .then(|| json::rejection(&response.text))
            .flatten();
        self.originals
            .compress(request.host, &request.session, candidates, &mut response);
        if let Some(reason) = json_rejection {
            response.reason = Some(reason.into());
            for candidate in &mut response.candidates {
                candidate.reason = reason.into();
            }
        }
        response
    }
}
