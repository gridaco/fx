//! Provider adapters behind an injected transport. Adapters never retry; the engine owns retries.
//!
//! Step 3 defines the interface the engine's retry owner drives (`grida_fx_runtime::calls::retry`)
//! and nothing that talks to a provider: real adapters, and the transport they share, arrive in
//! step 4. The rule this interface encodes is the ratified "Retry and billing" rule
//! (docs/wg/overview.md; spec/protocol.md §6.1 step 7; spec/store.md §5):
//!
//! - an adapter makes **one send** per call of [`RequestAdapter::send`] (or one submit, one
//!   collect) and reports what happened as a [`Sent`] (or [`Submitted`], [`Collected`]);
//! - [`Sent::NotReceived`] is the only outcome after which the same request may be sent again
//!   under the same attempt: the connection or the send failed, or the provider said it took
//!   nothing. It bills nothing;
//! - [`Sent::Refused`] means the adapter refused before anything left (`capability_refused`,
//!   settled at $0, never retried);
//! - [`Sent::Answered`] carries the answer and the cost the provider reported, if any. The
//!   adapter's [`RequestAdapter::check`] then runs; a refusal fails the attempt **as billed**;
//! - [`Sent::Failed`] means the provider received the request: the attempt is billed (the
//!   reported cost, else the whole hold) and the engine may make a new, reserved attempt;
//! - a long job is submitted once ([`LongJob::submit`]) and then only collected by its handle
//!   ([`LongJob::collect`]); it is never submitted again while its job record says `submitted`.
//!
//! [`Adapters`] maps a capability and a provider to an adapter. The `grida-fx` binary of step 3
//! registers none, so every uncached live call is refused with `no_route` ("no adapter serves").
//! Tests use [`fake::FakeAdapter`] (feature `testing`), which plays a script of outcomes.

pub mod adapter;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod registry;

pub use adapter::{
    Adapter, Answer, AnsweredFile, CallRequest, Collected, LongJob, RequestAdapter, RequestFile,
    RouteRef, Sent, Submitted,
};
pub use registry::Adapters;

use std::future::Future;
use std::pin::Pin;

/// A boxed, sendable future: what the dyn-compatible adapter traits return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
