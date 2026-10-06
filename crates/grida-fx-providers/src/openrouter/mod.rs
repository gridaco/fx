//! OpenRouter: images, structured output, agent turns and music, all synchronous
//! (spec/providers.md §9.2; spec/capabilities.md).
//!
//! Credential `authorization: Bearer <OPENROUTER_API_KEY>`; base `OPENROUTER_BASE_URL`, default
//! `https://openrouter.ai/api/v1`. Every request is one JSON body (pictures as data URLs); no
//! downloads, no multipart, no long jobs. Costs come from `usage.cost` ([`crate::wire::usd_ceil`]),
//! the last SSE `usage` for music. Status classes, shared by the four adapters, are
//! spec/providers.md §9.2's table, implemented once in [`classify`]; so are the contract check
//! (`serves`, `not_served`), the request-body cap (`body_exceeds`) and request ids in reasons
//! (`with_request_id`).

pub mod agent;
pub mod image;
pub mod music;
pub mod schema;
pub mod structured;

use crate::adapter::{Adapter, CallRequest, Sent};
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{HttpResponse, Phase, TransportError, TransportErrorKind};
use grida_fx_core::money::Usd;
use serde_json::Value;
use std::sync::Arc;

/// The largest request body OpenRouter is sent; a bigger one is refused before sending.
pub const MAX_REQUEST_BYTES: usize = 200 * 1024 * 1024;

/// The response cap of a chat completion.
pub const MAX_CHAT_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// The refusal of a request body over [`MAX_REQUEST_BYTES`].
pub const BODY_TOO_LARGE: &str = "request body exceeds 200 MiB";

/// The shared client.
pub fn client(setup: &Setup) -> Client {
    setup.client(KeyName::OpenRouter, &setup.endpoints.openrouter)
}

/// How an exchange that is not a 2xx becomes a [`Sent`] (spec/providers.md §4.2 and the §9.2
/// table); `label` is the capability's (`OpenRouter image generation`, …). A 2xx comes back as
/// `Ok` for the adapter to parse. Shared by the four adapters, so they classify alike. A request
/// the transport refused itself (the network turned off, a URL it cannot use) is `Refused`:
/// nothing left, so resending cannot help.
#[allow(clippy::result_large_err)] // `Sent` is what every caller returns at once
pub fn classify(
    client: &Client,
    label: &str,
    exchange: Result<HttpResponse, TransportError>,
) -> Result<HttpResponse, Sent> {
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
    let envelope = crate::wire::error_envelope(&response.body).is_some();
    let detail = crate::wire::safe_error_detail(&response.body, &client.redactor);
    let mut reason = crate::wire::status_reason(label, status, detail.as_deref());
    if let Some(id) = crate::wire::request_id(&response) {
        reason.push_str(&format!(" (request {id})"));
    }
    let sent = match status {
        429 => {
            let mut reason = format!("{label} was rate limited (HTTP 429)");
            if let Some(after) = response
                .header("retry-after")
                .filter(|v| SAFE_NUMBER.is_match(v))
            {
                reason.push_str(&format!("; retry-after {after}"));
            }
            Sent::NotReceived {
                reason,
                retry_after: crate::adapter::retry_after(&response),
            }
        }
        408 => Sent::NotReceived {
            reason,
            retry_after: crate::adapter::retry_after(&response),
        },
        503 if envelope => Sent::NotReceived {
            reason,
            retry_after: crate::adapter::retry_after(&response),
        },
        300..=399 => Sent::Failed {
            reason: format!("{label} was redirected (HTTP {status}); check OPENROUTER_BASE_URL"),
            cost: None,
            retryable: false,
        },
        400..=499 => Sent::Failed {
            reason,
            cost: envelope.then_some(Usd::ZERO),
            retryable: false,
        },
        _ => Sent::Failed {
            reason,
            cost: None,
            retryable: true,
        },
    };
    Err(match sent {
        Sent::NotReceived {
            reason,
            retry_after,
        } => Sent::NotReceived {
            reason: client.reason(&reason),
            retry_after,
        },
        Sent::Failed {
            reason,
            cost,
            retryable,
        } => Sent::Failed {
            reason: client.reason(&reason),
            cost,
            retryable,
        },
        other => other,
    })
}

static SAFE_NUMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[0-9]{1,6}(?:\.[0-9]{1,3})?$").expect("a valid pattern")
});

/// `<route> is not a <capability> route this adapter serves` (spec/providers.md §5 step 1).
pub(crate) fn not_served(call: &CallRequest) -> String {
    format!(
        "{} is not a {} route this adapter serves",
        call.route.id(),
        call.route.capability
    )
}

/// Whether a route contract is one the adapter `adapter` serves (spec/providers.md §5 step 1):
/// `null` or an object whose `adapter`, when present, is `adapter`, and whose `adapter_behavior`,
/// when present, is one `behavior` accepts. `null` members count as absent.
pub(crate) fn serves(contract: &Value, adapter: &str, behavior: impl Fn(&Value) -> bool) -> bool {
    let contract = match contract {
        Value::Null => return true,
        Value::Object(contract) => contract,
        _ => return false,
    };
    let named = match contract.get("adapter") {
        None | Some(Value::Null) => true,
        Some(name) => name.as_str() == Some(adapter),
    };
    let behaves = match contract.get("adapter_behavior") {
        None | Some(Value::Null) => true,
        Some(value) => behavior(value),
    };
    named && behaves
}

/// The reason with ` (request <id>)` when the response carries a safe request id.
pub(crate) fn with_request_id(reason: String, response: &HttpResponse) -> String {
    match crate::wire::request_id(response) {
        Some(id) => format!("{reason} (request {id})"),
        None => reason,
    }
}

/// Whether the compact JSON encoding of `body` (what the transport sends) is longer than `limit`
/// bytes. Counts without building the encoding, and stops at the limit.
pub(crate) fn body_exceeds(body: &Value, limit: usize) -> bool {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(buf.len());
            if self.bytes > self.limit {
                return Err(std::io::Error::other("over the limit"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, body).is_err() || counter.bytes > limit
}

/// Registers the four adapters on provider `openrouter`.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let client = client(setup);
    let images = Arc::new(image::OpenRouterImages::new(client.clone()));
    adapters.register(
        "image.generate",
        "openrouter",
        Adapter::Request(images.clone()),
    );
    adapters.register("image.edit", "openrouter", Adapter::Request(images));
    adapters.register(
        "structured.generate",
        "openrouter",
        Adapter::Request(Arc::new(structured::OpenRouterStructured::new(
            client.clone(),
        ))),
    );
    adapters.register(
        "agent.turn",
        "openrouter",
        Adapter::Request(Arc::new(agent::OpenRouterAgent::new(client.clone()))),
    );
    adapters.register(
        "music.generate",
        "openrouter",
        Adapter::Request(Arc::new(music::OpenRouterMusic::new(client))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use crate::keys::Keys;
    use crate::setup::Endpoints;
    use crate::transport::TransportErrorKind;
    use crate::transport::replay::NoNetwork;
    use serde_json::json;

    fn client() -> Client {
        let setup = Setup {
            transport: Arc::new(NoNetwork),
            keys: Keys::from_pairs(&[(KeyName::OpenRouter, "or-secret")]),
            endpoints: Endpoints::default(),
            clock: Arc::new(SystemClock::new()),
        };
        super::client(&setup)
    }

    fn sent(status: u16, body: serde_json::Value) -> Sent {
        classify(
            &client(),
            "OpenRouter image generation",
            Ok(HttpResponse::json(status, &body)),
        )
        .unwrap_err()
    }

    #[test]
    fn statuses_classify_per_the_table() {
        let envelope = json!({"error": {"code": 400, "message": "bad or-secret"}});
        assert!(matches!(sent(429, json!({})), Sent::NotReceived { .. }));
        assert!(matches!(sent(408, json!({})), Sent::NotReceived { .. }));
        assert!(matches!(
            sent(503, envelope.clone()),
            Sent::NotReceived { .. }
        ));
        assert!(matches!(
            sent(503, json!("x")),
            Sent::Failed {
                cost: None,
                retryable: true,
                ..
            }
        ));
        for status in [400, 401, 402, 403, 404, 413, 422] {
            assert!(
                matches!(
                    sent(status, envelope.clone()),
                    Sent::Failed {
                        cost: Some(Usd::ZERO),
                        retryable: false,
                        ..
                    }
                ),
                "{status}"
            );
        }
        assert!(matches!(
            sent(400, json!("x")),
            Sent::Failed {
                cost: None,
                retryable: false,
                ..
            }
        ));
        for status in [500, 502, 504, 520] {
            assert!(matches!(
                sent(status, json!({})),
                Sent::Failed {
                    cost: None,
                    retryable: true,
                    ..
                }
            ));
        }
        assert!(matches!(
            sent(302, json!({})),
            Sent::Failed {
                cost: None,
                retryable: false,
                ..
            }
        ));
        let Sent::Failed { reason, .. } = sent(401, envelope) else {
            unreachable!()
        };
        assert_eq!(
            reason,
            "OpenRouter image generation returned HTTP 401: code=400"
        );
        assert!(classify(&client(), "X", Ok(HttpResponse::new(200, b"{}".to_vec()))).is_ok());
    }

    #[test]
    fn transport_phases_classify() {
        let not_sent = classify(
            &client(),
            "X",
            Err(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused",
            )),
        )
        .unwrap_err();
        assert!(matches!(not_sent, Sent::NotReceived { .. }));
        let refused = classify(
            &client(),
            "X",
            Err(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off",
            )),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            Sent::Refused {
                reason: "X was not sent: the network is off".into()
            }
        );
        let after = classify(
            &client(),
            "X",
            Err(TransportError::after_send(
                TransportErrorKind::Other,
                "reset",
            )),
        )
        .unwrap_err();
        assert!(matches!(
            after,
            Sent::Failed {
                cost: None,
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn a_rate_limit_names_its_wait() {
        let response = HttpResponse::new(429, Vec::new()).with_header("Retry-After", "20");
        let Sent::NotReceived {
            reason,
            retry_after,
        } = classify(&client(), "X", Ok(response)).unwrap_err()
        else {
            unreachable!()
        };
        assert_eq!(reason, "X was rate limited (HTTP 429); retry-after 20");
        assert_eq!(retry_after, Some(std::time::Duration::from_secs(20)));
    }

    #[test]
    fn bodies_are_measured_as_sent() {
        let body = json!({"model": "m", "prompt": "é"});
        let sent = serde_json::to_vec(&body).unwrap().len();
        assert!(!body_exceeds(&body, sent));
        assert!(body_exceeds(&body, sent - 1));
        assert!(body_exceeds(&json!({"prompt": "x".repeat(4096)}), 1024));
        // {"a":"xxxx"} is 12 bytes.
        assert!(!body_exceeds(&json!({"a": "xxxx"}), 12));
        assert!(body_exceeds(&json!({"a": "xxxx"}), 11));
        assert_eq!(MAX_REQUEST_BYTES, 200 * 1024 * 1024);
    }

    #[test]
    fn contract_members_are_checked_when_present() {
        let three = |v: &Value| v == &json!("3");
        let adapter = "fx-openrouter-image-v1";
        for contract in [
            json!({"adapter": adapter, "adapter_behavior": "3", "route_id": "x"}),
            json!({"adapter": adapter}),
            json!({"adapter_behavior": "3"}),
            json!({"adapter": null, "adapter_behavior": null}),
            json!({}),
            json!(null),
        ] {
            assert!(serves(&contract, adapter, three), "{contract}");
        }
        for contract in [
            json!({"adapter": adapter, "adapter_behavior": 3}),
            json!({"adapter": "gnode-openrouter-image-v1", "adapter_behavior": "3"}),
            json!({"adapter": 1}),
            json!([]),
            json!("fx-openrouter-image-v1"),
        ] {
            assert!(!serves(&contract, adapter, three), "{contract}");
        }
    }
}
