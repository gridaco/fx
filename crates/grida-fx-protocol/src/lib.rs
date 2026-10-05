//! The FX node protocol, `fx-node-protocol-v1` (spec/protocol.md and
//! spec/schemas/fx-node-protocol-v1.schema.json): JSON-RPC 2.0 over stdio, framed as in LSP.
//!
//! This crate is data and framing only: it never spawns a process and never computes an
//! identity. [`framing`] reads and writes `Content-Length` frames as bytes; the receiver parses
//! the bytes with the engine's strict I-JSON reader (`grida_fx_core::value::parse_json`) and
//! then deserializes the typed messages in [`jsonrpc`] and [`types`]. Every type here mirrors a
//! `$defs` entry of the schema with the same `lower_snake_case` field names.

pub mod framing;
pub mod jsonrpc;
pub mod run_types;
pub mod types;

pub use jsonrpc::{ErrorCode, Id, Message, RpcError};
pub use types::*;

/// The protocol this engine speaks (protocol.md §2).
pub const PROTOCOL: &str = "fx-node-protocol-v1";

/// Method names (protocol.md §2, §5, §6).
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const SHUTDOWN: &str = "shutdown";
    pub const EXIT: &str = "exit";
    pub const DESCRIBE: &str = "describe";
    pub const BUILD: &str = "build";
    pub const RUN: &str = "run";
    pub const TOOL_INVOKE: &str = "tool.invoke";
    pub const AGENT_CHECK: &str = "agent.check";
    pub const CANCEL: &str = "$/cancel";
    pub const CAPABILITY: &str = "capability";
    pub const AGENT_RUN: &str = "agent.run";
    pub const FACT: &str = "fact";
    pub const ANNOTATE: &str = "annotate";
    pub const PROGRESS: &str = "progress";
    pub const PROMPT_RENDER: &str = "prompt.render";
    pub const FILE_PUT: &str = "file.put";
}
