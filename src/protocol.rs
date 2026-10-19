use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub id: String,
    pub host: Host,
    pub session: String,
    pub tool: Tool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Pi,
    Opencode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub input: Value,
    pub source: Source,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    SourceCode,
    Configuration,
    Search,
    BuildLog,
    TestLog,
    RuntimeLog,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub id: String,
    pub text: String,
    pub disposition: Disposition,
    pub applied_rule: Option<String>,
    pub original_path: Option<String>,
}

/// A transport/schema error has no processed text. Valid processing requests
/// always return `Response`, including failures that pass the original through.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub version: u32,
    pub id: Option<String>,
    pub error: ProtocolError,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolError {
    InvalidJson,
    InvalidRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Compressed,
    Protected,
    RecoverySkipped,
    NotShorter,
    NoMatch,
    FailedOpen,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordDeliveryRequest {
    #[serde(rename = "type")]
    pub message_type: DeliveryMessageType,
    pub version: u32,
    pub id: String,
    pub disposition: Disposition,
    pub applied_rule: Option<String>,
    pub before_chars: u64,
    pub after_chars: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMessageType {
    RecordDelivery,
}

#[derive(Debug, Serialize)]
pub struct DeliveryAcknowledgement {
    pub version: u32,
    pub id: String,
    pub acknowledged: bool,
}
