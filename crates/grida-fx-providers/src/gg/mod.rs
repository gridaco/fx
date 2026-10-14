//! Grida Gateway (GG): Grida's own provider. One organization key serves text, images, video and
//! 3D through one OpenAI-compatible API, billed to the organization's Grida AI credits
//! (src/gg/README.md).
//!
//! Credential `authorization: Bearer <GG_API_KEY>`; base `GG_BASE_URL`, default
//! `https://gateway.grida.ai/v1`. The adapters: [`image`] (`image.generate`, `image.edit`),
//! [`chat`] (`structured.generate`, `agent.turn`), [`video`] (`video.generate`) and [`mesh`]
//! (`mesh.generate`, `mesh.rig`), the last three long jobs sharing [`job`]. GG publishes every
//! model's exact request schema and validates a request against it before any paid work, so
//! these adapters check FX's own shapes and leave each model's limits to GG, whose refusals cost
//! nothing ([`classify`]).
//!
//! Shared here:
//! - [`Gateway`]: the client, the clock, and the session every request's `Idempotency-Key` is
//!   drawn from ([`Gateway::idempotency_key`]): `fx-<session>-<call key, 16 characters>-<attempt>`.
//!   The session is new for each set of adapters, so for each invocation: every send of one
//!   attempt carries the same key, and GG never takes it twice; every new attempt, in this
//!   invocation or a later one, carries a new key, and is new work. The one exception: a 429 whose
//!   error says `request_not_admitted` and `retry_with_new_idempotency_key` (GG's image slot was
//!   busy, and nothing was dispatched or held) renews the attempt's key, `.<n>` appended, so the
//!   engine's resend is an identity GG may admit ([`Gateway::exchange`]).
//! - [`classify`]: the statuses of a synchronous request (text, images).
//! - [`Gateway::priced`]: what an accepted synchronous request cost: GG's charge, once GG has
//!   settled it, from `x-gg-charged-mills` when `x-gg-billing-status` is `settled`, else from the
//!   request's billing (`x-gg-billing-url`), read until it settles or [`CHARGE_WAIT`] passes. A
//!   charge still unsettled then is unknown: the answer reports no cost, and the engine charges
//!   the whole hold. A failure of an accepted request whose cost is otherwise unknown is priced
//!   the same way ([`Gateway::charged`]). `x-gg-reserved-mills` is GG's authorization, never its
//!   charge, and is not read.
//! - [`mills`]: GG's money, exact decimal mills (1 000 mills = USD 1), as micro-dollars rounded up.

pub mod chat;
pub mod image;
pub mod job;
pub mod mesh;
pub mod video;

use crate::adapter::{Adapter, Answer, CallRequest, Sent};
use crate::clock::Clock;
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{
    Credential, HttpRequest, HttpResponse, Lane, Method, Phase, TransportError, TransportErrorKind,
};
use crate::wire;
use grida_fx_core::money::Usd;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

/// The provider name routes use (`model@gg`).
pub const PROVIDER: &str = "gg";

/// The variable a redirect points at (spec/providers.md §1; spec/adapters.md §3.3).
pub const BASE_URL_VARIABLE: &str = "GG_BASE_URL";

/// Where an organization manages its credits, keys and usage.
pub const CONSOLE: &str = "console.grida.co";

/// How long an answer waits for GG to settle its charge, from the first billing read's wait.
pub const CHARGE_WAIT: Duration = Duration::from_secs(30);

/// The wait before the first billing read; each later wait doubles, up to [`CHARGE_POLL_MAX`].
/// GG asks for no more than one read a second.
pub const CHARGE_POLL: Duration = Duration::from_secs(1);

/// The longest wait between billing reads.
pub const CHARGE_POLL_MAX: Duration = Duration::from_secs(8);

/// The longest single billing read.
const BILLING_READ_DEADLINE: Duration = Duration::from_secs(30);

/// The response cap of a billing read.
const BILLING_RESPONSE_BYTES: u64 = 1024 * 1024;

/// The label of a billing read's reasons.
pub const BILLING_LABEL: &str = "GG billing read";

/// GG's request ids (`OpaqueID`), as FX quotes them and puts them in a path: at most 96
/// characters, the first a letter or digit (so never `.` or `..`).
static REQUEST_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$").expect("a valid pattern"));

/// One invocation's way to GG: the client, the clock its billing reads wait on, and the session
/// its idempotency keys are drawn from.
#[derive(Clone)]
pub struct Gateway {
    pub client: Client,
    clock: Arc<dyn Clock>,
    session: String,
    /// How many times each attempt's key was renewed (module doc), by call key and attempt.
    renewals: Arc<Mutex<HashMap<(String, u32), u32>>>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("client", &self.client)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// The gateway of `setup`, with a new random session.
    pub fn new(setup: &Setup) -> Gateway {
        Gateway::with_session(setup, &new_session())
    }

    /// The gateway of `setup` with a given session (tests use a fixed one): 1 to 32 letters and
    /// digits.
    pub fn with_session(setup: &Setup, session: &str) -> Gateway {
        Gateway {
            client: client(setup),
            clock: Arc::clone(&setup.clock),
            session: session.to_string(),
            renewals: Arc::default(),
        }
    }

    /// The `authorization: Bearer <GG_API_KEY>` credential, or `GG_API_KEY
    /// is not set`.
    pub fn credential(&self) -> Result<Credential, String> {
        self.client.credential("authorization", "Bearer ")
    }

    /// The `Idempotency-Key` of `call`'s attempt (module doc): printable ASCII, no spaces, at most
    /// 72 characters.
    pub fn idempotency_key(&self, call: &CallRequest) -> String {
        let key: String = call
            .key
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(16)
            .collect();
        let key = if key.is_empty() { "0".to_string() } else { key };
        let renewal = self
            .renewals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(call.key.clone(), call.attempt))
            .copied()
            .unwrap_or(0);
        match renewal {
            0 => format!("fx-{}-{key}-{}", self.session, call.attempt),
            n => format!("fx-{}-{key}-{}.{n}", self.session, call.attempt),
        }
    }

    /// Renews the key of `call`'s attempt when `response` is a 429 by which GG says it admitted
    /// nothing and asks for a new key (module doc).
    pub(crate) fn renew_if_asked(&self, call: &CallRequest, response: &HttpResponse) {
        if response.status == 429 && asks_for_a_new_key(&response.body) {
            *self
                .renewals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry((call.key.clone(), call.attempt))
                .or_insert(0) += 1;
        }
    }

    /// Sends `call`'s paid synchronous request, renews its key if GG asks, and classifies the
    /// answer ([`classify`]); a failure whose cost is unknown costs GG's settled charge for it,
    /// when GG gives it in time ([`Gateway::charged`]).
    #[allow(clippy::result_large_err)] // `Sent` is what every caller returns at once
    pub async fn exchange(
        &self,
        call: &CallRequest,
        label: &str,
        request: HttpRequest,
    ) -> Result<HttpResponse, Sent> {
        let exchange = self.client.send(request).await;
        let failed = match &exchange {
            Ok(response) if !response.is_success() => {
                self.renew_if_asked(call, response);
                Some(response.clone())
            }
            _ => None,
        };
        match classify(&self.client, label, exchange) {
            Ok(response) => Ok(response),
            Err(sent) => Err(match failed {
                Some(response) => self.charged(sent, &response).await,
                None => sent,
            }),
        }
    }

    /// `sent`, a failure of the request `response` answered, with GG's settled charge as its cost
    /// when it has none ([`Gateway::charge`]): GG keeps the billing of every request it accepted,
    /// failed ones included, and settles a provider's refusal at what the provider charged.
    pub async fn charged(&self, sent: Sent, response: &HttpResponse) -> Sent {
        match sent {
            Sent::Failed {
                reason,
                cost: None,
                retryable,
            } => Sent::Failed {
                reason,
                cost: self.charge(response).await,
                retryable,
            },
            other => other,
        }
    }

    /// `answer` priced as the module doc says: its cost is GG's settled charge for the accepted
    /// `response`, when GG gives it in time ([`Gateway::charge`]), else none.
    pub async fn priced(&self, mut answer: Answer, response: &HttpResponse) -> Answer {
        answer.cost = self.charge(response).await;
        answer
    }

    /// GG's settled charge for an accepted synchronous request: `x-gg-charged-mills` when
    /// `x-gg-billing-status` is `settled`; else, when the response names its billing
    /// (`x-gg-billing-url`, `/v1/requests/<id>/billing`), `GET {base}/requests/<id>/billing` after
    /// [`CHARGE_POLL`], then again after each doubled wait up to [`CHARGE_POLL_MAX`], until its
    /// `billing.status` is `settled` or [`CHARGE_WAIT`] has passed. A read that fails in a way the
    /// next may not (a transport failure, 408, 429, a 5xx, a body that is not a JSON object, a
    /// charge still `reserved` or `pending`) waits for the next; any other ends the wait. `None`
    /// when GG has not said.
    pub async fn charge(&self, response: &HttpResponse) -> Option<Usd> {
        if let Some(charge) = settled_charge(response) {
            return Some(charge);
        }
        let id = billing_id(response)?;
        let credential = self.credential().ok()?;
        let deadline = self.clock.now() + CHARGE_WAIT;
        let mut wait = CHARGE_POLL;
        loop {
            if self.clock.now() + wait > deadline {
                return None;
            }
            self.clock.sleep(wait).await;
            match self.read_billing(&credential, &id, deadline).await {
                Billing::Charged(charge) => return Some(charge),
                Billing::Pending => {}
                Billing::Stop => return None,
            }
            wait = (wait * 2).min(CHARGE_POLL_MAX);
        }
    }

    /// One billing read of request `id` (see [`Gateway::charge`]).
    async fn read_billing(&self, credential: &Credential, id: &str, deadline: Duration) -> Billing {
        let remaining = deadline.saturating_sub(self.clock.now());
        if remaining.is_zero() {
            return Billing::Stop;
        }
        let request = HttpRequest::new(
            Method::Get,
            self.client.url(&format!("requests/{id}/billing")),
            Lane::Provider,
        )
        .credential(credential.clone())
        .timeout(remaining.min(BILLING_READ_DEADLINE))
        .max_response_bytes(BILLING_RESPONSE_BYTES);
        let response = match self.client.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::Refused => return Billing::Stop,
            Err(_) => return Billing::Pending,
        };
        match response.status {
            200..=299 => match wire::json_object(&response.body, BILLING_LABEL) {
                Ok(read) => request_billing(&read, id),
                Err(_) => Billing::Pending,
            },
            408 | 429 | 500..=599 => Billing::Pending,
            _ => Billing::Stop,
        }
    }

    /// A paid request: the credential and `call`'s `Idempotency-Key`.
    pub fn paid(
        &self,
        request: HttpRequest,
        credential: Credential,
        call: &CallRequest,
    ) -> HttpRequest {
        request
            .credential(credential)
            .header("idempotency-key", &self.idempotency_key(call))
    }

    /// A reason, redacted and bounded.
    pub fn reason(&self, text: &str) -> String {
        self.client.reason(text)
    }
}

/// A session: 16 hexadecimal characters from the operating system's random source, or, should it
/// fail, from the time, the process and a counter.
fn new_session() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        use sha2::{Digest, Sha256};
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!(
            "{nanos}:{}:{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        bytes.copy_from_slice(&Sha256::digest(seed.as_bytes())[..8]);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What one billing read saw.
enum Billing {
    /// GG settled the request at this charge.
    Charged(Usd),
    /// Not settled yet, or the read saw nothing: read again.
    Pending,
    /// The wait ends here.
    Stop,
}

/// A `request_billing` read of request `id`: its `charged_mills` once `billing.status` is
/// `settled`. A read about another request, a status GG does not document, or a settled charge
/// that is not GG's money ends the wait.
fn request_billing(read: &Map<String, Value>, id: &str) -> Billing {
    if read
        .get("request_id")
        .is_some_and(|other| other.as_str() != Some(id))
    {
        return Billing::Stop;
    }
    let Some(billing) = read.get("billing").and_then(Value::as_object) else {
        return Billing::Pending;
    };
    match billing.get("status").and_then(Value::as_str) {
        Some("settled") => billing
            .get("charged_mills")
            .and_then(Value::as_str)
            .and_then(mills)
            .map_or(Billing::Stop, Billing::Charged),
        Some("reserved" | "pending") => Billing::Pending,
        _ => Billing::Stop,
    }
}

/// `x-gg-charged-mills` of a response whose `x-gg-billing-status` is `settled`.
pub fn settled_charge(response: &HttpResponse) -> Option<Usd> {
    let status = response.header("x-gg-billing-status")?.trim();
    if !status.eq_ignore_ascii_case("settled") {
        return None;
    }
    response
        .header("x-gg-charged-mills")
        .and_then(|text| mills(text.trim()))
}

/// The request id of `x-gg-billing-url` when it is exactly `/v1/requests/<id>/billing` with one of
/// GG's ids; anything else names no billing FX reads.
pub fn billing_id(response: &HttpResponse) -> Option<String> {
    let path = response.header("x-gg-billing-url")?.trim();
    let id = path
        .strip_prefix("/v1/requests/")?
        .strip_suffix("/billing")?;
    REQUEST_ID.is_match(id).then(|| id.to_string())
}

/// GG's error asks for a new key: `request_not_admitted` and `retry_with_new_idempotency_key` are
/// both `true` (src/gg/README.md).
pub fn asks_for_a_new_key(body: &[u8]) -> bool {
    wire::error_envelope(body).is_some_and(|error| {
        ["request_not_admitted", "retry_with_new_idempotency_key"]
            .iter()
            .all(|flag| error.get(*flag) == Some(&Value::Bool(true)))
    })
}

/// The request id of a response: its header ([`wire::request_id`]), else its error's
/// `request_id` when that is one of GG's ids.
pub fn request_id(response: &HttpResponse) -> Option<String> {
    wire::request_id(response).or_else(|| {
        wire::error_envelope(&response.body)?
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|id| REQUEST_ID.is_match(id))
            .map(str::to_string)
    })
}

/// The client of GG's API.
pub fn client(setup: &Setup) -> Client {
    setup.client(KeyName::GridaGateway, &setup.endpoints.gg)
}

/// GG's error `code` (`invalid_request`, `insufficient_credit`, …) when the body is its error
/// envelope and the code is a plain lower-case name.
pub fn error_code(body: &[u8]) -> Option<String> {
    static CODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,63}$").expect("a valid pattern"));
    wire::error_envelope(body)?
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| CODE.is_match(code))
        .map(str::to_string)
}

/// A refusal whose sentence names the error code, the request id and what to do (spec/adapters.md
/// §7): `<label> returned HTTP <n>: <detail> (request <id>)`, then `; <hint>`.
fn status_text(label: &str, response: &HttpResponse, client: &Client) -> String {
    let detail = wire::safe_error_detail(&response.body, &client.redactor);
    let mut text = wire::status_reason(label, response.status, detail.as_deref());
    if let Some(id) = request_id(response) {
        text.push_str(&format!(" (request {id})"));
    }
    let hint = match (response.status, error_code(&response.body).as_deref()) {
        (401, _) => Some("check GG_API_KEY".to_string()),
        (402, Some("insufficient_credit")) => {
            Some(format!("add Grida AI credits at https://{CONSOLE}"))
        }
        (402, Some("cost_limit_exceeded")) => {
            Some("GG reserves more for this call than its ceiling allows".to_string())
        }
        (403, _) | (429, Some("key_budget_exceeded")) => Some(format!(
            "check the key and organization at https://{CONSOLE}"
        )),
        (429, _) if asks_for_a_new_key(&response.body) => {
            Some("nothing was admitted; it is sent again under a new Idempotency-Key".to_string())
        }
        _ => None,
    };
    if let Some(hint) = hint {
        text.push_str(&format!("; {hint}"));
    }
    text
}

/// How a synchronous exchange that is not a 2xx becomes a [`Sent`] (src/gg/README.md,
/// "Statuses"); a 2xx comes back as `Ok` for the adapter to parse. `label` names the call
/// (`GG image generation`, …).
///
/// - A transport failure: `Refused` when the transport refused the request itself, `NotReceived`
///   when it provably never left, else `Failed { None, retryable }` (spec/adapters.md §3.2).
/// - 3xx: `Failed { None, false }`, naming [`BASE_URL_VARIABLE`].
/// - 408 and 429 (but `key_budget_exceeded`): `NotReceived`, with the wait the response asks
///   for. GG admitted nothing. A 429 that asks for a new key has renewed it first
///   ([`Gateway::exchange`]).
/// - 409 `already_submitted`: `Failed { None, false }`, since the original was taken;
///   `idempotency_conflict` and `price_revision_changed`: `Failed { Some(0), false }`.
/// - Any other 4xx in GG's error envelope: `Failed { Some(0), false }`. GG refuses an invalid,
///   unauthorized or unaffordable request before any paid work and documents it as unbilled.
///   Without the envelope, the cost is `None`.
/// - 502 `upstream_rejected`: `Failed { None, false }`, since the model's provider refused the
///   request as given; any other 5xx: `Failed { None, true }`.
#[allow(clippy::result_large_err)] // `Sent` is what every caller returns at once
pub fn classify(
    client: &Client,
    label: &str,
    exchange: Result<HttpResponse, TransportError>,
) -> Result<HttpResponse, Sent> {
    let response = match exchange {
        Ok(response) => response,
        Err(error) => return Err(transport_failure(client, label, &error)),
    };
    if response.is_success() {
        return Ok(response);
    }
    let status = response.status;
    let code = error_code(&response.body);
    let envelope = wire::error_envelope(&response.body).is_some();
    let text = status_text(label, &response, client);
    let failed = |cost: Option<Usd>, retryable: bool| Sent::Failed {
        reason: client.reason(&text),
        cost,
        retryable,
    };
    let not_received = || Sent::NotReceived {
        reason: client.reason(&text),
        retry_after: crate::adapter::retry_after(&response),
    };
    Err(match (status, code.as_deref()) {
        (300..=399, _) => Sent::Failed {
            reason: client.reason(&format!(
                "{label} was redirected (HTTP {status}); check {BASE_URL_VARIABLE}"
            )),
            cost: None,
            retryable: false,
        },
        (429, Some("key_budget_exceeded")) => failed(Some(Usd::ZERO), false),
        (408 | 429, _) => not_received(),
        (409, Some("already_submitted")) => failed(None, false),
        (409, Some("idempotency_conflict" | "price_revision_changed")) => {
            failed(Some(Usd::ZERO), false)
        }
        (409 | 410, _) => failed(None, false),
        (400..=499, _) => failed(envelope.then_some(Usd::ZERO), false),
        (502, Some("upstream_rejected")) => failed(None, false),
        _ => failed(None, true),
    })
}

/// A failed exchange of a synchronous request (spec/adapters.md §3.2).
fn transport_failure(client: &Client, label: &str, error: &TransportError) -> Sent {
    match (error.kind, error.phase) {
        (TransportErrorKind::Refused, Phase::NotSent) => Sent::Refused {
            reason: client.reason(&format!("{label} was not sent: {}", error.reason)),
        },
        (_, Phase::NotSent) => {
            Sent::not_received(client.reason(&format!("{label} was not sent: {}", error.reason)))
        }
        (_, Phase::AfterSend) => Sent::Failed {
            reason: client.reason(&format!("{label} failed: {}", error.reason)),
            cost: None,
            retryable: true,
        },
    }
}

/// GG's money: an exact decimal number of mills (`^(0|[1-9][0-9]{0,19})(\.[0-9]{1,18})?$`), as
/// micro-dollars rounded up (1 mill is 1 000 micro-dollars, spec/adapters.md §5). `None` for
/// anything else.
pub fn mills(text: &str) -> Option<Usd> {
    static DECIMAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(0|[1-9][0-9]{0,19})(\.[0-9]{1,18})?$").expect("a valid pattern")
    });
    if !DECIMAL.is_match(text) {
        return None;
    }
    wire::decimal_micros_ceil(text, 1_000)
}

/// The reason with ` (request <id>)` when the response carries a safe request id.
pub(crate) fn with_request_id(reason: String, response: &HttpResponse) -> String {
    match request_id(response) {
        Some(id) => format!("{reason} (request {id})"),
        None => reason,
    }
}

/// `<route> is not a <capability> route this adapter serves`, then `: <why>` when there is one
/// (spec/adapters.md §4 step 1).
pub(crate) fn not_served(call: &CallRequest, why: Option<&str>) -> String {
    let mut reason = format!(
        "{} is not a {} route this adapter serves",
        call.route.id(),
        call.route.capability
    );
    if let Some(why) = why {
        reason.push_str(": ");
        reason.push_str(why);
    }
    reason
}

/// Reads a GG route contract (spec/adapters.md §4 step 1): `null` or an object whose `adapter`,
/// when present, is `adapter`, whose `adapter_behavior`, when present, is one `behavior` accepts
/// ([`crate::checks::serves`], the engine's check, made again here for a direct caller), and whose
/// other members are among `members` or `provider_options`, which the adapter reads when it builds
/// its body ([`crate::wire::provider_options`]). `null` members count as absent. The contract's
/// members are handed back.
pub(crate) fn read_contract(
    call: &CallRequest,
    capabilities: &[&str],
    adapter: &str,
    behavior: impl Fn(&Value) -> bool,
    members: &[&str],
) -> Result<serde_json::Map<String, Value>, String> {
    if !capabilities.contains(&call.route.capability.as_str())
        || !crate::checks::serves(&call.route.contract, adapter, behavior)
    {
        return Err(not_served(call, None));
    }
    let Some(contract) = call.route.contract.as_object() else {
        return Ok(serde_json::Map::new());
    };
    if let Some(member) = contract.keys().find(|key| {
        !matches!(
            key.as_str(),
            "adapter" | "adapter_behavior" | "provider_options"
        ) && !members.contains(&key.as_str())
    }) {
        return Err(not_served(
            call,
            Some(&format!("the contract takes no member {member}")),
        ));
    }
    Ok(contract
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

/// Registers GG's adapters on provider `gg`: images, structured output, agent turns, video, 3D
/// models and rigs.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let gateway = Gateway::new(setup);
    let images = Arc::new(image::GgImages::new(gateway.clone()));
    adapters.register("image.generate", PROVIDER, Adapter::Request(images.clone()));
    adapters.register("image.edit", PROVIDER, Adapter::Request(images));
    adapters.register(
        "structured.generate",
        PROVIDER,
        Adapter::Request(Arc::new(chat::GgStructured::new(gateway.clone()))),
    );
    adapters.register(
        "agent.turn",
        PROVIDER,
        Adapter::Request(Arc::new(chat::GgAgent::new(gateway.clone()))),
    );
    let clock = || Arc::clone(&setup.clock);
    adapters.register(
        "video.generate",
        PROVIDER,
        Adapter::Job(Arc::new(video::GgVideo::new(gateway.clone(), clock()))),
    );
    adapters.register(
        "mesh.generate",
        PROVIDER,
        Adapter::Job(Arc::new(mesh::GgMesh::new(gateway.clone(), clock()))),
    );
    adapters.register(
        "mesh.rig",
        PROVIDER,
        Adapter::Job(Arc::new(mesh::GgRig::new(gateway, clock()))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::RouteRef;
    use crate::clock::SystemClock;
    use crate::keys::Keys;
    use crate::transport::replay::NoNetwork;
    use serde_json::json;

    fn setup() -> Setup {
        Setup::new(
            Arc::new(NoNetwork),
            Keys::from_pairs(&[(KeyName::GridaGateway, "sk-gg-v1-test-secret")]),
            Arc::new(SystemClock::new()),
        )
    }

    fn client() -> Client {
        super::client(&setup())
    }

    fn sent(status: u16, body: Value) -> Sent {
        classify(
            &client(),
            "GG image generation",
            Ok(HttpResponse::json(status, &body)),
        )
        .unwrap_err()
    }

    fn envelope(code: &str) -> Value {
        json!({"error": {"code": code, "message": "m", "param": null, "type": "invalid_request_error"}})
    }

    fn call(key: &str, attempt: u32) -> CallRequest {
        CallRequest {
            route: RouteRef {
                capability: "image.generate".into(),
                model: "acme/img-a".into(),
                provider: PROVIDER.into(),
                contract: json!({}),
            },
            request: json!({}),
            files: indexmap::IndexMap::new(),
            take: vec![1],
            key: key.into(),
            attempt,
        }
    }

    #[test]
    fn statuses_classify_per_the_table() {
        let free = |sent: Sent| {
            matches!(
                sent,
                Sent::Failed {
                    cost: Some(Usd::ZERO),
                    retryable: false,
                    ..
                }
            )
        };
        for code in [
            "invalid_request",
            "invalid_json",
            "unsupported_parameter",
            "invalid_idempotency_key",
        ] {
            assert!(free(sent(400, envelope(code))), "{code}");
        }
        assert!(free(sent(401, envelope("invalid_api_key"))));
        assert!(free(sent(402, envelope("insufficient_credit"))));
        assert!(free(sent(402, envelope("cost_limit_exceeded"))));
        assert!(free(sent(403, envelope("insufficient_scope"))));
        assert!(free(sent(403, envelope("organization_inactive"))));
        assert!(free(sent(404, envelope("model_not_found"))));
        assert!(free(sent(413, envelope("request_too_large"))));
        assert!(free(sent(415, envelope("unsupported_media_type"))));
        assert!(free(sent(429, envelope("key_budget_exceeded"))));
        assert!(free(sent(409, envelope("idempotency_conflict"))));
        assert!(free(sent(409, envelope("price_revision_changed"))));
        assert!(matches!(
            sent(400, json!("not an envelope")),
            Sent::Failed {
                cost: None,
                retryable: false,
                ..
            }
        ));
        for (status, body) in [
            (429, envelope("rate_limit_exceeded")),
            (429, envelope("concurrency_limit_exceeded")),
            (429, json!({})),
            (408, json!({})),
        ] {
            assert!(
                matches!(sent(status, body.clone()), Sent::NotReceived { .. }),
                "{status} {body}"
            );
        }
        for (status, body, retryable) in [
            (409, envelope("already_submitted"), false),
            (410, envelope("job_expired"), false),
            (502, envelope("upstream_rejected"), false),
            (502, envelope("upstream_unavailable"), true),
            (502, envelope("invalid_upstream_response"), true),
            (503, envelope("temporarily_unavailable"), true),
            (503, envelope("settlement_pending"), true),
            (500, envelope("internal_error"), true),
            (504, json!({}), true),
        ] {
            assert_eq!(
                sent(status, body.clone()),
                Sent::Failed {
                    reason: match &sent(status, body.clone()) {
                        Sent::Failed { reason, .. } => reason.clone(),
                        _ => unreachable!(),
                    },
                    cost: None,
                    retryable,
                },
                "{status} {body}"
            );
        }
        let Sent::Failed { reason, .. } = sent(302, json!({})) else {
            unreachable!()
        };
        assert_eq!(
            reason,
            "GG image generation was redirected (HTTP 302); check GG_BASE_URL"
        );
        assert!(classify(&client(), "X", Ok(HttpResponse::new(200, b"{}".to_vec()))).is_ok());
    }

    #[test]
    fn refusals_name_the_code_the_request_and_what_to_do() {
        let response = HttpResponse::json(402, &envelope("insufficient_credit"))
            .with_header("x-request-id", "req_abc-1");
        let Sent::Failed { reason, .. } =
            classify(&client(), "GG image generation", Ok(response)).unwrap_err()
        else {
            unreachable!()
        };
        assert_eq!(
            reason,
            "GG image generation returned HTTP 402: type=invalid_request_error; \
             code=insufficient_credit (request req_abc-1); add Grida AI credits at \
             https://console.grida.co"
        );
        let Sent::Failed { reason, .. } = sent(401, envelope("invalid_api_key")) else {
            unreachable!()
        };
        assert!(reason.ends_with("; check GG_API_KEY"), "{reason}");
        let Sent::NotReceived {
            reason,
            retry_after,
        } = classify(
            &client(),
            "X",
            Ok(HttpResponse::json(429, &envelope("rate_limit_exceeded"))
                .with_header("retry-after", "3")),
        )
        .unwrap_err()
        else {
            unreachable!()
        };
        assert_eq!(
            reason,
            "X returned HTTP 429: type=invalid_request_error; code=rate_limit_exceeded"
        );
        assert_eq!(retry_after, Some(std::time::Duration::from_secs(3)));
    }

    #[test]
    fn transport_phases_classify() {
        let failure = |error| classify(&client(), "X", Err(error)).unwrap_err();
        assert!(matches!(
            failure(TransportError::not_sent(
                TransportErrorKind::Connect,
                "connection refused"
            )),
            Sent::NotReceived { .. }
        ));
        assert_eq!(
            failure(TransportError::not_sent(
                TransportErrorKind::Refused,
                "the network is off"
            )),
            Sent::Refused {
                reason: "X was not sent: the network is off".into()
            }
        );
        assert!(matches!(
            failure(TransportError::after_send(
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
    fn money_is_exact_decimal_mills_rounded_up() {
        assert_eq!(mills("0"), Some(Usd::ZERO));
        assert_eq!(mills("1000"), Some(Usd(1_000_000)));
        assert_eq!(mills("16.5928"), Some(Usd(16_593)));
        assert_eq!(mills("9240.48"), Some(Usd(9_240_480)));
        assert_eq!(mills("0.000000000000000001"), Some(Usd(1)));
        for bad in ["", "-1", "1e3", "01", "1.", ".5", " 1", "1,0", "NaN"] {
            assert_eq!(mills(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn idempotency_keys_are_one_per_attempt_and_session() {
        let gateway = Gateway::with_session(&setup(), "0123456789abcdef");
        let key = "a".repeat(64);
        assert_eq!(
            gateway.idempotency_key(&call(&key, 1)),
            "fx-0123456789abcdef-aaaaaaaaaaaaaaaa-1"
        );
        assert_eq!(
            gateway.idempotency_key(&call(&key, 6)),
            "fx-0123456789abcdef-aaaaaaaaaaaaaaaa-6"
        );
        assert_eq!(
            gateway.idempotency_key(&call("", 1)),
            "fx-0123456789abcdef-0-1"
        );
        let other = Gateway::with_session(&setup(), "fedcba9876543210");
        assert_ne!(
            other.idempotency_key(&call(&key, 1)),
            gateway.idempotency_key(&call(&key, 1))
        );
        // A 429 that asks for a new key renews that attempt's key, and only that attempt's.
        let busy = |flags: Value| {
            let mut body = envelope("concurrency_limit_exceeded");
            body["error"]
                .as_object_mut()
                .unwrap()
                .extend(flags.as_object().unwrap().clone());
            HttpResponse::json(429, &body)
        };
        let both = json!({"request_not_admitted": true, "retry_with_new_idempotency_key": true});
        gateway.renew_if_asked(&call(&key, 1), &busy(json!({"request_not_admitted": true})));
        let mut unavailable = busy(both.clone());
        unavailable.status = 503;
        gateway.renew_if_asked(&call(&key, 1), &unavailable);
        assert_eq!(
            gateway.idempotency_key(&call(&key, 1)),
            "fx-0123456789abcdef-aaaaaaaaaaaaaaaa-1"
        );
        gateway.renew_if_asked(&call(&key, 1), &busy(both.clone()));
        let renewed = gateway.clone();
        renewed.renew_if_asked(&call(&key, 1), &busy(both));
        assert_eq!(
            gateway.idempotency_key(&call(&key, 1)),
            "fx-0123456789abcdef-aaaaaaaaaaaaaaaa-1.2",
            "clones share their renewals"
        );
        assert_eq!(
            gateway.idempotency_key(&call(&key, 2)),
            "fx-0123456789abcdef-aaaaaaaaaaaaaaaa-2"
        );
        let session = new_session();
        assert_eq!(session.len(), 16);
        assert!(session.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(new_session(), session, "a new session each time");
    }

    #[test]
    fn charges_and_billing_are_read_strictly() {
        let response = |headers: &[(&str, &str)]| {
            headers.iter().fold(
                HttpResponse::new(200, Vec::new()),
                |response, (name, value)| response.with_header(name, value),
            )
        };
        assert_eq!(
            settled_charge(&response(&[
                ("x-gg-billing-status", "settled"),
                ("x-gg-charged-mills", "0.185")
            ])),
            Some(Usd(185))
        );
        for headers in [
            &[
                ("x-gg-billing-status", "pending"),
                ("x-gg-charged-mills", "1"),
            ][..],
            &[("x-gg-charged-mills", "1")],
            &[
                ("x-gg-billing-status", "settled"),
                ("x-gg-charged-mills", "1e3"),
            ],
            &[("x-gg-billing-status", "settled")],
        ] {
            assert_eq!(settled_charge(&response(headers)), None, "{headers:?}");
        }
        let id = |path: &str| billing_id(&response(&[("x-gg-billing-url", path)]));
        assert_eq!(
            id("/v1/requests/req_a-1.b/billing").as_deref(),
            Some("req_a-1.b")
        );
        for bad in [
            "https://elsewhere.example/v1/requests/req_a/billing",
            "//elsewhere.example/v1/requests/req_a/billing",
            "/v1/requests/req_a/billing?x=1",
            "/v1/requests/../billing",
            "/v1/requests/./billing",
            "/v1/requests/req%2Fa/billing",
            "/v1/requests//billing",
            "/v2/requests/req_a/billing",
        ] {
            assert_eq!(id(bad), None, "{bad}");
        }
        let read = |value: Value| match request_billing(value.as_object().unwrap(), "req_a") {
            Billing::Charged(charge) => Some(Some(charge)),
            Billing::Pending => Some(None),
            Billing::Stop => None,
        };
        let billing = |status: &str, charged: Value| {
            json!({"object": "request_billing", "request_id": "req_a", "billing": {
                "status": status, "currency": "USD", "unit": "mill", "price_revision": "r",
                "authorized_mills": "1000", "reserved_mills": "0", "charged_mills": charged,
                "refunded_mills": "0", "usage": null}})
        };
        assert_eq!(
            read(billing("settled", json!("185"))),
            Some(Some(Usd(185_000)))
        );
        assert_eq!(read(billing("settled", json!("0"))), Some(Some(Usd::ZERO)));
        assert_eq!(read(billing("pending", json!(null))), Some(None));
        assert_eq!(read(billing("reserved", json!(null))), Some(None));
        assert_eq!(read(billing("settled", json!(185))), None);
        assert_eq!(read(billing("refunded", json!("1"))), None);
        let mut other = billing("settled", json!("185"));
        other["request_id"] = json!("req_b");
        assert_eq!(read(other), None, "a read about another request");
        assert!(asks_for_a_new_key(
            &serde_json::to_vec(&json!({"error": {"code": "concurrency_limit_exceeded",
                "request_not_admitted": true, "retry_with_new_idempotency_key": true}}))
            .unwrap()
        ));
        assert!(!asks_for_a_new_key(
            &serde_json::to_vec(&json!({"error": {"code": "concurrency_limit_exceeded",
                "request_not_admitted": true, "retry_with_new_idempotency_key": "true"}}))
            .unwrap()
        ));
        let error_id = HttpResponse::json(
            400,
            &json!({"error": {"code": "invalid_request", "message": "m", "param": "size",
                "type": "invalid_request_error", "request_id": "att_1"}}),
        );
        assert_eq!(request_id(&error_id).as_deref(), Some("att_1"));
        assert_eq!(
            request_id(&error_id.with_header("x-request-id", "req_h")).as_deref(),
            Some("req_h")
        );
    }

    #[test]
    fn contracts_are_read_or_refused() {
        let one = |v: &Value| v == &json!(1);
        let read = |contract: Value| {
            let mut call = call("k", 1);
            call.route.contract = contract;
            read_contract(&call, &["image.generate"], "gg-x", one, &["quality"])
        };
        assert_eq!(read(json!(null)), Ok(serde_json::Map::new()));
        assert_eq!(read(json!({})), Ok(serde_json::Map::new()));
        let full =
            read(json!({"adapter": "gg-x", "adapter_behavior": 1, "quality": "max"})).unwrap();
        assert_eq!(full.get("quality"), Some(&json!("max")));
        assert!(read(json!({"adapter": null, "adapter_behavior": null, "quality": null})).is_ok());
        assert!(read(json!({"provider_options": {"gg": {"seed": 1}}})).is_ok());
        let foreign = "acme/img-a@gg is not a image.generate route this adapter serves";
        for contract in [
            json!({"adapter": "gg-y"}),
            json!({"adapter_behavior": "1"}),
            json!({"adapter": 1}),
            json!([]),
        ] {
            assert_eq!(read(contract.clone()).unwrap_err(), foreign, "{contract}");
        }
        assert_eq!(
            read(json!({"route_id": "x"})).unwrap_err(),
            format!("{foreign}: the contract takes no member route_id")
        );
        let mut video = call("k", 1);
        video.route.capability = "video.generate".into();
        assert_eq!(
            read_contract(&video, &["image.generate"], "gg-x", one, &[]).unwrap_err(),
            "acme/img-a@gg is not a video.generate route this adapter serves"
        );
    }
}
