//! Optional evidence accompanying an adapter failure; never an execution outcome.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSource {
    Engine,
    Adapter,
    Transport,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Validate,
    Upload,
    Send,
    Submit,
    Poll,
    Collect,
    Download,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticDetail {
    Available,
    #[default]
    Unavailable,
    Unreadable,
    Withheld,
}

/// Safe, bounded provider evidence. The engine sanitizes every string before recording it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterDiagnostic {
    pub source: DiagnosticSource,
    pub phase: DiagnosticPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default)]
    pub detail: DiagnosticDetail,
    #[serde(default)]
    pub redacted: bool,
    #[serde(default)]
    pub truncated: bool,
}

impl AdapterDiagnostic {
    pub fn new(source: DiagnosticSource, phase: DiagnosticPhase) -> Self {
        Self {
            source,
            phase,
            operation: None,
            http_status: None,
            code: None,
            field: None,
            message: None,
            request_id: None,
            task_id: None,
            detail: DiagnosticDetail::Unavailable,
            redacted: false,
            truncated: false,
        }
    }
}
