//! Running a planned workflow: the store, the call cache, budgets, the retry owner and the node
//! hosts.
//!
//! Step 2 has only [`host`]: the Python node host process and its JSON-RPC session
//! (spec/protocol.md), implementing `grida_fx_core::host::NodeHost` for `describe` and `build`;
//! and [`tools`], resolving external programs (`GRIDA_FX_TOOL_<NAME>`, then `PATH`). The session
//! is built for step 3's `run`, where requests flow both ways.

pub mod host;
pub mod tools;
