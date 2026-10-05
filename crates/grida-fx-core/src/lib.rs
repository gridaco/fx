//! The offline core of Grida FX: documents, expressions, expansion, identity, planning and pricing.
//!
//! The core is synchronous and pure: it reads project files and nothing else. It never spawns a
//! process or opens a connection. Node hosts reach it through the [`host::NodeHost`] trait, which
//! `grida-fx-runtime` implements; tests use a fake. The module map, data flow and conventions are
//! in the step-2 ARCHITECTURE notes; each module's doc names the spec sections it follows.

pub mod builtins;
pub mod docs;
pub mod error;
pub mod expand;
pub mod expr;
pub mod facts;
pub mod host;
pub mod inputs;
pub mod kinds;
pub mod lock;
pub mod money;
pub mod plan;
pub mod project;
pub mod registry;
pub mod routes;
pub mod spec;
pub mod text;
pub mod val;
pub mod value;
pub mod yaml;

pub use error::{Error, ErrorKind, Problem, Result};

/// The engine's name and version, as `initialize` and messages report them.
pub const ENGINE_NAME: &str = "grida-fx";
/// The engine's version.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
