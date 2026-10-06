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
use crate::transport::HttpResponse;
use grida_fx_core::money::Usd;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// `retry_after` is the wait the provider asked for before that ([`retry_after`]; `None` when
    /// it asked for none): the engine waits at least its own backoff, and at least `retry_after`
    /// up to its cap (spec/providers.md §4.3).
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
