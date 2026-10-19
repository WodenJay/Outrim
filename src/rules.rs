use crate::protocol::{Source, Tool};

#[derive(Clone, Copy)]
pub(crate) enum AppliedRule {
    LogTail,
    SearchHitGroups,
}

impl AppliedRule {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::LogTail => "log_tail",
            Self::SearchHitGroups => "search_hit_groups",
        }
    }
}

pub(crate) fn compress_candidate(tool: &Tool, text: &str) -> Option<(AppliedRule, String)> {
    match tool.source {
        Source::BuildLog | Source::TestLog | Source::RuntimeLog => {
            Some((AppliedRule::LogTail, log_tail(text)))
        }
        Source::Search => {
            crate::search::group_search_hits(text).map(|text| (AppliedRule::SearchHitGroups, text))
        }
        _ => None,
    }
}

fn log_tail(text: &str) -> String {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    lines[lines.len().saturating_sub(10)..].concat()
}
