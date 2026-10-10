//! The served report, `fx-layout-report-v1` (spec/layout.md §6.11,
//! spec/schemas/fx-layout-report-v1.schema.json). Field order is the schema's.

use indexmap::IndexMap;
use serde::Serialize;

pub const KIND: &str = "fx-layout-report-v1";

/// One answer for a run or a saved plan: every slot's cell and the members' order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutReport {
    pub kind: &'static str,
    /// The derived layout file (Ship B); null in Ship A.
    pub file: Option<String>,
    /// The digest of the layout file read (Ship B); null in Ship A.
    pub revision: Option<String>,
    /// `applied`, `none` or `refused`; always `none` in Ship A.
    pub state: &'static str,
    /// The observation cursor of the record prefix the report reflects; null for a plan.
    pub cursor: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    /// Every slot's address, container by container in order key.
    pub cells: IndexMap<String, CellEntry>,
    /// Every member (instance and scope ids) in order key (§4 rule 4).
    pub order: Vec<String>,
}

/// A structured layout diagnostic (§6.10). Ship A reports none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellEntry {
    pub column: u32,
    pub row: u32,
    /// `automatic`, or `authored` from Ship B.
    pub source: &'static str,
}
