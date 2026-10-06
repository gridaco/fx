//! Provider adapters behind an injected transport. Adapters never retry; the engine owns retries.
//!
//! The rule the adapter interface encodes is the ratified "Retry and billing" rule
//! (docs/wg/overview.md; spec/protocol.md §6.1 step 7; spec/store.md §5), and how each provider's
//! outcomes map onto it is spec/providers.md:
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
//! Layout:
//! - [`transport`]: the [`Transport`] trait, its request and response types, the default
//!   reqwest transport ([`transport::http`]), and for tests the replay transport and `NoNetwork`;
//! - [`keys`] (the allowlisted key loader), [`redact`], [`setup`] (what adapters are built
//!   from), [`clock`] (polling time), [`wire`] (helpers every adapter shares),
//!   [`capabilities`] (spec/capabilities.md as data), [`routes`] (the built-in route table);
//! - one module per provider: [`openai`], [`openrouter`], [`fal`], [`tripo`], [`elevenlabs`],
//!   each with `register(&mut Adapters, &Setup)`;
//! - [`live`]: the adapters of a live run, the only place the default transport is built;
//! - [`fake`] and [`testing`] (feature `testing`): the scripted `FakeAdapter`, test setups,
//!   calls and synthetic media.
//!
//! [`Adapters`] maps a capability and a provider to an adapter. Without `--live` the `grida-fx`
//! binary registers none, so every uncached paid call is refused with `no_route` ("no adapter
//! serves").

pub mod adapter;
pub mod capabilities;
pub mod clock;
pub mod elevenlabs;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod fal;
pub mod keys;
pub mod live;
pub mod openai;
pub mod openrouter;
pub mod redact;
pub mod registry;
pub mod routes;
pub mod setup;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod tripo;
pub mod wire;

pub use adapter::{
    Adapter, Answer, AnsweredFile, CallRequest, Collected, LongJob, RequestAdapter, RequestFile,
    RouteRef, Sent, Submitted,
};
pub use keys::{KeyName, Keys, Secret};
pub use registry::Adapters;
pub use setup::{Endpoints, Setup};
pub use transport::{HttpRequest, HttpResponse, Transport, TransportError};

use std::future::Future;
use std::pin::Pin;

/// A boxed, sendable future: what the dyn-compatible adapter traits return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
