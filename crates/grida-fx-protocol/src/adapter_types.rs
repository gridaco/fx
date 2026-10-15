//! A project adapter's messages (protocol.md §5.8, §6.8; adapters.md §9): `adapter.load`, which
//! the planning host and an adapter host answer, the calls an adapter host answers
//! (`adapter.send`, `adapter.submit`, `adapter.collect`), and the `fetch` it sends back.
//!
//! One serde type per `$defs` entry of fx-node-protocol-v1, field names as in the schema:
//! `adapter_target` (and `adapter_load_params`), `adapter_load_result`, `adapter_route`,
//! `adapter_file`, `adapter_send_params` (and `adapter_submit_params`), `adapter_collect_params`,
//! `adapter_usage`, `adapter_send_result`, `adapter_submit_result`, `adapter_collect_result` and
//! their `outcome_*` members, `http_header`, `fetch_body`, `fetch_params` and `fetch_result`.
//! The conventions are [`crate::types`]', with one more: an adapter's outcomes share members
//! (`reason`, `cost_usd`), so each is named by its `outcome` member, an internally tagged enum,
//! where every other `oneOf` of the protocol is told apart by its members.
//!
//! The schema says more than serde can: a non-blank `reason`, a `cost_usd` of at least 0, an
//! `attempt` from 1 to 6, no `body` on a `GET`, and the caps of `fetch`. The engine checks those
//! itself when it reads a message.

use crate::run_types::StandInFileWire;
use crate::types::{ClosureEntry, nullable};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A project adapter: its module, POSIX, relative to the project root, and the module-level name
/// it is declared under (`adapter_target`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterTarget {
    pub path: String,
    pub attribute: String,
}

/// `adapter.load` params (protocol.md §5.8): the adapter to load.
pub type AdapterLoadParams = AdapterTarget;

/// How a capability is served (protocol.md §5.8): sent once per send, or submitted and collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallKind {
    /// `adapter.send`.
    Request,
    /// `adapter.submit` and `adapter.collect`.
    Job,
}

/// What an adapter declares: its version, and the capabilities it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterDeclaration {
    /// `null` for an adapter without a version, whose behaviour is its source. Required, even
    /// when null.
    #[serde(deserialize_with = "nullable")]
    pub version: Option<u64>,
    /// Each capability it serves, in the order it declares them. The engine refuses an empty map
    /// (adapters.md §9).
    pub capabilities: IndexMap<String, CallKind>,
}

/// `adapter.load` result (protocol.md §5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterLoadResult {
    pub adapter: AdapterDeclaration,
    /// The module's source closure, as `describe` finds a node module's (protocol.md §5.1).
    pub closure: Vec<ClosureEntry>,
}

/// The route of an adapter call (identity.md §7), with its contract as its table's reading
/// completed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterRoute {
    /// `model@provider`.
    pub id: String,
    pub model: String,
    pub provider: String,
    pub contract: IndexMap<String, Value>,
}

/// A file an adapter call's request names (`adapter_file`): lighter than a [`crate::FileRef`],
/// with no name and no file facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterFile {
    pub digest: String,
    pub kind: String,
    pub size: u64,
    /// The store copy's absolute path. The host reads it, and never writes, moves or deletes it.
    pub path: String,
}

/// `adapter.send` and `adapter.submit` params (protocol.md §5.8): one call, sent or submitted
/// once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterCallParams {
    /// Unique in the session; the `fetch` requests made for this call carry it.
    pub call_id: String,
    pub adapter: AdapterTarget,
    pub capability: String,
    pub route: AdapterRoute,
    /// The declaration's base URL, normalized: its variable's value when set, else its default.
    pub base_url: String,
    /// The canonical request, exactly as keyed; files as file values.
    pub request: IndexMap<String, Value>,
    /// An [`AdapterFile`] for every file value in `request`, by digest.
    pub files: IndexMap<String, AdapterFile>,
    /// The call's take list, as in the call key.
    pub take: Vec<u64>,
    /// The call key.
    pub key: String,
    /// From 1 to 6.
    pub attempt: u32,
}

/// `adapter.send` params.
pub type AdapterSendParams = AdapterCallParams;

/// `adapter.submit` params.
pub type AdapterSubmitParams = AdapterCallParams;

/// `adapter.collect` params (protocol.md §5.8): the call, and the handle its submit was accepted
/// with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterCollectParams {
    pub call_id: String,
    pub adapter: AdapterTarget,
    pub capability: String,
    pub route: AdapterRoute,
    pub base_url: String,
    pub request: IndexMap<String, Value>,
    pub files: IndexMap<String, AdapterFile>,
    pub take: Vec<u64>,
    pub key: String,
    pub attempt: u32,
    /// The job's handle, as `adapter.submit` accepted it.
    pub handle: IndexMap<String, Value>,
}

impl AdapterCollectParams {
    /// The params that collect `call`'s job by `handle`.
    pub fn new(call: AdapterCallParams, handle: IndexMap<String, Value>) -> AdapterCollectParams {
        AdapterCollectParams {
            call_id: call.call_id,
            adapter: call.adapter,
            capability: call.capability,
            route: call.route,
            base_url: call.base_url,
            request: call.request,
            files: call.files,
            take: call.take,
            key: call.key,
            attempt: call.attempt,
            handle,
        }
    }
}

/// What a call consumed besides dollars, as the provider reported it (`adapter_usage`; store.md
/// §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

/// An answer an adapter sent or collected (`outcome_answered`, without its tag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterAnswer {
    /// The answer's files by name: bytes, and a kind unless the capability names one.
    pub files: IndexMap<String, StandInFileWire>,
    /// JSON, or `null`. Required.
    pub data: Value,
    /// The cost the provider reported, in US dollars; `None` when it reported none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AdapterUsage>,
}

/// `adapter.send` result (adapters.md §3.1, §9): one outcome, named by `outcome`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterSendResult {
    Answered(AdapterAnswer),
    NotReceived {
        reason: String,
        /// The wait the provider asked for, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_s: Option<f64>,
    },
    Refused {
        reason: String,
    },
    Failed {
        reason: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
}

/// `adapter.submit` result (adapters.md §6, §9): one outcome, named by `outcome`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterSubmitResult {
    /// The provider took the job; the handle is what collecting needs.
    Accepted {
        handle: IndexMap<String, Value>,
    },
    Uncertain {
        reason: String,
    },
    NotReceived {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_s: Option<f64>,
    },
    Refused {
        reason: String,
    },
    Failed {
        reason: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
}

/// `adapter.collect` result (adapters.md §6, §9): one outcome, named by `outcome`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterCollectResult {
    Answered(AdapterAnswer),
    Ended {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
    Unreachable {
        reason: String,
    },
}

/// One HTTP header: its name, lowercase, and its value (`http_header`).
pub type HttpHeader = (String, String);

/// The method of a `fetch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FetchMethod {
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
}

/// What a `fetch` is for (adapters.md §2 item 5): it decides whether a credential may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FetchLane {
    Provider,
    Upload,
    Download,
}

/// A `fetch` body (`fetch_body`): bytes with their content type, or a JSON value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum FetchBody {
    /// The bytes, base64 with padding.
    Bytes {
        base64: String,
        content_type: String,
    },
    /// Sent as compact UTF-8 with `content-type: application/json`.
    Json { json: Value },
}

/// `fetch` params (protocol.md §6.8): one exchange for a pending adapter call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchParams {
    /// The `call_id` of a pending `adapter.send`, `adapter.submit` or `adapter.collect`.
    pub call_id: String,
    pub method: FetchMethod,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<HttpHeader>>,
    /// Only on a `POST`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<FetchBody>,
    pub lane: FetchLane,
    /// `true` asks the engine to attach the declaration's key; `None` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<bool>,
    /// Above 0 and at most 300 seconds, the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<f64>,
    /// From 1 to 64 MiB, the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
}

/// When a `fetch` failed (protocol.md §6.8; adapters.md §2 item 7, §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchPhase {
    /// The engine or its transport refused the request before it left.
    Refused,
    /// It provably never left.
    NotSent,
    /// It may have been received.
    AfterSend,
}

/// Why a `fetch` failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchFailure {
    pub phase: FetchPhase,
    /// Never a URL's path or query, a header value or a body.
    pub reason: String,
}

/// `fetch` result (protocol.md §6.8): the response, or why the exchange failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum FetchResult {
    Response {
        status: u16,
        /// Names lowercase, in order.
        headers: Vec<HttpHeader>,
        /// The body, base64 with padding.
        base64: String,
    },
    Failed {
        error: FetchFailure,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    const DIGEST: &str = "ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859";

    fn round_trip<T: DeserializeOwned + Serialize>(value: Value) -> T {
        let typed: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), value);
        typed
    }

    fn refused<T: DeserializeOwned + std::fmt::Debug>(value: Value) {
        let read = serde_json::from_value::<T>(value.clone());
        assert!(read.is_err(), "{value} read as {read:?}");
    }

    fn call() -> Value {
        json!({
            "call_id": "c1",
            "adapter": {"path": "adapters/acme.py", "attribute": "acme"},
            "capability": "image.generate",
            "route": {"id": "img-a@acme", "model": "img-a", "provider": "acme",
                      "contract": {"adapter": "adapters/acme.py#acme", "adapter_behavior": 1}},
            "base_url": "https://api.acme.test/v1",
            "request": {"prompt": "a kite", "references": [{"file": DIGEST}]},
            "files": {DIGEST: {"digest": DIGEST, "kind": "image/png", "size": 136,
                               "path": "/work/acme/.fx/cache/files/ad/ad5e"}},
            "take": [1],
            "key": DIGEST,
            "attempt": 2
        })
    }

    #[test]
    fn loading_an_adapter() {
        round_trip::<AdapterLoadParams>(json!({"path": "adapters/acme.py", "attribute": "acme"}));
        refused::<AdapterLoadParams>(json!({"path": "adapters/acme.py"}));
        let loaded = round_trip::<AdapterLoadResult>(json!({
            "adapter": {"version": null, "capabilities": {"video.generate": "job", "image.generate": "request"}},
            "closure": [{"label": "adapters/acme.py", "path": "/work/acme/adapters/acme.py"}]
        }));
        assert_eq!(loaded.adapter.version, None);
        // The order the adapter declares is kept.
        let kinds: Vec<_> = loaded.adapter.capabilities.values().copied().collect();
        assert_eq!(kinds, [CallKind::Job, CallKind::Request]);
        round_trip::<AdapterLoadResult>(json!({
            "adapter": {"version": 3, "capabilities": {}}, "closure": []
        }));
        // The version is required, even when null.
        refused::<AdapterLoadResult>(json!({"adapter": {"capabilities": {}}, "closure": []}));
        refused::<AdapterLoadResult>(json!({
            "adapter": {"version": 1, "capabilities": {"image.generate": "stream"}}, "closure": []
        }));
    }

    #[test]
    fn calls_and_collects() {
        let sent = round_trip::<AdapterCallParams>(call());
        assert_eq!(sent.files[DIGEST].size, 136);
        let mut with_handle = call();
        with_handle["handle"] = json!({"job": "j-1"});
        refused::<AdapterCallParams>(with_handle.clone());
        let collect = round_trip::<AdapterCollectParams>(with_handle.clone());
        assert_eq!(
            serde_json::to_value(AdapterCollectParams::new(sent, collect.handle.clone())).unwrap(),
            with_handle
        );
        refused::<AdapterCollectParams>(call());
        let mut handle = call();
        handle["handle"] = json!(["j-1"]);
        refused::<AdapterCollectParams>(handle);
    }

    #[test]
    fn outcomes_are_named_by_their_outcome() {
        let answered = round_trip::<AdapterSendResult>(json!({
            "outcome": "answered",
            "files": {"image": {"base64": "iVBORw0KGgo=", "kind": "image/png"}},
            "data": null,
            "cost_usd": 0.04,
            "usage": {"input_tokens": 12}
        }));
        assert!(matches!(answered, AdapterSendResult::Answered(a) if a.cost_usd == Some(0.04)));
        round_trip::<AdapterSendResult>(
            json!({"outcome": "answered", "files": {}, "data": {"json": 1}}),
        );
        round_trip::<AdapterSendResult>(
            json!({"outcome": "not_received", "reason": "busy", "retry_after_s": 2.5}),
        );
        round_trip::<AdapterSendResult>(json!({"outcome": "refused", "reason": "blank prompt"}));
        round_trip::<AdapterSendResult>(
            json!({"outcome": "failed", "reason": "acme returned HTTP 500", "retryable": true}),
        );
        round_trip::<AdapterSendResult>(
            json!({"outcome": "failed", "reason": "acme returned HTTP 400", "retryable": false, "cost_usd": 0.0}),
        );
        // No tag, another method's tag, a member of another outcome, or no data.
        refused::<AdapterSendResult>(json!({"files": {}, "data": null}));
        refused::<AdapterSendResult>(json!({"outcome": "accepted", "handle": {}}));
        refused::<AdapterSendResult>(
            json!({"outcome": "refused", "reason": "x", "retryable": false}),
        );
        refused::<AdapterSendResult>(json!({"outcome": "answered", "files": {}}));
        refused::<AdapterSendResult>(json!({"outcome": "failed", "reason": "x"}));

        let accepted = round_trip::<AdapterSubmitResult>(
            json!({"outcome": "accepted", "handle": {"job": "j-1"}}),
        );
        assert!(
            matches!(accepted, AdapterSubmitResult::Accepted { handle } if handle["job"] == "j-1")
        );
        round_trip::<AdapterSubmitResult>(json!({"outcome": "uncertain", "reason": "timed out"}));
        round_trip::<AdapterSubmitResult>(json!({"outcome": "not_received", "reason": "busy"}));
        round_trip::<AdapterSubmitResult>(json!({"outcome": "refused", "reason": "no frame"}));
        round_trip::<AdapterSubmitResult>(
            json!({"outcome": "failed", "reason": "x", "retryable": false, "cost_usd": 1.5}),
        );
        refused::<AdapterSubmitResult>(json!({"outcome": "accepted", "handle": "j-1"}));
        refused::<AdapterSubmitResult>(json!({"outcome": "answered", "files": {}, "data": null}));

        round_trip::<AdapterCollectResult>(
            json!({"outcome": "answered", "files": {}, "data": null}),
        );
        round_trip::<AdapterCollectResult>(
            json!({"outcome": "ended", "reason": "cancelled", "cost_usd": 0.0}),
        );
        round_trip::<AdapterCollectResult>(
            json!({"outcome": "unreachable", "reason": "still running"}),
        );
        refused::<AdapterCollectResult>(json!({"outcome": "uncertain", "reason": "x"}));
    }

    #[test]
    fn fetches() {
        let get = round_trip::<FetchParams>(json!({
            "call_id": "c1", "method": "GET", "url": "https://api.acme.test/v1/jobs/j-1",
            "headers": [["accept", "application/json"]], "lane": "provider", "credential": true,
            "timeout_s": 30.0, "max_response_bytes": 1024
        }));
        assert_eq!(get.method, FetchMethod::Get);
        round_trip::<FetchParams>(json!({
            "call_id": "c1", "method": "POST", "url": "https://api.acme.test/v1/draw",
            "body": {"json": {"prompt": "a kite"}}, "lane": "provider"
        }));
        let upload = round_trip::<FetchParams>(json!({
            "call_id": "c1", "method": "POST", "url": "https://api.acme.test/v1/files",
            "body": {"base64": "AAE=", "content_type": "image/png"}, "lane": "upload"
        }));
        assert!(matches!(upload.body, Some(FetchBody::Bytes { .. })));
        refused::<FetchParams>(
            json!({"call_id": "c1", "method": "PUT", "url": "u", "lane": "provider"}),
        );
        refused::<FetchParams>(
            json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "cdn"}),
        );
        refused::<FetchParams>(
            json!({"run_id": "r1", "method": "GET", "url": "u", "lane": "provider"}),
        );
        refused::<FetchParams>(json!({
            "call_id": "c1", "method": "GET", "url": "u", "lane": "provider", "headers": [["a"]]
        }));
        refused::<FetchParams>(json!({
            "call_id": "c1", "method": "POST", "url": "u", "lane": "provider",
            "body": {"json": 1, "base64": "AA=="}
        }));

        let response = round_trip::<FetchResult>(json!({
            "status": 200, "headers": [["content-type", "application/json"]], "base64": "e30="
        }));
        assert!(matches!(
            response,
            FetchResult::Response { status: 200, .. }
        ));
        let failed = round_trip::<FetchResult>(
            json!({"error": {"phase": "refused", "reason": "the network is off (GRIDA_FX_NETWORK=off)"}}),
        );
        assert!(
            matches!(failed, FetchResult::Failed { error } if error.phase == FetchPhase::Refused)
        );
        round_trip::<FetchResult>(
            json!({"error": {"phase": "not_sent", "reason": "no connection"}}),
        );
        round_trip::<FetchResult>(json!({"error": {"phase": "after_send", "reason": "reset"}}));
        refused::<FetchResult>(json!({"error": {"phase": "lost", "reason": "x"}}));
        refused::<FetchResult>(json!({"status": 200, "headers": [], "base64": "", "error": null}));
    }
}
