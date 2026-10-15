//! Provider adapters behind an injected transport. Adapters never retry; the engine owns retries.
//!
//! The rule the adapter interface encodes is the ratified "Retry and billing" rule
//! (docs/wg/overview.md; spec/protocol.md §6.1 step 7; spec/store.md §5), and how each provider's
//! outcomes map onto it is spec/adapters.md, with each adapter's reference beside its code
//! (`src/<provider>/README.md`, listed in spec/providers.md §2):
//!
//! - an adapter makes **one send** per call of [`RequestAdapter::send`] (or one submit, one
//!   collect) and reports what happened as a [`Sent`] (or [`Submitted`], [`Collected`]);
//! - [`Sent::NotReceived`] is the only outcome after which the same request may be sent again
//!   under the same attempt: the connection or the send failed, or the provider said it took
//!   nothing. It bills nothing;
//! - [`Sent::Refused`] means the call was refused before anything left (`capability_refused`,
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
//!   from), [`clock`] (polling time), [`wire`] (helpers every adapter shares), [`checks`] (the
//!   engine's checks before a send, the answer checks routes share, and every route's check of a
//!   capability),
//!   [`capabilities`] (spec/capabilities.md as data), [`routes`] (the built-in route table);
//! - one module per provider: [`openai`], [`openrouter`], [`fal`], [`tripo`], [`elevenlabs`],
//!   [`codex`] (the user's own Codex, started as a process), [`grok`] (the user's own Grok Build,
//!   spoken to over [`acp`]), each with `register(&mut Adapters, &Setup)`; and
//!   [`openai_compatible`], the family that serves each provider the planning project declares
//!   with it, registered from the setup's declarations;
//! - [`installed`]: what the adapters of programs the user installed share (finding one, its
//!   version); [`process_group`]: the process groups of their runtimes, and their end;
//! - [`live`]: the adapters of a live run, the only place the default transport is built;
//! - [`stand_in`]: the checks a stand-in's answer gets in a stand-in run (spec/protocol.md §5.7):
//!   its shape, then its route's check, from adapters built keyless over a refusing transport;
//! - [`fake`] and [`testing`] (feature `testing`): the scripted `FakeAdapter`, test setups,
//!   calls, synthetic media and test programs.
//!
//! **The engine's checks.** Every adapter here names the contract it serves
//! ([`adapter::Identity`]; spec/providers.md §2). The engine then refuses a call before the adapter
//! sees it, as spec/adapters.md §4 steps 1 and 2 say ([`checks::before_send`]: the contract, the
//! adapter's own judgement of the contract's other members, the request), and holds its answers
//! to the capability's every-route check after the adapter's own ([`checks::every_route_with`]). Each
//! adapter makes steps 1 and 2 again for a caller that hands it a call directly, through the same
//! functions, so the order its module doc gives is its whole order.
//!
//! [`Adapters`] maps a capability, a provider and a contract's `adapter` to an adapter, with a
//! default per capability and provider ([`registry`]). Without `--live` the `grida-fx` binary
//! registers none, so every uncached paid call is refused with `no_route` ("no adapter serves").

pub mod acp;
pub mod adapter;
pub mod capabilities;
pub mod chat;
pub mod checks;
pub mod clock;
pub mod codex;
pub mod elevenlabs;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod fal;
pub mod gg;
pub mod grok;
pub mod installed;
pub mod keys;
pub mod live;
pub mod openai;
pub mod openai_compatible;
pub mod openrouter;
pub mod process_group;
pub mod redact;
pub mod registry;
pub mod routes;
pub mod setup;
pub mod stand_in;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod tripo;
pub mod wire;

pub use adapter::{
    Adapter, Answer, AnsweredFile, CallRequest, Collected, LongJob, RequestAdapter, RequestFile,
    RouteLimits, RouteRef, Sent, Submitted,
};
pub use keys::{KeyName, Keys, Secret};
pub use registry::Adapters;
pub use setup::{Endpoints, Setup};
pub use stand_in::{StandInChecks, StandInFile};
pub use transport::{HttpRequest, HttpResponse, Transport, TransportError};

use std::future::Future;
use std::pin::Pin;

/// A boxed, sendable future: what the dyn-compatible adapter traits return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
