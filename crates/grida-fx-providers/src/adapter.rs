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
use crate::redact::Redactor;
use crate::transport::HttpResponse;
use grida_fx_core::money::{Units, Usd};
use grida_fx_core::routes::{PriceUnit, Route};
use indexmap::IndexMap;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The route a call goes to (spec/identity.md §7): what an adapter needs to address the
/// provider, and the limits its table declares. Prices, pacing and the features the engine judges
/// are the engine's business and are not here.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRef {
    pub capability: String,
    pub model: String,
    pub provider: String,
    /// The route's contract object, as its table's reading completed it (spec/identity.md §7);
    /// `{}` when none.
    pub contract: Value,
    /// What the route's table declares of the calls it takes ([`RouteLimits`]).
    pub limits: RouteLimits,
}

impl RouteRef {
    /// The route a call on `route` goes to, its limits included.
    pub fn of(route: &Route) -> RouteRef {
        RouteRef {
            capability: route.capability.clone(),
            model: route.model.clone(),
            provider: route.provider.clone(),
            contract: route.contract.clone(),
            limits: RouteLimits::of(route),
        }
    }

    /// `model@provider`.
    pub fn id(&self) -> String {
        format!("{}@{}", self.model, self.provider)
    }
}

/// What a route's table declares of the calls it takes, for an adapter that holds a request to
/// the route's limits rather than its own: the GG and generic image adapters to the picture limit,
/// OpenRouter's generic image adapter to whether a generate takes references, fal's video adapter
/// to the tiers, the length and whether a first frame is needed (their references). None of it is
/// part of the route's fingerprint (spec/identity.md §7), so an adapter holds a request to it only
/// before sending, never when a job already sent is collected (spec/adapters.md §4 step 4, §6).
/// Default: nothing declared.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RouteLimits {
    /// `max_input_pictures`: the most input pictures one call takes; `None` when none is
    /// declared.
    pub max_input_pictures: Option<u32>,
    /// `price.max_units` of a route priced per second: the most seconds one call carries.
    pub max_seconds: Option<Units>,
    /// The setting the price is tiered by (`price.by`), and its tiers' names in the table's order.
    pub tiers: Option<(String, Vec<String>)>,
    /// Whether the route declares the feature `image_input`: an `image.generate` route takes
    /// `references` (spec/capabilities.md §2).
    pub image_input: bool,
    /// Whether the route declares the feature `text_to_video`: it draws a clip from the prompt
    /// alone (spec/capabilities.md §6).
    pub text_to_video: bool,
}

impl RouteLimits {
    /// The limits `route`'s table declares.
    pub fn of(route: &Route) -> RouteLimits {
        let price = &route.price;
        RouteLimits {
            max_input_pictures: route.max_input_pictures,
            max_seconds: price.max_units.filter(|_| price.unit == PriceUnit::Second),
            tiers: price
                .by
                .clone()
                .map(|by| (by, price.tiers.keys().cloned().collect())),
            image_input: route.features.contains("image_input"),
            text_to_video: route.features.contains("text_to_video"),
        }
    }

    /// The names of the price's tiers when it is tiered by `setting`.
    pub fn tiers_by(&self, setting: &str) -> Option<&[String]> {
        match &self.tiers {
            Some((by, names)) if by == setting => Some(names),
            _ => None,
        }
    }

    /// [`RouteLimits::max_seconds`] in whole seconds, rounded down.
    pub fn max_whole_seconds(&self) -> Option<u64> {
        let units = self.max_seconds?;
        let divisor = 10i128.checked_pow(units.scale)?;
        u64::try_from(units.mantissa.max(0) / divisor).ok()
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

/// How a provider bills its calls (spec/adapters.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    /// In US dollars, to the account of the key FX sends: what the ceiling bounds.
    Dollars,
    /// Against the user's own plan in a runtime they signed into (Codex): never in dollars, and
    /// unseen by FX. A live run whose calls all bill this way needs no ceiling.
    Plan,
}

/// What a call consumed besides dollars, as its provider reported it (spec/store.md §5): kept in
/// the call record, for a call that bills a plan (whose dollar cost is unknown) above all.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    /// The runtime that served the call and its version, such as `codex 0.160.1`.
    pub runtime: Option<String>,
    /// The model the runtime's agent ran (not necessarily the one that drew).
    pub agent_model: Option<String>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

impl Usage {
    /// The record's `usage` object: the members that are known, in this order.
    pub fn to_value(&self, billing: Billing) -> Value {
        let mut map = serde_json::Map::new();
        let billing = match billing {
            Billing::Dollars => "dollars",
            Billing::Plan => "plan",
        };
        map.insert("billing".into(), Value::from(billing));
        let text = [
            ("runtime", &self.runtime),
            ("agent_model", &self.agent_model),
        ];
        for (name, value) in text {
            if let Some(value) = value {
                map.insert(name.into(), Value::from(value.as_str()));
            }
        }
        let counts = [
            ("input_tokens", self.input_tokens),
            ("cached_input_tokens", self.cached_input_tokens),
            ("output_tokens", self.output_tokens),
        ];
        for (name, value) in counts {
            if let Some(value) = value {
                map.insert(name.into(), Value::from(value));
            }
        }
        Value::Object(map)
    }
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
    /// What else the call consumed, when the provider said ([`Usage`]).
    pub usage: Option<Box<Usage>>,
}

impl Answer {
    /// An answer with `data`, no files, and the reported `cost`.
    pub fn new(data: Value, cost: Option<Usd>) -> Answer {
        Answer {
            files: IndexMap::new(),
            data,
            cost,
            usage: None,
        }
    }

    /// The same answer with the usage the provider reported.
    pub fn with_usage(mut self, usage: Usage) -> Answer {
        self.usage = Some(Box::new(usage));
        self
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
    /// `retry_after` is the wait the provider asked for before that ([`retry_after`]; `None` when
    /// it asked for none): the engine waits at least its own backoff, and at least `retry_after`
    /// up to its cap (spec/adapters.md §3.3).
    NotReceived {
        reason: String,
        retry_after: Option<Duration>,
    },
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
    /// Provably not received; may be submitted again under the same attempt, at $0. `retry_after`
    /// as for [`Sent::NotReceived`].
    NotReceived {
        reason: String,
        retry_after: Option<Duration>,
    },
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

impl Sent {
    /// [`Sent::NotReceived`] with no wait asked for (a connection that failed before sending).
    pub fn not_received(reason: impl Into<String>) -> Sent {
        Sent::NotReceived {
            reason: reason.into(),
            retry_after: None,
        }
    }
}

impl Submitted {
    /// [`Submitted::NotReceived`] with no wait asked for.
    pub fn not_received(reason: impl Into<String>) -> Submitted {
        Submitted::NotReceived {
            reason: reason.into(),
            retry_after: None,
        }
    }
}

/// The longest wait [`retry_after`] reports; a provider asking for more gets this (the engine
/// caps it further).
pub const RETRY_AFTER_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// The wait a response asks for before the same request is sent again, for
/// [`Sent::NotReceived`]'s `retry_after`: `retry-after-ms` (milliseconds, the OpenAI form) when it
/// is a number, else `retry-after` as delta-seconds or an HTTP date (IMF-fixdate, measured from
/// now; a date in the past is no wait). `None` when neither header is there or neither parses.
/// Fractions are honoured; a value above [`RETRY_AFTER_LIMIT`] is that limit.
pub fn retry_after(response: &HttpResponse) -> Option<Duration> {
    retry_after_at(response, SystemTime::now())
}

/// [`retry_after`] with the current time given, so an HTTP date is testable.
pub fn retry_after_at(response: &HttpResponse, now: SystemTime) -> Option<Duration> {
    if let Some(millis) = response.header("retry-after-ms").and_then(seconds) {
        return Some(bounded_wait(millis / 1000.0));
    }
    let value = response.header("retry-after")?.trim();
    if let Some(secs) = seconds(value) {
        return Some(bounded_wait(secs));
    }
    let at = http_date(value)?;
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(bounded_wait(at.saturating_sub(now) as f64))
}

/// A non-negative decimal number of at most 12 digits and 9 decimals.
fn seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let digits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(whole, 12) || fraction.is_some_and(|f| !digits(f, 9)) {
        return None;
    }
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn bounded_wait(secs: f64) -> Duration {
    Duration::try_from_secs_f64(secs)
        .unwrap_or(RETRY_AFTER_LIMIT)
        .min(RETRY_AFTER_LIMIT)
}

/// Seconds since the Unix epoch of an IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`).
fn http_date(text: &str) -> Option<u64> {
    let mut words = text.split_ascii_whitespace();
    let weekday = words.next()?;
    let day: u64 = words.next()?.parse().ok()?;
    let month = words.next()?;
    let year: i64 = words.next()?.parse().ok()?;
    let time = words.next()?;
    if words.next()? != "GMT" || words.next().is_some() {
        return None;
    }
    const WEEKDAYS: [&str; 7] = ["Mon,", "Tue,", "Wed,", "Thu,", "Fri,", "Sat,", "Sun,"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !WEEKDAYS.contains(&weekday) {
        return None;
    }
    let month = MONTHS.iter().position(|m| *m == month)? as i64 + 1;
    let mut clock = time.split(':').map(|part| {
        (part.len() == 2)
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    if clock.next().is_some()
        || !(1..=31).contains(&day)
        || !(1970..=9999).contains(&year)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from the civil date (proleptic Gregorian), after Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days)
        .ok()
        .map(|days| days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// The outcome of collecting a long job by its handle. Collecting bills nothing new.
#[derive(Debug, Clone, PartialEq)]
pub enum Collected {
    /// The job's answer.
    Answered(Answer),
    /// The job ended without a result: the record becomes `settled`, the call fails
    /// `call_failed`, and a later run submits it anew. `cost` is what the ended job costs
    /// (spec/adapters.md §6): the provider's own figure for it, as for an answer when the job
    /// finished with an unusable result; `Some(0)` only where its reference says so
    /// (spec/providers.md §2); `None` otherwise, and the run that submitted the job then charges
    /// its whole hold.
    Ended { reason: String, cost: Option<Usd> },
    /// Collecting failed and the job may still be running: the record stays `submitted`, the
    /// call fails `call_failed`, and a later run collects again.
    Unreachable { reason: String },
}

/// The contract an adapter FX ships serves, as a route's contract names it (spec/providers.md §2):
/// its `adapter`, and its `adapter_behavior` as canonical JSON (spec/identity.md §2), such as
/// `"1"` for the string and `1` for the number. The engine holds each call's route to it before
/// the adapter sees the call ([`crate::checks::contract`]; spec/adapters.md §4 step 1), and core's
/// [`grida_fx_core::routes::BUILT_IN_ADAPTERS`] and [`grida_fx_core::routes::FAMILY_ADAPTERS`],
/// which complete a route's contract when a table is read (spec/identity.md §7), name the same
/// (`tests/routes.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub adapter: &'static str,
    pub behavior: &'static str,
}

impl Identity {
    /// Whether `value`, a contract's `adapter_behavior`, is the behaviour this adapter serves:
    /// its canonical JSON is [`Identity::behavior`].
    pub fn behaves(&self, value: &Value) -> bool {
        grida_fx_core::value::canon(value) == self.behavior
    }

    /// The identity of an adapter known only once its project is planned, a project's own
    /// (spec/adapters.md §9): `adapter` (`<path>#<attribute>`) and its behaviour `behavior` as
    /// canonical JSON. Each distinct text is kept once for the life of the process, so a
    /// long-lived process that plans again keeps one copy of each.
    pub fn interned(adapter: &str, behavior: &str) -> Identity {
        Identity {
            adapter: intern(adapter),
            behavior: intern(behavior),
        }
    }
}

/// `text`, kept once for the life of the process ([`Identity::interned`]).
fn intern(text: &str) -> &'static str {
    static KEPT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> =
        std::sync::LazyLock::new(Default::default);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = kept.get(text) {
        return found;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    kept.insert(leaked);
    leaked
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

    /// The contract this adapter serves; `None`, the default, names none. For an adapter that names
    /// one (every adapter FX ships, and a project adapter), the engine refuses a call before
    /// [`RequestAdapter::send`] as spec/adapters.md §4 steps 1 and 2 say, and holds an answer to
    /// the capability's every-route check after [`RequestAdapter::check`] (spec/adapters.md
    /// §3.4).
    fn identity(&self) -> Option<Identity> {
        None
    }

    /// The adapter's judgement of its route's other contract members (spec/adapters.md §4 step
    /// 1), which the engine makes after its own check of the contract and before its check of the
    /// request. A refusal is a sentence. None by default.
    fn check_contract(&self, _call: &CallRequest) -> Result<(), String> {
        Ok(())
    }

    /// How this adapter's calls are billed ([`Billing`]); dollars unless it says otherwise.
    fn billing(&self) -> Billing {
        Billing::Dollars
    }

    /// The redactor every reason of this adapter's calls passes through last (spec/adapters.md
    /// §7). The engine's retry owner redacts with it each reason it reports: the adapter's own,
    /// its check's, the engine's refusals before a send ([`crate::checks::before_send`]), and the
    /// engine's checks of the answer. A live invocation's adapters answer with the redactor over
    /// every key of the invocation ([`crate::live::adapters`]); the default masks the known secret
    /// shapes only.
    fn redactor(&self) -> Redactor {
        Redactor::default()
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

    /// As [`RequestAdapter::identity`]: the engine's refusals come before
    /// [`LongJob::submit`], never before [`LongJob::collect`].
    fn identity(&self) -> Option<Identity> {
        None
    }

    /// As [`RequestAdapter::check_contract`].
    fn check_contract(&self, _call: &CallRequest) -> Result<(), String> {
        Ok(())
    }

    /// As [`RequestAdapter::billing`].
    fn billing(&self) -> Billing {
        Billing::Dollars
    }

    /// As [`RequestAdapter::redactor`].
    fn redactor(&self) -> Redactor {
        Redactor::default()
    }
}

/// A runtime some adapters share: a program the user installed and signed into (Codex), started
/// on an invocation's first call that needs it and kept for the others (spec/adapters.md §1). The
/// engine owns its life: it is shut down with the run ([`crate::Adapters::shutdown`]).
pub trait Runtime: Send + Sync {
    /// Ends the runtime and everything it started. Whether everything is verifiably ended; a
    /// runtime that never started is.
    fn shutdown(&self) -> BoxFuture<'_, bool>;
}

/// One registered adapter.
#[derive(Clone)]
pub enum Adapter {
    Request(Arc<dyn RequestAdapter>),
    Job(Arc<dyn LongJob>),
}

impl Adapter {
    /// How the adapter's calls are billed.
    pub fn billing(&self) -> Billing {
        match self {
            Adapter::Request(adapter) => adapter.billing(),
            Adapter::Job(adapter) => adapter.billing(),
        }
    }

    /// The contract the adapter serves ([`RequestAdapter::identity`]).
    pub fn identity(&self) -> Option<Identity> {
        match self {
            Adapter::Request(adapter) => adapter.identity(),
            Adapter::Job(adapter) => adapter.identity(),
        }
    }

    /// The adapter's judgement of its route's other contract members
    /// ([`RequestAdapter::check_contract`]).
    pub fn check_contract(&self, call: &CallRequest) -> Result<(), String> {
        match self {
            Adapter::Request(adapter) => adapter.check_contract(call),
            Adapter::Job(adapter) => adapter.check_contract(call),
        }
    }
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
            limits: Default::default(),
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
                    cost: None,
                }
            })
        }
    }

    #[test]
    fn an_identity_compares_its_behaviour_as_canonical_json() {
        let text = Identity {
            adapter: "fx-acme-image-v1",
            behavior: r#""1""#,
        };
        assert!(text.behaves(&json!("1")));
        for other in [json!(1), json!(1.0), json!("2"), json!(["1"]), json!(true)] {
            assert!(!text.behaves(&other), "{other}");
        }
        let number = Identity {
            adapter: "acme-video",
            behavior: "1",
        };
        assert!(number.behaves(&json!(1)));
        assert!(number.behaves(&json!(1.0)), "the same number");
        for other in [json!("1"), json!(2), json!(1.5)] {
            assert!(!number.behaves(&other), "{other}");
        }
        // An adapter names none unless it says so, and judges no other member.
        let call = CallRequest {
            route: RouteRef {
                capability: "image.generate".into(),
                model: "img-a".into(),
                provider: "acme".into(),
                contract: json!({"adapter": "x"}),
                limits: Default::default(),
            },
            request: json!({}),
            files: IndexMap::new(),
            take: vec![1],
            key: "0".repeat(64),
            attempt: 1,
        };
        for adapter in [
            Adapter::Request(Arc::new(Silent)),
            Adapter::Job(Arc::new(Silent)),
        ] {
            assert_eq!(adapter.identity(), None);
            assert_eq!(adapter.check_contract(&call), Ok(()));
        }
    }

    #[test]
    fn a_project_adapters_identity_is_interned() {
        let first = Identity::interned("adapters/acme.py#acme", "3");
        let again = Identity::interned(&String::from("adapters/acme.py#acme"), "3");
        assert_eq!(first, again);
        assert!(std::ptr::eq(first.adapter, again.adapter));
        assert!(first.behaves(&json!(3)) && !first.behaves(&json!("3")));
    }

    #[test]
    fn not_received_asks_for_no_wait_by_default() {
        assert_eq!(
            Sent::not_received("reset"),
            Sent::NotReceived {
                reason: "reset".into(),
                retry_after: None
            }
        );
        assert_eq!(
            Submitted::not_received("reset"),
            Submitted::NotReceived {
                reason: "reset".into(),
                retry_after: None
            }
        );
    }

    fn limited(headers: &[(&str, &str)]) -> HttpResponse {
        headers
            .iter()
            .fold(HttpResponse::new(429, Vec::new()), |r, (n, v)| {
                r.with_header(n, v)
            })
    }

    #[test]
    fn retry_after_reads_seconds_and_milliseconds() {
        let at = |headers: &[(&str, &str)]| retry_after_at(&limited(headers), UNIX_EPOCH);
        assert_eq!(at(&[]), None);
        assert_eq!(at(&[("Retry-After", "20")]), Some(Duration::from_secs(20)));
        assert_eq!(
            at(&[("retry-after", " 1.5 ")]),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(at(&[("retry-after", "0")]), Some(Duration::ZERO));
        assert_eq!(
            at(&[("retry-after-ms", "250"), ("retry-after", "20")]),
            Some(Duration::from_millis(250)),
            "the finer header wins"
        );
        assert_eq!(
            at(&[("retry-after-ms", "soon"), ("retry-after", "3")]),
            Some(Duration::from_secs(3))
        );
        for bad in ["-1", "1e3", "NaN", "inf", "", "1.", ".5", "20 s", "0x10"] {
            assert_eq!(at(&[("retry-after", bad)]), None, "{bad:?}");
        }
        assert_eq!(
            at(&[("retry-after", "999999999999")]),
            Some(RETRY_AFTER_LIMIT)
        );
    }

    #[test]
    fn retry_after_reads_an_http_date_from_now() {
        // Sun, 06 Nov 1994 08:49:37 GMT is 784111777 s after the epoch.
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        assert_eq!(http_date(date), Some(784_111_777));
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            http_date("Tue, 29 Feb 2028 23:59:59 GMT"),
            Some(1_835_481_599)
        );
        let now = UNIX_EPOCH + Duration::from_secs(784_111_757);
        assert_eq!(
            retry_after_at(&limited(&[("retry-after", date)]), now),
            Some(Duration::from_secs(20))
        );
        let later = UNIX_EPOCH + Duration::from_secs(784_111_800);
        assert_eq!(
            retry_after_at(&limited(&[("retry-after", date)]), later),
            Some(Duration::ZERO),
            "a date in the past is no wait"
        );
        for bad in [
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Nov 1994 8:49:37 GMT",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMT extra",
        ] {
            assert_eq!(http_date(bad), None, "{bad:?}");
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
                limits: Default::default(),
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
        // The default redactor knows no key, only the known secret shapes.
        for redactor in [
            RequestAdapter::redactor(&Silent),
            LongJob::redactor(&Silent),
        ] {
            assert_eq!(
                redactor.reason("used sk-proj-AbCdEf123456"),
                "used [redacted]"
            );
            assert_eq!(redactor.reason("used acme-key-1"), "used acme-key-1");
        }
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
