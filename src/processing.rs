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
    policy::{self, Method, Selection},
    protocol::{self, Disposition, Request, Response, Source, VERSION},
    recording, tokens,
};
use std::{cell::RefCell, path::Path};

enum Grouping {
    Unrecognized,
    NotApplicable,
    Candidate(String),
}

impl Grouping {
    fn recognized(&self) -> bool {
        !matches!(self, Self::Unrecognized)
    }

    fn candidate(self) -> Option<String> {
        match self {
            Self::Candidate(text) => Some(text),
            Self::Unrecognized | Self::NotApplicable => None,
        }
    }
}

pub struct Processor {
    home: std::path::PathBuf,
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
            home: user_home.to_owned(),
            originals: compression::Originals::new(user_home),
            gain: gain::Store::new(user_home),
            recorder: recording::Recorder::new(user_home, enabled),
            reads: RefCell::default(),
        }
    }

    pub(crate) fn record_recovery(&self, recovery: protocol::RecoveryRequest) -> bool {
        let cost = tokens::count(&recovery.text);
        if self.gain.record_recovery(&recovery, cost).is_err() {
            return false;
        }
        self.recorder
            .record(
                protocol::RecordObservationRequest {
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
                        recovery: Some(protocol::RecoveryDetails {
                            event_id: recovery.event_id,
                            tokens: cost,
                            attribution: recovery.attribution,
                            original_paths: recovery.original_paths,
                        }),
                        ..Default::default()
                    }),
                },
                None,
            )
            .is_ok()
    }

    pub(crate) fn record_delivery(&self, mut delivery: protocol::RecordDeliveryRequest) -> bool {
        let counts = tokens::Counts::new(&delivery.original_text, &delivery.final_text);
        let recorded = self.gain.record(&delivery, counts).is_ok();
        if let Some(observation) = delivery.observation.take() {
            let same_text = observation.original_text.as_deref() == Some(&delivery.original_text)
                && observation.final_text.as_deref() == Some(&delivery.final_text);
            let _ = self
                .recorder
                .record(*observation, same_text.then_some(counts));
        }
        recorded
    }

    pub(crate) fn record_observation(
        &self,
        observation: protocol::RecordObservationRequest,
    ) -> bool {
        self.recorder.record(observation, None).is_ok()
    }

    pub fn selection(
        &self,
        context: Option<&policy::CallContext>,
        tool: &protocol::Tool,
    ) -> Selection {
        policy::resolve(&self.home, context, tool)
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

    pub(crate) fn process_shell_read(
        &self,
        request: Request,
        call: &str,
        policy: Option<Selection>,
    ) -> Response {
        let selection =
            self.reads
                .borrow_mut()
                .take_shell(request.host, &request.session, call, &request.text);
        if let Some(selection) = selection {
            return self.process_read(reads::ReadRequest {
                request,
                selection,
                policy,
            });
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
        let reads::ReadRequest {
            request,
            selection,
            policy,
        } = read;
        let policy =
            policy.unwrap_or_else(|| self.selection(request.context.as_ref(), &request.tool));
        let eligible = matches!(policy, Selection::Builtin)
            && request.version == VERSION
            && !protection::recovery_reference(&request.tool.input)
            && request.tool.exit_code.is_none_or(|code| code == 0);
        let key = eligible
            .then(|| reads::Reads::key(request.host, &request.session, &selection))
            .flatten();
        let textual = reads::text_read(&request.text);
        let mut comparison = key.as_ref().filter(|_| textual).map(|key| {
            self.reads
                .borrow_mut()
                .compare(key, &selection, &request.text)
        });
        let text = request.text.clone();
        let host = request.host;
        let session = request.session.clone();
        let mut response = self.process_selected(request, &policy);
        if let Some(comparison) = &mut comparison {
            let omission = if comparison.identical {
                // Exact equality also verifies the host's ambiguous final line,
                // without promoting that line into cross-selection evidence.
                comparison.omitted.fill(true);
                Some(comparison.body.as_ref().map_or_else(
                    || {
                        format!(
                            "[outrim]: Already read {}:{}; verified unchanged, so omitted.\n",
                            selection.path, selection.label
                        )
                    },
                    |body| body.omit_exact(&selection),
                ))
            } else if comparison.omitted.iter().any(|omit| *omit) {
                comparison
                    .body
                    .as_ref()
                    .map(|body| body.omit(&selection, &comparison.omitted))
            } else {
                None
            };
            if omission.is_some() || (comparison.conflict && comparison.body.is_some()) {
                // Range conflicts deliver the complete observation, not a delta or
                // another compression. Source/config protection still permits reuse.
                response.text = text.clone();
                response.disposition = Disposition::NoMatch;
                response.applied_rule = None;
                response.original_path = None;
                response.reason = None;
                response.candidates.clear();
            }
            if let Some(body) = omission {
                self.originals
                    .omit_read(host, &session, &body, &mut response);
            }
            if let Some(key) = key {
                self.reads
                    .borrow_mut()
                    .stage(key, &selection, &response, &text, comparison);
            }
        }
        response
    }

    pub fn process(&self, request: Request) -> Response {
        let selection = self.selection(request.context.as_ref(), &request.tool);
        self.process_selected(request, &selection)
    }

    pub fn process_selected(&self, request: Request, selection: &Selection) -> Response {
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
        if matches!(selection, Selection::Off) {
            response.reason = Some("compression_disabled".into());
            return response;
        }
        if let Selection::Methods(methods) = selection {
            let mut candidates = Vec::new();
            for method in methods {
                match method {
                    Method::LogCompaction => {
                        if let Some(text) = log_compaction::selected(&response.text) {
                            candidates.push(("log_compaction", text));
                        }
                    }
                    Method::SearchHitGroups => {
                        if let Some(text) =
                            search_hits::group_search_hits(&response.text).candidate()
                        {
                            candidates.push(("search_hit_groups", text));
                        }
                    }
                    Method::PathPrefixGroups => {
                        if let Some(text) = path_list::group_prefixes(&response.text).candidate() {
                            candidates.push(("path_prefix_groups", text));
                        }
                    }
                    Method::JsonCompact | Method::JsonToon => {
                        if let Some(json) = json::selected_candidates(
                            &response.text,
                            matches!(method, Method::JsonCompact),
                            matches!(method, Method::JsonToon),
                        ) {
                            candidates.extend(json);
                        }
                    }
                }
            }
            if candidates.is_empty() {
                response.reason = Some("selected_methods_not_applicable".into());
            } else {
                self.originals
                    .compress(request.host, &request.session, candidates, &mut response);
            }
            return response;
        }
        // Search paths are query scope only when the output parses as hit records.
        let search_grouping = if matches!(request.tool.source, Source::Search) {
            search_hits::group_search_hits(&response.text)
        } else {
            Grouping::Unrecognized
        };
        let mut recognized_grouping = search_grouping.recognized();
        let search_candidate = search_grouping.candidate();
        if protection::protected(&request.tool) && search_candidate.is_none() {
            response.disposition = Disposition::Protected;
            response.reason = Some("source_or_configuration_protection".into());
            return response;
        }
        let candidates = if let Some(compacted) = log_compaction::compact(
            &request.tool,
            &response.text,
            request
                .context
                .as_ref()
                .and_then(|context| context.command.as_deref()),
        ) {
            vec![("log_compaction", compacted)]
        } else if let Some(grouped) = search_candidate {
            vec![("search_hit_groups", grouped)]
        } else if let Some(grouped) = matches!(request.tool.source, Source::Search)
            .then(|| {
                let grouping = path_list::group_prefixes(&response.text);
                recognized_grouping |= grouping.recognized();
                grouping.candidate()
            })
            .flatten()
        {
            vec![("path_prefix_groups", grouped)]
        } else if let Some(json_candidates) = json::candidates(&request.tool, &response.text) {
            json_candidates
        } else {
            response.reason = Some(
                if recognized_grouping {
                    "no_applicable_grouping"
                } else if matches!(request.tool.source, Source::Search) {
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
