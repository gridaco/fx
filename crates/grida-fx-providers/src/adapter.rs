//! The adapter interface (crate doc; spec/protocol.md §6.1; spec/store.md §5).
//!
//! Every type here is data the engine hands an adapter or gets back. An adapter never sees a
//! budget, a hold, the store or a job record: the engine's retry owner reserves, records and
//! settles around each attempt, and writes the job record of a long job from what
//! [`LongJob::submit`] and [`LongJob::collect`] report.
//!
//! Credentials never appear here: a request is the canonical request (spec/identity.md §9), and
//! keys travel in the transport (step 4).

use crate::BoxFuture;
use grida_fx_core::money::Usd;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// The route a call goes to (spec/identity.md §7): what an adapter needs to address the
/// provider. Prices, features and pacing are the engine's business and are not here.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRef {
    pub capability: String,
    pub model: String,
    pub provider: String,
    /// The route's declared contract object; `{}` when none.
    pub contract: Value,
}

impl RouteRef {
    /// `model@provider`.
    pub fn id(&self) -> String {
        format!("{}@{}", self.model, self.provider)
    }
}

/// A file the request names by digest (`{"file": "<digest>"}`), readable at `path` in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestFile {
    pub digest: String,
    pub kind: String,
    pub size: u64,
    /// The store copy. Read only; never written, moved or kept in a record.
    pub path: PathBuf,
}

/// One attempt of one call, as the adapter sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRequest {
    pub route: RouteRef,
    /// The canonical request (spec/identity.md §9): files as `{"file": digest}`.
    pub request: Value,
    /// Every file the request names, by digest.
    pub files: IndexMap<String, RequestFile>,
    /// One take number per regenerating level, outermost first.
    pub take: Vec<u32>,
    /// The call key (spec/identity.md §9), for the adapter's own logs.
    pub key: String,
    /// The attempt, from 1 to 6.
    pub attempt: u32,
}

/// A file an answer carries. The engine stores the bytes; a provider's URL is never kept
/// (spec/store.md §7).
#[derive(Debug, Clone, PartialEq)]
pub struct AnsweredFile {
    /// The file kind (spec/identity.md §4).
    pub kind: String,
    pub bytes: Vec<u8>,
}

/// What a provider answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The files, by name (`image`, `video`, …), in the provider's order.
    pub files: IndexMap<String, AnsweredFile>,
    /// What else the call returned, as JSON; `null` when nothing.
    pub data: Value,
    /// The cost the provider reported; `None` when it reported none (the engine then charges the
    /// whole hold).
    pub cost: Option<Usd>,
}

impl Answer {
    /// An answer with `data`, no files, and the reported `cost`.
    pub fn new(data: Value, cost: Option<Usd>) -> Answer {
        Answer {
            files: IndexMap::new(),
            data,
            cost,
        }
    }

    /// The same answer with one more file under `name` (a later file of the same name replaces
    /// it, keeping its place).
    pub fn with_file(mut self, name: &str, kind: &str, bytes: Vec<u8>) -> Answer {
        self.files.insert(
            name.to_string(),
            AnsweredFile {
                kind: kind.to_string(),
                bytes,
            },
        );
        self
    }
}

/// The outcome of one send of a plain request.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// Provably not received: the connection or the send failed, or the provider said it took
    /// nothing. The engine may send the same request again under the same attempt, at $0.
    NotReceived { reason: String },
    /// Refused before anything was sent (a missing key, a request the route cannot serve).
    /// `capability_refused`: settled at $0, never retried.
    Refused { reason: String },
    /// The provider answered. The adapter's `check` runs next.
    Answered(Answer),
    /// The provider received the request and the call failed. Billed: the reported cost, else the
    /// whole hold. `retryable: false` ends the call (`call_failed`) without a new attempt.
    Failed {
        reason: String,
        cost: Option<Usd>,
        retryable: bool,
    },
}

/// The outcome of submitting a long job once.
#[derive(Debug, Clone, PartialEq)]
pub enum Submitted {
    /// Provably not received; may be submitted again under the same attempt, at $0.
    NotReceived { reason: String },
    /// Refused before anything was sent: `capability_refused`, $0.
    Refused { reason: String },
    /// The provider took the job. `handle` is what collecting needs (never a credential or a
    /// signed URL); the engine records the job as `submitted`.
    Accepted { handle: Value },
    /// Nobody can say whether the provider took the job (a timeout after the request left). The
    /// job record stays `submitting`; the call fails `job_unsettled` until a person forgets it.
    Uncertain { reason: String },
    /// The provider received the submission and refused or failed it: billed, and nothing of it
    /// is outstanding (the record becomes `settled`).
    Failed {
        reason: String,
        cost: Option<Usd>,
        retryable: bool,
    },
}

/// The outcome of collecting a long job by its handle. Collecting bills nothing new.
#[derive(Debug, Clone, PartialEq)]
pub enum Collected {
    /// The job's answer.
    Answered(Answer),
    /// The job ended without a result: the record becomes `settled`, the call fails
    /// `call_failed`, and a later run submits it anew.
    Ended { reason: String },
    /// Collecting failed and the job may still be running: the record stays `submitted`, the
    /// call fails `call_failed`, and a later run collects again.
    Unreachable { reason: String },
}

/// An adapter for calls answered within one request.
pub trait RequestAdapter: Send + Sync {
    /// Sends the request once. Never retries.
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent>;

    /// Checks an answer before the engine accepts it (a signature, an opacity, a schema). A
    /// refusal is a sentence; the attempt is then failed as billed.
    fn check(&self, _call: &CallRequest, _answer: &Answer) -> Result<(), String> {
        Ok(())
    }
}

/// An adapter for long provider jobs (a video, a rig): submitted once, collected by handle.
pub trait LongJob: Send + Sync {
    /// Submits the job once. Never retries.
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted>;

    /// Collects a submitted job by its handle, waiting for it as long as the adapter's own
    /// deadline allows. Never submits.
    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected>;

    /// As [`RequestAdapter::check`], on a collected answer.
    fn check(&self, _call: &CallRequest, _answer: &Answer) -> Result<(), String> {
        Ok(())
    }
}

/// One registered adapter.
#[derive(Clone)]
pub enum Adapter {
    Request(Arc<dyn RequestAdapter>),
    Job(Arc<dyn LongJob>),
}

impl std::fmt::Debug for Adapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Adapter::Request(_) => f.write_str("Adapter::Request"),
            Adapter::Job(_) => f.write_str("Adapter::Job"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_route_is_named_model_at_provider() {
        let route = RouteRef {
            capability: "image.generate".into(),
            model: "img-a".into(),
            provider: "acme".into(),
            contract: json!({}),
        };
        assert_eq!(route.id(), "img-a@acme");
    }

    #[test]
    fn answers_are_built_with_their_files_in_order() {
        let answer = Answer::new(json!({"n": 1}), Some(Usd(20_000)))
            .with_file("image", "image/png", vec![1])
            .with_file("mask", "image/png", vec![2])
            .with_file("image", "image/webp", vec![3]);
        assert_eq!(answer.data, json!({"n": 1}));
        assert_eq!(answer.cost, Some(Usd(20_000)));
        let names: Vec<&str> = answer.files.keys().map(String::as_str).collect();
        assert_eq!(names, ["image", "mask"]);
        assert_eq!(
            answer.files["image"],
            AnsweredFile {
                kind: "image/webp".into(),
                bytes: vec![3],
            }
        );
        assert!(Answer::new(Value::Null, None).files.is_empty());
    }

    struct Silent;

    impl RequestAdapter for Silent {
        fn send<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Sent> {
            Box::pin(async {
                Sent::Refused {
                    reason: "silent".into(),
                }
            })
        }
    }

    impl LongJob for Silent {
        fn submit<'a>(&'a self, _call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
            Box::pin(async {
                Submitted::Refused {
                    reason: "silent".into(),
                }
            })
        }

        fn collect<'a>(
            &'a self,
            _call: &'a CallRequest,
            _handle: &'a Value,
        ) -> BoxFuture<'a, Collected> {
            Box::pin(async {
                Collected::Ended {
                    reason: "silent".into(),
                }
            })
        }
    }

    #[test]
    fn the_default_checks_pass_and_adapters_debug_by_kind() {
        let call = CallRequest {
            route: RouteRef {
                capability: "video.generate".into(),
                model: "vid-a".into(),
                provider: "acme".into(),
                contract: json!({}),
            },
            request: json!({}),
            files: IndexMap::new(),
            take: vec![1],
            key: "0".repeat(64),
            attempt: 1,
        };
        let answer = Answer::new(json!({"bad": true}), None);
        assert_eq!(RequestAdapter::check(&Silent, &call, &answer), Ok(()));
        assert_eq!(LongJob::check(&Silent, &call, &answer), Ok(()));
        assert_eq!(
            format!("{:?}", Adapter::Request(Arc::new(Silent))),
            "Adapter::Request"
        );
        assert_eq!(
            format!("{:?}", Adapter::Job(Arc::new(Silent))),
            "Adapter::Job"
        );
    }
}
