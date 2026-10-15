//! OpenRouter structured output: `POST {base}/chat/completions` with a strict JSON-schema
//! `response_format` and provider routing from the route contract (src/openrouter/README.md;
//! spec/capabilities.md §4).
//!
//! Contract: `adapter: openrouter-structured`, `adapter_behavior: 1`, optional `request_policy`
//! ([`RequestPolicy`]), `pictures` (absent: reduce to a 1600 px long edge flattened on `matte`;
//! `"unchanged"`: send the file as is) and `provider_options` (spec/adapters.md §8). The schema is
//! a `json` file or an inline object; it is sent after `$ref` inlining and strict canonicalization
//! ([`crate::chat::schema`]), named from its `title` (else `answer`). Text context files are
//! appended to the prompt as `--- context <i> ---\n<text>`. The answer's value (`message.parsed`,
//! else the JSON text of `message.content`, unwrapped from `completionState`) is `data: {"json":
//! <value>}`; `cost` from `usage` ([`super::usage_cost`]). `check`: the value against the request's
//! **original** schema (draft 2020-12; `format` not asserted), refused as `<path>: <message>`.
//! Deadline 900 s for `openai/gpt-6-astra`, 1800 s otherwise.
//!
//! `send`, in the order of spec/adapters.md §4 (the engine makes steps 1 and 2 before it, the
//! contract's other members through `check_contract`; the adapter makes them again for a direct
//! caller):
//! 1. the contract (`read_contract`): `adapter` and `adapter_behavior`, when present, are this
//!    adapter's; `request_policy` and `pictures` are valid; no other member but `provider_options`;
//! 2. `capabilities::check_request`;
//! 3. the key (`authorization: Bearer <OPENROUTER_API_KEY>`);
//! 4. values: provider options that set no member of [`OWNED`] ([`wire::provider_options`], added
//!    after the body's own members); a non-blank `prompt`; `matte` a colour (`#rgb`, `#rrggbb`,
//!    `#rrggbbaa`); `max_tokens` at least 1; an inline schema whose references inline and which
//!    compiles as a draft 2020-12 JSON Schema;
//! 5. files: the schema file (UTF-8 JSON, an object, then as an inline schema); each context
//!    file, a picture (`image/*`, reduced unless the contract says `unchanged`) or UTF-8 text;
//! 6. a body over 200 MiB is refused; otherwise one exchange.
//!
//! Non-2xx and transport outcomes are [`super::classify`]'s (a transport's own refusal is
//! `Refused`, spec/adapters.md §3.2). A 2xx that does not become an answer is
//! `Failed { cost: <the usage's cost when the body parsed>, retryable: true }` (spec/adapters.md
//! §3.4).

use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Identity, RequestAdapter, RouteRef, Sent};
use crate::capabilities;
use crate::chat;
use crate::checks;
use crate::setup::Client;
use crate::transport::{Credential, HttpRequest, HttpResponse};
use crate::wire;
use grida_fx_core::money::Usd;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use std::time::Duration;

pub use crate::chat::{DEFAULT_MATTE, DEFAULT_MAX_TOKENS, PICTURE_EDGE};

/// The label of every reason.
pub const LABEL: &str = "OpenRouter structured generation";

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "openrouter-structured";

/// The capability this adapter serves.
pub const CAPABILITY: &str = "structured.generate";

/// The contract this adapter serves: [`CONTRACT_ADAPTER`], behaviour the number 1.
pub const IDENTITY: Identity = Identity {
    adapter: CONTRACT_ADAPTER,
    behavior: "1",
};

/// The members provider options may not set (spec/adapters.md §8; src/openrouter/README.md):
/// every member the body has, and those that would change the answer.
pub const OWNED: &[&str] = &[
    "model",
    "messages",
    "response_format",
    "provider",
    "reasoning",
    "max_tokens",
    "stream",
    "n",
    "tools",
    "tool_choice",
];

/// The model whose structured calls get the shorter deadline (src/openrouter/README.md, "Limits").
pub const FAST_DEADLINE_MODEL: &str = "openai/gpt-6-astra";

/// The deadline of a structured call on [`FAST_DEADLINE_MODEL`].
pub const FAST_DEADLINE: Duration = Duration::from_secs(900);

/// The deadline of every other structured call.
pub const DEADLINE: Duration = Duration::from_secs(1800);

/// The `reasoning.effort` values a policy may give.
pub const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The `image_detail` values a policy may give.
pub const IMAGE_DETAILS: [&str; 4] = ["auto", "low", "high", "original"];

static PROVIDER_SLUG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9][a-z0-9._-]*(?:/[a-z0-9][a-z0-9._-]*)*$").expect("a valid pattern")
});

/// A route contract's `request_policy` (shared with [`super::agent`]): `provider`
/// (`require_parameters` must be true, the default; `only` a non-empty list of distinct slugs;
/// `allow_fallbacks` a bool), `reasoning.effort` (`none`, `minimal`, `low`, `medium`, `high`,
/// `xhigh`, `max`) and `image_detail` (`auto`, `low`, `high`, `original`). Anything else refuses.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestPolicy {
    /// The `provider` object sent, members in the order `require_parameters`, `only`,
    /// `allow_fallbacks`.
    pub provider: Value,
    pub effort: Option<String>,
    pub image_detail: Option<String>,
}

impl Default for RequestPolicy {
    /// No policy: `{"require_parameters": true}`, no effort, no detail.
    fn default() -> RequestPolicy {
        RequestPolicy {
            provider: json!({"require_parameters": true}),
            effort: None,
            image_detail: None,
        }
    }
}

impl RequestPolicy {
    /// Reads `contract.request_policy` (absent: `{"require_parameters": true}`, no effort, no
    /// detail). The refusal is a sentence.
    ///
    /// A member given as `null` counts as absent.
    pub fn from_contract(contract: &Value) -> Result<RequestPolicy, String> {
        let policy = match contract.get("request_policy") {
            None | Some(Value::Null) => return Ok(RequestPolicy::default()),
            Some(Value::Object(policy)) => policy,
            Some(_) => return Err("request_policy is an object".into()),
        };
        only_members(
            policy,
            "request_policy",
            &["provider", "reasoning", "image_detail"],
        )?;
        let provider = match policy.get("provider") {
            None | Some(Value::Null) => json!({"require_parameters": true}),
            Some(Value::Object(provider)) => provider_routing(provider)?,
            Some(_) => return Err("request_policy.provider is an object".into()),
        };
        let effort = match policy.get("reasoning") {
            None | Some(Value::Null) => None,
            Some(Value::Object(reasoning)) => {
                only_members(reasoning, "request_policy.reasoning", &["effort"])?;
                match reasoning.get("effort") {
                    Some(Value::String(effort)) if EFFORTS.contains(&effort.as_str()) => {
                        Some(effort.clone())
                    }
                    _ => {
                        return Err(format!(
                            "request_policy.reasoning.effort is one of {}",
                            EFFORTS.join(", ")
                        ));
                    }
                }
            }
            Some(_) => return Err("request_policy.reasoning is an object".into()),
        };
        let image_detail = match policy.get("image_detail") {
            None | Some(Value::Null) => None,
            Some(Value::String(detail)) if IMAGE_DETAILS.contains(&detail.as_str()) => {
                Some(detail.clone())
            }
            Some(_) => {
                return Err(format!(
                    "request_policy.image_detail is one of {}",
                    IMAGE_DETAILS.join(", ")
                ));
            }
        };
        Ok(RequestPolicy {
            provider,
            effort,
            image_detail,
        })
    }

    /// The `reasoning` member of a body, when the policy gives an effort.
    pub fn reasoning(&self) -> Option<Value> {
        self.effort.as_ref().map(|effort| json!({"effort": effort}))
    }

    /// The `image_url` part of a picture, with the policy's `detail`.
    pub fn picture_part(&self, url: String) -> Value {
        chat::picture_part(url, self.image_detail.as_deref())
    }
}

fn only_members(map: &Map<String, Value>, what: &str, allowed: &[&str]) -> Result<(), String> {
    match map.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!("{what} takes no member {key}")),
        None => Ok(()),
    }
}

fn provider_routing(provider: &Map<String, Value>) -> Result<Value, String> {
    only_members(
        provider,
        "request_policy.provider",
        &["require_parameters", "only", "allow_fallbacks"],
    )?;
    match provider.get("require_parameters") {
        None | Some(Value::Null) | Some(Value::Bool(true)) => {}
        Some(_) => return Err("request_policy.provider.require_parameters must be true".into()),
    }
    let mut out = Map::new();
    out.insert("require_parameters".into(), Value::Bool(true));
    match provider.get("only") {
        None | Some(Value::Null) => {}
        Some(Value::Array(slugs)) if valid_slugs(slugs) => {
            out.insert("only".into(), Value::Array(slugs.clone()));
        }
        Some(_) => {
            return Err(
                "request_policy.provider.only is a non-empty list of distinct provider slugs"
                    .into(),
            );
        }
    }
    match provider.get("allow_fallbacks") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(allow)) => {
            out.insert("allow_fallbacks".into(), Value::Bool(*allow));
        }
        Some(_) => return Err("request_policy.provider.allow_fallbacks is true or false".into()),
    }
    Ok(Value::Object(out))
}

fn valid_slugs(slugs: &[Value]) -> bool {
    let mut seen: Vec<&str> = Vec::new();
    for slug in slugs {
        let Some(slug) = slug.as_str() else {
            return false;
        };
        if !PROVIDER_SLUG.is_match(slug) || seen.contains(&slug) {
            return false;
        }
        seen.push(slug);
    }
    !seen.is_empty()
}

/// What a route contract asks of an OpenRouter chat adapter (spec/adapters.md §4 step 1;
/// src/openrouter/README.md): its policy, and for structured output whether pictures go unchanged.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChatContract {
    pub(super) policy: RequestPolicy,
    pub(super) pictures_unchanged: bool,
}

/// Reads the route's contract for the adapter `adapter` serving `capability`. `adapter` and
/// `adapter_behavior` (the integer 1, which both OpenRouter chat adapters serve), when present,
/// must be this adapter's ([`checks::contract`], the engine's check); `request_policy` is read by
/// [`RequestPolicy::from_contract`]; `pictures` (absent or `"unchanged"`) only when
/// `takes_pictures`; `provider_options`, read when the body is built ([`wire::provider_options`]);
/// no other member. Refused with `<route> is not a <capability> route this adapter serves`,
/// followed by `: <why>` when a member is not one it can serve.
pub(super) fn read_contract(
    route: &RouteRef,
    capability: &str,
    adapter: &'static str,
    takes_pictures: bool,
) -> Result<ChatContract, String> {
    let refused = |why: Option<String>| {
        let mut reason = format!(
            "{} is not a {capability} route this adapter serves",
            route.id()
        );
        if let Some(why) = why {
            reason.push_str(": ");
            reason.push_str(&why);
        }
        reason
    };
    if route.capability != capability {
        return Err(refused(None));
    }
    let identity = Identity {
        adapter,
        behavior: "1",
    };
    checks::contract(route, &identity)?;
    let empty = Map::new();
    let contract = route.contract.as_object().unwrap_or(&empty);
    let mut allowed = vec![
        "adapter",
        "adapter_behavior",
        "request_policy",
        "provider_options",
    ];
    if takes_pictures {
        allowed.push("pictures");
    }
    only_members(contract, "the contract", &allowed).map_err(|why| refused(Some(why)))?;
    let pictures_unchanged = match contract.get("pictures") {
        None | Some(Value::Null) => false,
        Some(Value::String(pictures)) if pictures == "unchanged" => true,
        Some(_) => {
            return Err(refused(Some(
                "pictures is \"unchanged\" or absent".to_string(),
            )));
        }
    };
    let policy = RequestPolicy::from_contract(&route.contract).map_err(|why| refused(Some(why)))?;
    Ok(ChatContract {
        policy,
        pictures_unchanged,
    })
}

// ---------------------------------------------------------------------------------------------
// Helpers shared with the agent adapter

/// The chat completions request carrying `body` ([`chat::completions_request`]).
pub(super) fn chat_request(
    client: &Client,
    credential: Credential,
    body: Value,
    timeout: Duration,
) -> HttpRequest {
    chat::completions_request(
        client,
        Some(credential),
        body,
        timeout,
        super::MAX_CHAT_RESPONSE_BYTES,
    )
}

/// The cost `usage` reports ([`super::usage_cost`]).
pub(super) fn reported_cost(body: &Map<String, Value>) -> Option<Usd> {
    super::usage_cost(body.get("usage"))
}

/// A structural failure of a 2xx (spec/adapters.md §3.4): billed, retryable
/// ([`chat::structural`]).
pub(super) use crate::chat::structural;

// ---------------------------------------------------------------------------------------------
// The adapter

/// OpenRouter's structured-output adapter (module doc).
#[derive(Debug, Clone)]
pub struct OpenRouterStructured {
    client: Client,
}

/// A call ready to send.
struct Prepared {
    credential: Credential,
    body: Value,
    timeout: Duration,
}

impl OpenRouterStructured {
    pub fn new(client: Client) -> OpenRouterStructured {
        OpenRouterStructured { client }
    }

    /// Steps 1 to 5 of the module doc, and the body.
    fn prepare(&self, call: &CallRequest) -> Result<Prepared, String> {
        // 1. The contract.
        let contract = read_contract(&call.route, CAPABILITY, CONTRACT_ADAPTER, true)?;
        // 2. The capability.
        capabilities::check_request(CAPABILITY, &call.request)?;
        // 3. The key.
        let credential = self.client.credential()?;
        // 4 and 5. Values, then files.
        let options = wire::provider_options(call, OWNED)?;
        let prompt = chat::structured_prompt(
            call,
            contract.pictures_unchanged,
            contract.policy.image_detail.as_deref(),
        )?;
        // The body (src/openrouter/README.md).
        let mut body = Map::new();
        body.insert(
            "model".into(),
            Value::String(call.route.model.trim().into()),
        );
        body.insert("messages".into(), Value::Array(prompt.messages));
        body.insert("response_format".into(), prompt.response_format);
        body.insert("provider".into(), contract.policy.provider.clone());
        if let Some(reasoning) = contract.policy.reasoning() {
            body.insert("reasoning".into(), reasoning);
        }
        body.insert("max_tokens".into(), json!(prompt.max_tokens));
        body.extend(options);
        let body = Value::Object(body);
        if wire::body_exceeds(&body, super::MAX_REQUEST_BYTES) {
            return Err(super::BODY_TOO_LARGE.into());
        }
        let timeout = if call.route.model.trim() == FAST_DEADLINE_MODEL {
            FAST_DEADLINE
        } else {
            DEADLINE
        };
        Ok(Prepared {
            credential,
            body,
            timeout,
        })
    }

    /// A 2xx's body as an answer (src/openrouter/README.md).
    fn parse(&self, response: &HttpResponse) -> Sent {
        let body = match wire::json_object(&response.body, LABEL) {
            Ok(body) => body,
            Err(reason) => return structural(&self.client, response, &reason, None),
        };
        let cost = reported_cost(&body);
        let failed =
            |what: &str| structural(&self.client, response, &format!("{LABEL} {what}"), cost);
        let Some(message) = chat::first_message(&body) else {
            return failed("returned no message");
        };
        match chat::structured_value(message) {
            Ok(value) => Sent::Answered(Answer::new(json!({"json": value}), cost)),
            Err(what) => failed(what),
        }
    }
}

impl RequestAdapter for OpenRouterStructured {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let prepared = match self.prepare(call) {
                Ok(prepared) => prepared,
                Err(reason) => {
                    return Sent::Refused {
                        reason: self.client.reason(&reason),
                    };
                }
            };
            let request = chat_request(
                &self.client,
                prepared.credential,
                prepared.body,
                prepared.timeout,
            );
            match super::classify(&self.client, LABEL, self.client.send(request).await) {
                Ok(response) => self.parse(&response),
                Err(sent) => sent,
            }
        })
    }

    /// The answer's value against the request's original schema (spec/capabilities.md §4;
    /// [`checks::structured`]).
    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        checks::structured(call, answer, &self.client.redactor)
    }

    fn identity(&self) -> Option<Identity> {
        Some(IDENTITY)
    }

    fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        read_contract(&call.route, CAPABILITY, CONTRACT_ADAPTER, true).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(contract: Value) -> RouteRef {
        RouteRef {
            capability: CAPABILITY.into(),
            model: "acme/text-a".into(),
            provider: "openrouter".into(),
            contract,
            limits: Default::default(),
        }
    }

    #[test]
    fn a_policy_reads_from_the_contract() {
        assert_eq!(
            RequestPolicy::from_contract(&json!({})).unwrap(),
            RequestPolicy::default()
        );
        assert_eq!(
            RequestPolicy::default().provider,
            json!({"require_parameters": true})
        );
        let policy = RequestPolicy::from_contract(&json!({"request_policy": {
            "image_detail": "high",
            "reasoning": {"effort": "high"},
            "provider": {"allow_fallbacks": false, "only": ["openai"], "require_parameters": true}
        }}))
        .unwrap();
        // Members in the order require_parameters, only, allow_fallbacks.
        assert_eq!(
            serde_json::to_string(&policy.provider).unwrap(),
            r#"{"require_parameters":true,"only":["openai"],"allow_fallbacks":false}"#
        );
        assert_eq!(policy.effort.as_deref(), Some("high"));
        assert_eq!(policy.image_detail.as_deref(), Some("high"));
        assert_eq!(policy.reasoning(), Some(json!({"effort": "high"})));
        assert_eq!(
            policy.picture_part("data:x".into()),
            json!({"type": "image_url", "image_url": {"url": "data:x", "detail": "high"}})
        );
        assert_eq!(
            RequestPolicy::default().picture_part("data:x".into()),
            json!({"type": "image_url", "image_url": {"url": "data:x"}})
        );
        // provider absent: the default; require_parameters absent: true.
        let bare = RequestPolicy::from_contract(
            &json!({"request_policy": {"provider": {"only": ["a/b-c.d_e"]}}}),
        )
        .unwrap();
        assert_eq!(
            bare.provider,
            json!({"require_parameters": true, "only": ["a/b-c.d_e"]})
        );
    }

    #[test]
    fn every_documented_effort_and_detail_is_accepted() {
        for effort in EFFORTS {
            let policy = RequestPolicy::from_contract(
                &json!({"request_policy": {"reasoning": {"effort": effort}}}),
            )
            .unwrap();
            assert_eq!(policy.reasoning(), Some(json!({"effort": effort})));
        }
        for detail in IMAGE_DETAILS {
            let policy =
                RequestPolicy::from_contract(&json!({"request_policy": {"image_detail": detail}}))
                    .unwrap();
            assert_eq!(policy.image_detail.as_deref(), Some(detail));
        }
    }

    #[test]
    fn bad_policies_are_refused_with_the_member_named() {
        let refused = |policy: Value| {
            RequestPolicy::from_contract(&json!({ "request_policy": policy })).unwrap_err()
        };
        assert_eq!(refused(json!([])), "request_policy is an object");
        assert_eq!(
            refused(json!({"temperature": 0})),
            "request_policy takes no member temperature"
        );
        assert_eq!(
            refused(json!({"provider": ["openai"]})),
            "request_policy.provider is an object"
        );
        assert_eq!(
            refused(json!({"provider": {"order": ["openai"]}})),
            "request_policy.provider takes no member order"
        );
        for bad in [json!(false), json!(1), json!("true")] {
            assert_eq!(
                refused(json!({"provider": {"require_parameters": bad}})),
                "request_policy.provider.require_parameters must be true"
            );
        }
        for bad in [
            json!([]),
            json!(["openai", "openai"]),
            json!([""]),
            json!(["openai "]),
            json!(["https://example.test/provider"]),
            json!([7]),
            json!(["openai\nsecret"]),
            json!(["Openai"]),
            json!("openai"),
        ] {
            assert_eq!(
                refused(json!({"provider": {"only": bad}})),
                "request_policy.provider.only is a non-empty list of distinct provider slugs",
                "{bad}"
            );
        }
        for bad in [json!(1), json!("false")] {
            assert_eq!(
                refused(json!({"provider": {"allow_fallbacks": bad}})),
                "request_policy.provider.allow_fallbacks is true or false"
            );
        }
        for bad in [
            json!({"effort": "ultra"}),
            json!({"effort": true}),
            json!({}),
        ] {
            assert_eq!(
                refused(json!({ "reasoning": bad })),
                "request_policy.reasoning.effort is one of none, minimal, low, medium, high, xhigh, max"
            );
        }
        assert_eq!(
            refused(json!({"reasoning": "high"})),
            "request_policy.reasoning is an object"
        );
        assert_eq!(
            refused(json!({"reasoning": {"effort": "high", "summary": "auto"}})),
            "request_policy.reasoning takes no member summary"
        );
        for bad in [json!("medium"), json!(1)] {
            assert_eq!(
                refused(json!({ "image_detail": bad })),
                "request_policy.image_detail is one of auto, low, high, original"
            );
        }
    }

    #[test]
    fn contracts_are_read_or_refused() {
        let read =
            |contract: Value| read_contract(&route(contract), CAPABILITY, CONTRACT_ADAPTER, true);
        let ok = read(json!({"adapter": "openrouter-structured", "adapter_behavior": 1})).unwrap();
        assert_eq!(ok.policy, RequestPolicy::default());
        assert!(!ok.pictures_unchanged);
        assert!(read(json!({})).is_ok());
        // Provider options are read when the body is built, not here.
        assert!(read(json!({"provider_options": {"openrouter": {"seed": 1}}})).is_ok());
        assert!(
            read(json!({"adapter": "openrouter-structured", "adapter_behavior": 1, "pictures": "unchanged"}))
                .unwrap()
                .pictures_unchanged
        );
        let foreign =
            "acme/text-a@openrouter is not a structured.generate route this adapter serves";
        for contract in [
            json!({"adapter": "openrouter-tool-loop", "adapter_behavior": 1}),
            json!({"adapter": "openrouter-structured", "adapter_behavior": 2}),
            json!({"adapter": "openrouter-structured", "adapter_behavior": "1"}),
            json!({"adapter": 1}),
            json!([]),
        ] {
            assert_eq!(read(contract.clone()).unwrap_err(), foreign, "{contract}");
        }
        assert_eq!(
            read(json!({"pictures": "reduced"})).unwrap_err(),
            format!("{foreign}: pictures is \"unchanged\" or absent")
        );
        assert_eq!(
            read(json!({"route_id": "x"})).unwrap_err(),
            format!("{foreign}: the contract takes no member route_id")
        );
        assert_eq!(
            read(json!({"request_policy": {"image_detail": "medium"}})).unwrap_err(),
            format!("{foreign}: request_policy.image_detail is one of auto, low, high, original")
        );
        // The agent's contract has no pictures.
        assert_eq!(
            read_contract(
                &RouteRef {
                    capability: "agent.turn".into(),
                    ..route(json!({"pictures": "unchanged"}))
                },
                "agent.turn",
                "openrouter-tool-loop",
                false
            )
            .unwrap_err(),
            "acme/text-a@openrouter is not a agent.turn route this adapter serves: the contract takes no member pictures"
        );
        // Another capability's route is not served.
        assert_eq!(
            read_contract(
                &route(json!({})),
                "agent.turn",
                "openrouter-tool-loop",
                false
            )
            .unwrap_err(),
            "acme/text-a@openrouter is not a agent.turn route this adapter serves"
        );
    }
}
