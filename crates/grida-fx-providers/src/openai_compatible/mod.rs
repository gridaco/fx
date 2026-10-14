//! OpenAI-compatible providers: the `openai-compatible` family (src/openai_compatible/README.md;
//! spec/providers.md §1, §2). For each provider the planning project declares with the family, one
//! adapter per capability, the default of its capability on that provider: `structured.generate`
//! ([`structured::CompatibleStructured`]) and `agent.turn` ([`agent::CompatibleAgent`]), each one
//! `POST {base}/chat/completions` answered synchronously.
//!
//! The messages, the strict JSON-schema response format, the strict function tools and the answers
//! are [`crate::chat`]'s, as OpenRouter's and GG's are. A body holds only the members of OpenAI's
//! Chat Completions API those need, then the provider options. The credential is the
//! declaration's, and none for a provider declared without one. No cost is read: every answer and
//! every failure costs `None`, so the engine charges the whole hold, or a capped route's cap; the
//! tokens a completion's `usage` counts are the answer's usage ([`chat::usage`]). Statuses are
//! spec/adapters.md §3.3 as written ([`classify`]). Labels name the provider: `<name> structured
//! generation`, `<name> tool loop`.

pub mod agent;
pub mod structured;

use crate::adapter::{Adapter, Answer, Identity, RouteRef, Sent};
use crate::chat;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{HttpResponse, Phase, TransportError, TransportErrorKind};
use crate::wire;
use grida_fx_core::providers::Declaration;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::{Arc, LazyLock};

/// The family a declaration names (spec/providers.md §1).
pub const FAMILY: &str = "openai-compatible";

/// The largest request body sent; a bigger one is refused before sending.
pub const MAX_REQUEST_BYTES: usize = 200 * 1024 * 1024;

/// The refusal of a request body over [`MAX_REQUEST_BYTES`].
pub const BODY_TOO_LARGE: &str = "request body exceeds 200 MiB";

/// The response cap of a chat completion.
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// What both adapters of one declared provider share: its client, its name, and what a redirect
/// asks to check.
#[derive(Debug, Clone)]
pub struct Provider {
    pub client: Client,
    /// The declaration's label, its name: the first word of every label.
    pub name: String,
    /// The declaration's base-URL variable, else `providers.<name>.base_url`.
    redirect: String,
}

impl Provider {
    /// The provider of `declaration`, at its base URL in `setup`.
    pub fn new(setup: &Setup, declaration: &Declaration) -> Provider {
        let redirect = match &declaration.base_url.variable {
            Some(variable) => variable.clone(),
            None => format!("providers.{}.base_url", declaration.name),
        };
        Provider {
            client: setup.declared_client(declaration),
            name: declaration.label.clone(),
            redirect,
        }
    }
}

/// Registers both adapters on every provider the planning project declares with [`FAMILY`]
/// (`setup`'s declarations), each its capability's default there.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    for declaration in setup.declarations().declared() {
        if declaration.family.as_deref() != Some(FAMILY) {
            continue;
        }
        let provider = Provider::new(setup, declaration);
        let name = declaration.name.as_str();
        adapters.register(
            structured::CAPABILITY,
            name,
            Adapter::Request(Arc::new(structured::CompatibleStructured::new(
                provider.clone(),
            ))),
        );
        adapters.register(
            agent::CAPABILITY,
            name,
            Adapter::Request(Arc::new(agent::CompatibleAgent::new(provider))),
        );
    }
}

/// spec/adapters.md §4 step 1 for the adapter of `identity` serving `capability`: the call is on
/// `capability`, and the contract's `adapter` and `adapter_behavior`, when present, are the
/// adapter's ([`crate::checks::contract`], the engine's check, made again for a direct caller);
/// `provider_options` is read when the body is built; no other member. Refused with `<route> is
/// not a <capability> route this adapter serves`, followed by `: the contract takes no member
/// <m>` for another member.
pub(crate) fn read_contract(
    route: &RouteRef,
    capability: &str,
    identity: &Identity,
) -> Result<(), String> {
    if route.capability != capability {
        return Err(crate::checks::not_served(route));
    }
    crate::checks::contract(route, identity)?;
    let mut members = route.contract.as_object().into_iter().flat_map(Map::keys);
    let allowed = ["adapter", "adapter_behavior", "provider_options"];
    match members.find(|member| !allowed.contains(&member.as_str())) {
        Some(member) => Err(format!(
            "{}: the contract takes no member {member}",
            crate::checks::not_served(route)
        )),
        None => Ok(()),
    }
}

/// An answer's `data`, at no cost, with the tokens the completion `body` counts.
pub(crate) fn answer(data: Value, body: &Map<String, Value>) -> Answer {
    let answer = Answer::new(data, None);
    match chat::usage(body) {
        Some(usage) => answer.with_usage(usage),
        None => answer,
    }
}

/// A `retry-after` the reason may name: seconds, as a short decimal.
static SAFE_SECONDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{1,6}(?:\.[0-9]{1,3})?$").expect("a valid pattern"));

/// How an exchange that is not a 2xx becomes a [`Sent`] (spec/adapters.md §3.2 and §3.3, with no
/// override); `label` is the call's (`<name> structured generation`, `<name> tool loop`). A 2xx
/// comes back as `Ok` for the adapter to parse.
/// - a transport failure: refused by the transport → `Refused`; otherwise `not_sent` →
///   `NotReceived`; `after_send` → `Failed { cost: None, retryable: true }`;
/// - 3xx → `Failed { cost: None, retryable: false }`: `<label> was redirected (HTTP <n>); check
///   <VARIABLE>`, the declaration's base-URL variable, else `providers.<name>.base_url`;
/// - 408 and 429 → `NotReceived`, the provider's wait as `retry_after`;
/// - any other 4xx → `Failed { cost: None, retryable: false }`;
/// - 5xx, and any other status → `Failed { cost: None, retryable: true }`.
///
/// A status's reason is `<label> returned HTTP <n>`, with the safe detail of an OpenAI-style error
/// envelope ([`wire::safe_error_detail`]), the request id when the response carries a safe one, and
/// for a 408 or a 429 `; retry-after <seconds>`. Every reason is redacted.
#[allow(clippy::result_large_err)] // `Sent` is what every caller returns at once
pub fn classify(
    provider: &Provider,
    label: &str,
    exchange: Result<HttpResponse, TransportError>,
) -> Result<HttpResponse, Sent> {
    let client = &provider.client;
    let response = match exchange {
        Ok(response) => response,
        Err(error) => {
            let reason = match error.phase {
                Phase::NotSent => format!("{label} was not sent: {}", error.reason),
                Phase::AfterSend => format!("{label} failed: {}", error.reason),
            };
            let reason = client.reason(&reason);
            return Err(match error.phase {
                Phase::NotSent if error.kind == TransportErrorKind::Refused => {
                    Sent::Refused { reason }
                }
                Phase::NotSent => Sent::not_received(reason),
                Phase::AfterSend => Sent::Failed {
                    reason,
                    cost: None,
                    retryable: true,
                },
            });
        }
    };
    let status = response.status;
    if response.is_success() {
        return Ok(response);
    }
    if (300..400).contains(&status) {
        return Err(Sent::Failed {
            reason: client.reason(&format!(
                "{label} was redirected (HTTP {status}); check {}",
                provider.redirect
            )),
            cost: None,
            retryable: false,
        });
    }
    let detail = wire::safe_error_detail(&response.body, &client.redactor);
    let reason = wire::status_reason(label, status, detail.as_deref());
    let reason = wire::with_request_id(reason, &response);
    Err(match status {
        408 | 429 => {
            let named = response
                .header("retry-after")
                .map(str::trim)
                .filter(|after| SAFE_SECONDS.is_match(after));
            let reason = match named {
                Some(after) => format!("{reason}; retry-after {after}"),
                None => reason,
            };
            Sent::NotReceived {
                reason: client.reason(&reason),
                retry_after: crate::adapter::retry_after(&response),
            }
        }
        400..=499 => Sent::Failed {
            reason: client.reason(&reason),
            cost: None,
            retryable: false,
        },
        _ => Sent::Failed {
            reason: client.reason(&reason),
            cost: None,
            retryable: true,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{Environment, Keys};
    use crate::testing::setup;
    use crate::transport::replay::NoNetwork;
    use grida_fx_core::providers::{BaseUrl, Credential, Declarations};
    use serde_json::json;

    /// `acme`, declared with a key and a base-URL variable, and `local`, with neither.
    fn declared() -> Declarations {
        let declare = |name: &str, key: Option<&str>, variable: Option<&str>| Declaration {
            name: name.into(),
            label: name.into(),
            credential: key.map(|key| Credential {
                variable: key.into(),
                header: "authorization".into(),
                scheme: "Bearer".into(),
            }),
            base_url: BaseUrl {
                variable: variable.map(String::from),
                default: format!("https://api.{name}.test/v1"),
            },
            download_hosts: Vec::new(),
            family: Some(FAMILY.into()),
        };
        Declarations::with(&[
            declare("acme", Some("ACME_API_KEY"), Some("ACME_BASE_URL")),
            declare("local", None, None),
        ])
    }

    fn provider(name: &str) -> Provider {
        let environment = Environment::declared(&[("ACME_API_KEY", "acme-secret")], &declared());
        let setup = setup(Arc::new(NoNetwork), Keys::from_environment(&environment));
        Provider::new(&setup, setup.declarations().get(name).unwrap())
    }

    fn sent(status: u16, body: Value) -> Sent {
        classify(
            &provider("acme"),
            "acme tool loop",
            Ok(HttpResponse::json(status, &body)),
        )
        .unwrap_err()
    }

    #[test]
    fn statuses_follow_the_default_table() {
        let envelope =
            json!({"error": {"code": "invalid_request_error", "message": "bad acme-secret"}});
        for status in [408, 429] {
            assert!(
                matches!(sent(status, json!({})), Sent::NotReceived { .. }),
                "{status}"
            );
        }
        for status in [400, 401, 402, 403, 404, 413, 422] {
            assert!(
                matches!(
                    sent(status, envelope.clone()),
                    Sent::Failed {
                        cost: None,
                        retryable: false,
                        ..
                    }
                ),
                "{status}"
            );
        }
        for status in [500, 502, 503, 504, 520, 600] {
            assert!(
                matches!(
                    sent(status, envelope.clone()),
                    Sent::Failed {
                        cost: None,
                        retryable: true,
                        ..
                    }
                ),
                "{status}"
            );
        }
        let Sent::Failed { reason, .. } = sent(401, envelope) else {
            unreachable!()
        };
        assert_eq!(
            reason,
            "acme tool loop returned HTTP 401: code=invalid_request_error"
        );
        let response = HttpResponse::new(429, Vec::new())
            .with_header("Retry-After", "20")
            .with_header("x-request-id", "req-1");
        let Sent::NotReceived {
            reason,
            retry_after,
        } = classify(&provider("acme"), "acme tool loop", Ok(response)).unwrap_err()
        else {
            unreachable!()
        };
        assert_eq!(
            reason,
            "acme tool loop returned HTTP 429 (request req-1); retry-after 20"
        );
        assert_eq!(retry_after, Some(std::time::Duration::from_secs(20)));
        assert!(
            classify(
                &provider("acme"),
                "X",
                Ok(HttpResponse::new(200, b"{}".to_vec()))
            )
            .is_ok()
        );
    }

    #[test]
    fn a_redirect_names_where_the_base_url_comes_from() {
        for (name, check) in [
            ("acme", "ACME_BASE_URL"),
            ("local", "providers.local.base_url"),
        ] {
            let label = format!("{name} structured generation");
            let response = HttpResponse::new(307, Vec::new());
            assert_eq!(
                classify(&provider(name), &label, Ok(response)).unwrap_err(),
                Sent::Failed {
                    reason: format!("{label} was redirected (HTTP 307); check {check}"),
                    cost: None,
                    retryable: false,
                }
            );
        }
    }

    #[test]
    fn transport_phases_classify() {
        let classified = |error| classify(&provider("acme"), "X", Err(error)).unwrap_err();
        assert!(matches!(
            classified(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused"
            )),
            Sent::NotReceived { .. }
        ));
        assert_eq!(
            classified(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off"
            )),
            Sent::Refused {
                reason: "X was not sent: the network is off".into()
            }
        );
        assert!(matches!(
            classified(TransportError::after_send(
                TransportErrorKind::Other,
                "reset"
            )),
            Sent::Failed {
                cost: None,
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn only_a_family_s_declarations_are_registered() {
        let mut declarations: Vec<Declaration> = declared().declared().to_vec();
        declarations[1].family = None;
        let environment = Environment::declared(&[], &Declarations::with(&declarations));
        let setup = setup(Arc::new(NoNetwork), Keys::from_environment(&environment));
        let mut adapters = Adapters::new();
        register(&mut adapters, &setup);
        assert_eq!(
            adapters.served(),
            [
                ("structured.generate".to_string(), "acme".to_string()),
                ("agent.turn".to_string(), "acme".to_string()),
            ]
        );
    }
}
