//! Single-output ingress processing. The caller supplies the current user's home,
//! not an independently configurable original-output directory.

mod compression;
mod json;
mod log_tail;
mod path_list;
mod protection;
mod search_footer;
mod search_hits;

use crate::{
    gain,
    protocol::{self, Disposition, Request, Response, Source, VERSION},
    recording,
};
use std::path::Path;

pub struct Processor {
    originals: compression::Originals,
    gain: gain::Store,
    recorder: recording::Recorder,
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
        }
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
        let candidates = if let Some(tail) = log_tail::log_tail(&request.tool, &response.text) {
            vec![("log_tail", tail)]
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
            return response;
        };
        self.originals
            .compress(request.host, &request.session, candidates, &mut response);
        response
    }
}
