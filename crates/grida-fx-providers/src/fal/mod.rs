//! fal: `image.generate` and `image.edit` on the synchronous host, `video.generate` as a queue
//! long job, and `background.remove` (spec/providers.md §9.3; spec/capabilities.md).
//!
//! Credential `authorization: Key <FAL_KEY>`, attached only to requests to the run host
//! (`FAL_BASE_URL`, default `https://fal.run`) and the queue host (`https://queue.fal.run`), never
//! to a result download. Pictures go inline as data URLs under their own `image/*` kind (no
//! multipart, no uploads); a file of another kind is refused before sending. Every adapter
//! first refuses a route whose `contract.adapter` is present and is not its own (§9.3), since the
//! registry is keyed by provider, not model.
//!
//! What the adapters share lives here: the output host check ([`is_fal_media_url`]), the contract
//! guard, the answer root and reported cost, the run host's status classes (spec/providers.md
//! §4.2, §4.3 and §9.3) and the one place this module builds a `NotReceived`. Reading a picture
//! out of an answer (a data URL or a hosted download) is `output`.

pub mod background;
pub mod image;
mod output;
pub mod video;

use crate::adapter::{Adapter, CallRequest, Sent, Submitted};
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{HttpResponse, Phase, TransportError, TransportErrorKind};
use grida_fx_core::money::Usd;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

/// The image output host check of spec/providers.md §9.3: `https`, a host that is `fal.media` or
/// ends with `.fal.media` (lowercased, trailing dots removed), no userinfo, port absent or 443, no
/// fragment. A query is allowed.
pub fn is_fal_media_url(url: &str) -> bool {
    fal_media_url(url).is_some()
}

/// The parsed URL when it passes [`is_fal_media_url`]. A download sends the provider's text as
/// given; the transport parses it with the same parser, so the host checked here is the host
/// asked.
pub(crate) fn fal_media_url(url: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        // The URL parser drops a default port, so `:443` reads as absent here.
        || parsed.port().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let url::Host::Domain(domain) = parsed.host()? else {
        return None;
    };
    let host = domain.trim_end_matches('.').to_ascii_lowercase();
    let on_fal_media = host == "fal.media"
        || (host.ends_with(".fal.media") && !host.starts_with('.') && !host.contains(".."));
    on_fal_media.then_some(parsed)
}

/// The run-host client and the queue-host client (same key).
#[derive(Debug, Clone)]
pub struct FalClients {
    pub run: Client,
    pub queue: Client,
}

impl FalClients {
    pub fn new(setup: &Setup) -> FalClients {
        FalClients {
            run: setup.client(KeyName::Fal, &setup.endpoints.fal_run),
            queue: setup.client(KeyName::Fal, &setup.endpoints.fal_queue),
        }
    }
}

/// Registers fal's four adapters on provider `fal`.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let clients = FalClients::new(setup);
    let images = Arc::new(image::FalImages::new(clients.clone()));
    adapters.register("image.generate", "fal", Adapter::Request(images.clone()));
    adapters.register("image.edit", "fal", Adapter::Request(images));
    adapters.register(
        "video.generate",
        "fal",
        Adapter::Job(Arc::new(video::FalVideo::new(
            clients.clone(),
            Arc::clone(&setup.clock),
        ))),
    );
    adapters.register(
        "background.remove",
        "fal",
        Adapter::Request(Arc::new(background::FalBackground::new(clients))),
    );
}

/// The credential header and prefix fal takes on its run and queue hosts.
pub(crate) const CREDENTIAL_HEADER: &str = "authorization";
pub(crate) const CREDENTIAL_PREFIX: &str = "Key ";

/// The base-URL variable a run-host redirect points at (spec/providers.md §4.3).
pub(crate) const RUN_BASE_VARIABLE: &str = "FAL_BASE_URL";

/// Step 1 of spec/providers.md §5: `None` when the route is one of `capabilities` and its
/// contract's `adapter` is absent or `own`; else the refusal sentence.
pub(crate) fn contract_refusal(
    call: &CallRequest,
    own: &str,
    capabilities: &[&str],
) -> Option<String> {
    let adapter = call.route.contract.get("adapter");
    let serves_capability = capabilities.contains(&call.route.capability.as_str());
    let own_contract =
        crate::wire::is_absent(adapter) || adapter.and_then(Value::as_str) == Some(own);
    if serves_capability && own_contract {
        None
    } else {
        Some(format!(
            "{} is not a {} route this adapter serves",
            call.route.id(),
            call.route.capability
        ))
    }
}

/// The route's model as an endpoint id: surrounding whitespace and `/` removed.
pub(crate) fn endpoint(model: &str) -> &str {
    model.trim().trim_matches('/')
}

/// The answer's root (spec/providers.md §9.3): the payload's `data` when that is an object, else
/// the payload.
pub(crate) fn answer_root(payload: &Map<String, Value>) -> &Map<String, Value> {
    match payload.get("data") {
        Some(Value::Object(data)) => data,
        _ => payload,
    }
}

/// The reported cost: `usage.cost` at the payload's top level only (spec/providers.md §6, §9.3).
pub(crate) fn reported_cost(payload: &Map<String, Value>) -> Option<Usd> {
    payload
        .get("usage")
        .and_then(|usage| usage.get("cost"))
        .and_then(crate::wire::usd_ceil)
}

/// A request picture as a data URL, under its own kind (step 5 of spec/providers.md §5): a file the
/// call carries (`<member> has no bytes to send`), of an `image/*` kind (`<member> is <kind>, not
/// a picture`), whose store copy reads and is not empty. A file of another kind is refused before
/// anything leaves rather than sent under a picture's label, since fal may take a paid job it then
/// ends without a result.
pub(crate) fn picture(call: &CallRequest, value: &Value, member: &str) -> Result<String, String> {
    let file = crate::wire::request_file(call, value, member)?;
    if !file.kind.starts_with("image/") {
        return Err(format!("{member} is {}, not a picture", file.kind));
    }
    let bytes = crate::wire::read_file(file, member)?;
    Ok(crate::wire::data_url(&file.kind, &bytes))
}

/// Whether `text` is a provider id a handle or a reason may hold (spec/providers.md §7, §8):
/// `^[A-Za-z0-9_.:-]{1,96}$`.
pub(crate) fn is_safe_id(text: &str) -> bool {
    (1..=96).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

/// The wait a 408 or 429 asks for: the shared parser (spec/providers.md §4.3).
pub(crate) use crate::adapter::retry_after;

/// The sentence of a provider's "took nothing" answer: `retry_after`, when the provider sent one,
/// goes into the reason (spec/providers.md §4.3).
fn took_nothing_reason(client: &Client, text: &str, retry_after: Option<Duration>) -> String {
    match retry_after {
        Some(after) => client.reason(&format!(
            "{text}; retry-after {} s",
            grida_fx_core::value::format_number(after.as_secs_f64())
        )),
        None => client.reason(text),
    }
}

/// The one place this module builds a [`Sent::NotReceived`] (reason redacted and bounded).
pub(crate) fn not_received(client: &Client, text: &str, retry_after: Option<Duration>) -> Sent {
    Sent::NotReceived {
        reason: took_nothing_reason(client, text, retry_after),
        retry_after,
    }
}

/// The one place this module builds a [`Submitted::NotReceived`] (reason redacted and bounded).
pub(crate) fn submit_not_received(
    client: &Client,
    text: &str,
    retry_after: Option<Duration>,
) -> Submitted {
    Submitted::NotReceived {
        reason: took_nothing_reason(client, text, retry_after),
        retry_after,
    }
}

/// `<label> returned HTTP <n>`, plus the provider's request id when it is a safe field. Never a
/// response body (spec/providers.md §8, §9.3).
pub(crate) fn status_text(label: &str, response: &HttpResponse) -> String {
    let mut text = crate::wire::status_reason(label, response.status, None);
    if let Some(id) = crate::wire::request_id(response) {
        text.push_str(&format!(" (request {id})"));
    }
    text
}

/// How a run-host exchange becomes a [`Sent`] unless it is a 2xx (spec/providers.md §4.2, §4.3,
/// §9.3): a transport refusal is `Refused`; `not_sent` is `NotReceived`; `after_send` is
/// `Failed { None, true }`; 408 and 429 are `NotReceived`; a 3xx is `Failed { None, false }`
/// naming `FAL_BASE_URL`; another 4xx is `Failed { Some(0), false }` (fal validates before it
/// runs, and does not bill a refusal); a 5xx (or anything else) is `Failed { None, true }`.
#[allow(clippy::result_large_err)] // `Sent` is what every caller returns at once
pub(crate) fn classify_run(
    client: &Client,
    label: &str,
    exchange: Result<HttpResponse, TransportError>,
) -> Result<HttpResponse, Sent> {
    let response = match exchange {
        Ok(response) => response,
        Err(error) => return Err(transport_sent(client, label, &error)),
    };
    let status = response.status;
    if response.is_success() {
        return Ok(response);
    }
    let text = status_text(label, &response);
    Err(match status {
        408 => not_received(client, &text, retry_after(&response)),
        429 => not_received(
            client,
            &format!("{label} was rate limited (HTTP 429)"),
            retry_after(&response),
        ),
        300..=399 => Sent::Failed {
            reason: client.reason(&format!(
                "{label} was redirected (HTTP {status}); check {RUN_BASE_VARIABLE}"
            )),
            cost: None,
            retryable: false,
        },
        400..=499 => Sent::Failed {
            reason: client.reason(&text),
            cost: Some(Usd::ZERO),
            retryable: false,
        },
        _ => Sent::Failed {
            reason: client.reason(&text),
            cost: None,
            retryable: true,
        },
    })
}

/// A failed plain exchange (spec/providers.md §4.2).
fn transport_sent(client: &Client, label: &str, error: &TransportError) -> Sent {
    match (error.kind, error.phase) {
        (TransportErrorKind::Refused, _) => Sent::Refused {
            reason: client.reason(&format!("{label} was not sent: {}", error.reason)),
        },
        (_, Phase::NotSent) => not_received(
            client,
            &format!("{label} was not sent: {}", error.reason),
            None,
        ),
        (_, Phase::AfterSend) => Sent::Failed {
            reason: client.reason(&format!("{label} failed: {}", error.reason)),
            cost: None,
            retryable: true,
        },
    }
}

/// Whether a status member counts as set (spec/providers.md §9.3, "a truthy `error`"): anything
/// but `null`, `false`, `0`, `""`, `[]` and `{}`.
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => grida_fx_core::value::as_f64(n) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(members) => !members.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::RouteRef;
    use crate::clock::FakeClock;
    use crate::keys::Keys;
    use crate::transport::replay::NoNetwork;
    use indexmap::IndexMap;
    use serde_json::json;

    fn client() -> Client {
        let setup = Setup {
            transport: Arc::new(NoNetwork),
            keys: Keys::from_pairs(&[(KeyName::Fal, "fal-secret")]),
            endpoints: crate::setup::Endpoints::default(),
            clock: Arc::new(FakeClock::new()),
        };
        FalClients::new(&setup).run
    }

    fn call(capability: &str, contract: Value) -> CallRequest {
        CallRequest {
            route: RouteRef {
                capability: capability.into(),
                model: "img-a".into(),
                provider: "fal".into(),
                contract,
            },
            request: json!({}),
            files: IndexMap::new(),
            take: vec![1],
            key: "0".repeat(64),
            attempt: 1,
        }
    }

    #[test]
    fn output_hosts_are_fal_media_over_https_only() {
        for good in [
            "https://fal.media/files/a.png",
            "https://v3b.fal.media/files/a.png?sig=1",
            "https://V3.FAL.MEDIA/files/a.png",
            "https://v3.fal.media./files/a.png",
            "https://fal.media:443/a.png",
        ] {
            assert!(is_fal_media_url(good), "{good}");
        }
        for bad in [
            "http://v3b.fal.media/files/output.png",
            "https://127.0.0.1/output.png",
            "https://169.254.169.254/latest/meta-data",
            "https://cdn.example.test/output.png",
            "https://fal.media.evil.example/output.png",
            "https://user:secret@fal.media/output.png",
            "https://fal.media:8443/output.png",
            "https://fal.media/a.png#frag",
            "https://evilfal.media/a.png",
            "https://[::1]/a.png",
            "data:image/png;base64,aGVsbG8=",
            "not a url",
            "",
        ] {
            assert!(!is_fal_media_url(bad), "{bad}");
        }
    }

    #[test]
    fn contracts_and_capabilities_are_guarded() {
        let own = |contract| {
            contract_refusal(
                &call("image.generate", contract),
                "fx-fal-image-v1",
                &["image.generate"],
            )
        };
        assert_eq!(own(json!({})), None);
        assert_eq!(own(json!({"adapter": null})), None);
        assert_eq!(own(json!({"adapter": "fx-fal-image-v1", "x": 1})), None);
        assert_eq!(
            own(json!({"adapter": "gnode-openai-image-v1"})),
            Some("img-a@fal is not a image.generate route this adapter serves".into())
        );
        assert!(own(json!({"adapter": 1})).is_some());
        assert_eq!(
            contract_refusal(
                &call("video.generate", json!({})),
                "fx-fal-image-v1",
                &["image.generate"]
            ),
            Some("img-a@fal is not a video.generate route this adapter serves".into())
        );
    }

    #[test]
    fn roots_costs_and_endpoints() {
        let wrapped = json!({"data": {"images": []}, "usage": {"cost": 0.4}});
        let wrapped = wrapped.as_object().unwrap();
        assert!(answer_root(wrapped).contains_key("images"));
        assert_eq!(reported_cost(wrapped), Some(Usd(400_000)));
        let plain = json!({"data": "text", "images": [], "data_usage": 1});
        assert!(answer_root(plain.as_object().unwrap()).contains_key("images"));
        assert_eq!(reported_cost(plain.as_object().unwrap()), None);
        let nested = json!({"data": {"usage": {"cost": 0.4}}});
        assert_eq!(reported_cost(nested.as_object().unwrap()), None);
        assert_eq!(endpoint(" /fal-ai/birefnet/v2/ "), "fal-ai/birefnet/v2");
    }

    #[test]
    fn safe_ids_are_short_plain_tokens() {
        for good in [
            "req-1",
            "764cabcf-b745-4b3e-ae38-1200304cf45b",
            "a.b:c_d",
            &"x".repeat(96),
        ] {
            assert!(is_safe_id(good), "{good}");
        }
        for bad in [
            "",
            &"x".repeat(97),
            "a b",
            "a/b",
            "https://v3b.fal.media/a?X-Amz-Signature=1",
            "é",
            "a\nb",
        ] {
            assert!(!is_safe_id(bad), "{bad}");
        }
    }

    #[test]
    fn statuses_on_the_run_host() {
        let client = client();
        #[allow(clippy::result_large_err)] // `Sent` is what every caller returns at once
        let classify = |response: HttpResponse| classify_run(&client, "fal x", Ok(response));
        assert!(classify(HttpResponse::new(200, Vec::new())).is_ok());
        assert_eq!(
            classify(HttpResponse::new(429, Vec::new()).with_header("retry-after", "12")),
            Err(Sent::NotReceived {
                reason: "fal x was rate limited (HTTP 429); retry-after 12 s".into(),
                retry_after: Some(Duration::from_secs(12)),
            })
        );
        assert_eq!(
            classify(HttpResponse::new(408, Vec::new())),
            Err(Sent::not_received("fal x returned HTTP 408"))
        );
        assert_eq!(
            classify(HttpResponse::new(307, Vec::new())),
            Err(Sent::Failed {
                reason: "fal x was redirected (HTTP 307); check FAL_BASE_URL".into(),
                cost: None,
                retryable: false
            })
        );
        assert_eq!(
            classify(
                HttpResponse::new(422, b"fal-secret detail".to_vec())
                    .with_header("x-request-id", "req-7")
            ),
            Err(Sent::Failed {
                reason: "fal x returned HTTP 422 (request req-7)".into(),
                cost: Some(Usd::ZERO),
                retryable: false
            })
        );
        assert_eq!(
            classify(HttpResponse::new(502, Vec::new())),
            Err(Sent::Failed {
                reason: "fal x returned HTTP 502".into(),
                cost: None,
                retryable: true
            })
        );
        let refused = classify_run(
            &client,
            "fal x",
            Err(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off",
            )),
        );
        assert!(matches!(refused, Err(Sent::Refused { .. })));
    }

    #[test]
    fn truthiness_of_status_members() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!truthy(&falsy), "{falsy}");
        }
        for truthy_value in [
            json!(true),
            json!(1),
            json!("x"),
            json!([0]),
            json!({"a": 0}),
        ] {
            assert!(truthy(&truthy_value), "{truthy_value}");
        }
    }
}
