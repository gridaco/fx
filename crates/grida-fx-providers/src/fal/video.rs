//! fal video: a queue long job (spec/providers.md §9.3, §7).
//!
//! `submit`: one `POST {queue}/<model>` with `{"prompt", "image_url", "end_image_url"?,
//! "aspect_ratio", "resolution", "duration"}` (`duration` a JSON integer); a 2xx with a
//! `request_id` that is a safe id (`^[A-Za-z0-9_.:-]{1,96}$`), and `status_url` and
//! `response_url` both under the queue base, becomes the handle `{"request_id", "status_path",
//! "response_path"}` (the URLs minus `{queue}/`, a plain query kept; never a host name). Contract
//! `adapter` must be `fal-queue` when present.
//!
//! `collect`: polls `GET {queue}/<status_path>` every [`POLL`] until `COMPLETED` (or a truthy
//! `error`), then `GET {queue}/<response_path>`, then downloads `video.url` **once** (https, no
//! credential, the URL exactly as fal gave it, at most [`MAX_VIDEO_BYTES`]); all within
//! [`COLLECT_DEADLINE`] from the start of `collect`, each status or result read bounded by
//! min(remaining, 60 s), sleeping through the injected [`Clock`]. The answer is file `video`
//! (`video/mp4`) and `data: {"facts": {"width", "height", "duration_seconds", "fps"}}` from the
//! clip's file facts; `cost` from the result's top-level `usage.cost`. `check`: the clip's size and
//! duration against the request (spec/providers.md §9.3).
//!
//! Submit refusals, in this order (spec/providers.md §5, §9.3): another route's contract; a request
//! that does not fit `video.generate`; no `FAL_KEY`; a blank prompt, or one over
//! [`MAX_PROMPT_CHARS`]; no `first_frame`; a `duration` that is not a whole number from 3 to 10; a
//! `resolution` (default `720p`) or `aspect_ratio` (default `9:16`) the route does not draw; then
//! `first_frame` and `last_frame`, each when it is not an `image/*` file (`<member> is <kind>, not a
//! picture`) or has no bytes.
//!
//! Submit statuses: 4xx other than 408 and 429 is `Failed { Some(0), false }`; 408, 429, 500, 502
//! and 503 are `NotReceived`; any other status, a 2xx without a usable handle, or a handle outside
//! the queue is `Uncertain`. A transport refusal is `Refused`, `not_sent` is `NotReceived`, and
//! `after_send` is `Uncertain`. An `Uncertain` after a 2xx names the job fal returned (`(request
//! <id>)`, the body's `request_id`, else the response's request-id header, when it is a safe id),
//! so a person can find the job.
//!
//! Collect outcomes (spec/providers.md §7): a handle this adapter did not write is `Unreachable`
//! with nothing requested. Failed status and result reads (a transport failure, 408, 429, a 5xx, a
//! body that is not a JSON object) are polls that saw nothing. A 401, 403 or 3xx is `Unreachable`;
//! another 4xx is `Ended`; a status with a truthy `error` is `Ended`; the deadline is
//! `Unreachable`. A result without an https video is `Ended`. The download is made once: a
//! transport failure, a 3xx, 401, 403, 408, 429, a 5xx or another status outside 2xx and 4xx is
//! `Unreachable` (a later collect reads the result and downloads again); another 4xx, a body over
//! the cap, empty or without the MP4 signature, or a clip with no video stream is `Ended`. An
//! `Ended` after the result read comes from a job fal finished, so it costs that read's
//! `usage.cost`, as an answer would; every other `Ended` reports no cost (`None`), so the run that
//! submitted the job charges its whole hold. The bytes decide the kind: a `content-type` header
//! and the result's `content_type` are not read.

use super::FalClients;
use crate::BoxFuture;
use crate::adapter::{Answer, CallRequest, Collected, LongJob, Submitted};
use crate::clock::Clock;
use crate::setup::Client;
use crate::transport::{
    Body, Credential, HttpRequest, HttpResponse, Lane, Method, Phase, TransportError,
    TransportErrorKind,
};
use grida_fx_core::money::Usd;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

pub use crate::checks::ClipFacts;

/// The contract `adapter` this adapter serves.
pub const CONTRACT_ADAPTER: &str = "fal-queue";

/// The submit's deadline.
pub const SUBMIT_DEADLINE: Duration = Duration::from_secs(300);

/// How long one `collect` waits for the job, from its start.
pub const COLLECT_DEADLINE: Duration = Duration::from_secs(1500);

/// The polling interval.
pub const POLL: Duration = Duration::from_secs(5);

/// The longest single status or result read.
pub const READ_DEADLINE: Duration = Duration::from_secs(60);

/// The most bytes of one clip.
pub const MAX_VIDEO_BYTES: u64 = 512 * 1024 * 1024;

/// The longest prompt, in Unicode scalar values.
pub const MAX_PROMPT_CHARS: usize = 20_000;

/// The label of the submit's reasons.
pub const SUBMIT_LABEL: &str = "fal video submission";

/// The label of a status read's reasons.
pub const STATUS_LABEL: &str = "fal video job status";

/// The label of a result read's reasons.
pub const RESULT_LABEL: &str = "fal video job result";

/// The label of the clip download's reasons.
pub const DOWNLOAD_LABEL: &str = "fal output video download";

/// The sentence of a job still running at the deadline.
pub const OUTSTANDING: &str =
    "fal has not finished the video job yet; it is collected on the next run";

/// The response cap of the submit and of each status or result read (small JSON objects).
const JSON_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// The longest path a handle holds.
const MAX_PATH_CHARS: usize = 1024;

/// The whole seconds the route draws.
const SECONDS: std::ops::RangeInclusive<u64> = 3..=10;

/// The tiers the route draws, with their short sides.
const RESOLUTIONS: [(&str, u32); 4] = [("360p", 360), ("720p", 720), ("1080p", 1080), ("4k", 2160)];

/// The aspect ratios the route draws.
const ASPECTS: [&str; 2] = ["9:16", "16:9"];

/// fal's video adapter (module doc).
#[derive(Clone)]
pub struct FalVideo {
    clients: FalClients,
    clock: Arc<dyn Clock>,
}

impl FalVideo {
    pub fn new(clients: FalClients, clock: Arc<dyn Clock>) -> FalVideo {
        FalVideo { clients, clock }
    }
}

/// What a request asks of the route, once its values pass (step 4 of spec/providers.md §5).
#[derive(Debug, Clone, PartialEq)]
struct Clip<'a> {
    prompt: &'a str,
    first_frame: &'a Value,
    last_frame: Option<&'a Value>,
    seconds: u64,
    resolution: &'a str,
    aspect_ratio: &'a str,
}

/// The value refusals of the module doc, in order. The request already fits `video.generate`.
fn clip(request: &Value) -> Result<Clip<'_>, String> {
    let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("a clip needs its prompt as text".into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "the prompt is longer than {MAX_PROMPT_CHARS} characters"
        ));
    }
    let first_frame = match request.get("first_frame") {
        Some(frame) if crate::capabilities::is_file_value(frame) => frame,
        _ => return Err("this route draws from a first frame".into()),
    };
    let duration = request.get("duration").unwrap_or(&Value::Null);
    let seconds = whole_seconds(duration).ok_or_else(|| {
        format!(
            "this route draws whole seconds from 3 to 10, not {}",
            value_text(duration)
        )
    })?;
    let text = |name: &str, default: &'static str| match request.get(name) {
        None | Some(Value::Null) => Some(default),
        Some(value) => value.as_str(),
    };
    let (resolution, aspect_ratio) = (text("resolution", "720p"), text("aspect_ratio", "9:16"));
    match (resolution, aspect_ratio) {
        (Some(resolution), Some(aspect_ratio))
            if RESOLUTIONS.iter().any(|(name, _)| *name == resolution)
                && ASPECTS.contains(&aspect_ratio) =>
        {
            Ok(Clip {
                prompt,
                first_frame,
                last_frame: request.get("last_frame").filter(|frame| !frame.is_null()),
                seconds,
                resolution,
                aspect_ratio,
            })
        }
        _ => Err(format!(
            "this route draws no {} clip at {}",
            resolution.map_or_else(|| value_text(&request["resolution"]), str::to_string),
            aspect_ratio.map_or_else(|| value_text(&request["aspect_ratio"]), str::to_string),
        )),
    }
}

/// A whole number of seconds the route draws (`3`, `3.0`), else `None`.
fn whole_seconds(duration: &Value) -> Option<u64> {
    let Value::Number(n) = duration else {
        return None;
    };
    let x = grida_fx_core::value::as_f64(n);
    let whole = x.is_finite() && x.fract() == 0.0 && x >= 0.0;
    whole
        .then_some(x as u64)
        .filter(|seconds| SECONDS.contains(seconds))
}

/// A request value as a refusal shows it: a number in its JCS form, text as is, else its JSON.
fn value_text(value: &Value) -> String {
    match value {
        Value::Number(n) => grida_fx_core::value::format_number(grida_fx_core::value::as_f64(n)),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The size a clip of `resolution` at `aspect_ratio` has: the short side 360, 720, 1080 or 2160,
/// the long side `short × 16 ÷ 9` (integer division), portrait for `9:16` and landscape for
/// `16:9`. `None` for a tier or ratio the route does not draw.
pub fn expected_size(resolution: &str, aspect_ratio: &str) -> Option<(u32, u32)> {
    let short = RESOLUTIONS
        .iter()
        .find(|(name, _)| *name == resolution)
        .map(|(_, short)| *short)?;
    let long = short * 16 / 9;
    match aspect_ratio {
        "9:16" => Some((short, long)),
        "16:9" => Some((long, short)),
        _ => None,
    }
}

/// The check of a collected clip (spec/providers.md §9.3): its size is
/// [`expected_size`] (`the clip is <w>x<h>, not <W>x<H>`), and its duration is within
/// `1/fps + 0.01` s of `seconds` (`the clip runs <x> s, not <n> s`).
pub fn check_clip(
    facts: &ClipFacts,
    resolution: &str,
    aspect_ratio: &str,
    seconds: f64,
) -> Result<(), String> {
    let Some((width, height)) = expected_size(resolution, aspect_ratio) else {
        return Err(format!(
            "this route draws no {resolution} clip at {aspect_ratio}"
        ));
    };
    if (facts.width, facts.height) != (width, height) {
        return Err(format!(
            "the clip is {}x{}, not {width}x{height}",
            facts.width, facts.height
        ));
    }
    if (facts.duration_seconds - seconds).abs() > 1.0 / facts.fps + 0.01 {
        return Err(format!(
            "the clip runs {:.3} s, not {} s",
            facts.duration_seconds,
            grida_fx_core::value::format_number(seconds)
        ));
    }
    Ok(())
}

/// The collecting half of a handle: what [`handle_of`] wrote.
#[derive(Debug, Clone, PartialEq)]
struct Job {
    request_id: String,
    status_path: String,
    response_path: String,
}

impl Job {
    /// The handle when it is exactly one this adapter writes, else `None`: a safe `request_id`, and
    /// two queue references ([`is_queue_ref`]).
    fn from_handle(handle: &Value) -> Option<Job> {
        let members = handle.as_object()?;
        if members.len() != 3 {
            return None;
        }
        let text = |name: &str| {
            members
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_string)
        };
        let job = Job {
            request_id: text("request_id")?,
            status_path: text("status_path")?,
            response_path: text("response_path")?,
        };
        (super::is_safe_id(&job.request_id)
            && is_queue_ref(&job.status_path)
            && is_queue_ref(&job.response_path))
        .then_some(job)
    }
}

/// What a handle may hold of a queue URL: a [`is_queue_path`], then optionally `?` and a
/// [`is_plain_query`], at most [`MAX_PATH_CHARS`] in all. It is kept byte for byte.
fn is_queue_ref(reference: &str) -> bool {
    if reference.len() > MAX_PATH_CHARS {
        return false;
    }
    match reference.split_once('?') {
        Some((path, query)) => is_queue_path(path) && is_plain_query(query),
        None => is_queue_path(reference),
    }
}

/// A query a handle may keep (`logs=1`): empty, or `&`-joined pairs `name` or `name=value`, where
/// a name is 1 to 64 of `A-Z a-z 0-9 . _ ~ -` and a value is up to 256 of those and `+ , : @`.
/// No percent-encoding, `/`, `?`, `#` or `=` in a value, and no name that reads as a credential or
/// a signature (one holding `auth`, `credential`, `expires`, `key`, `password`, `policy`,
/// `secret`, `session`, `sig` or `token`, or starting `x-amz-` or `x-goog-`), since a handle never
/// holds a signed URL (spec/providers.md §7).
fn is_plain_query(query: &str) -> bool {
    const SECRET_WORDS: [&str; 10] = [
        "auth",
        "credential",
        "expires",
        "key",
        "password",
        "policy",
        "secret",
        "session",
        "sig",
        "token",
    ];
    let name_char = |c: char| c.is_ascii_alphanumeric() || "._~-".contains(c);
    let value_char = |c: char| name_char(c) || "+,:@".contains(c);
    query.is_empty()
        || query.split('&').all(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let lower = name.to_ascii_lowercase();
            (1..=64).contains(&name.len())
                && name.chars().all(name_char)
                && value.len() <= 256
                && value.chars().all(value_char)
                && !SECRET_WORDS.iter().any(|word| lower.contains(word))
                && !lower.starts_with("x-amz-")
                && !lower.starts_with("x-goog-")
        })
}

/// A path under the queue base a handle may hold: relative, non-empty segments that are not `.`
/// or `..`, of the characters `A-Z a-z 0-9 . _ ~ + = , @ -`. So no scheme, no host, no query, no
/// fragment, no backslash and no percent-encoding.
fn is_queue_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_CHARS
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._~+=,@-".contains(c))
        })
}

/// The handle of a 2xx submit answer (module doc), or the `Uncertain` sentence. A `request_id`
/// that is not a safe id is no handle: it would go into the job record (spec/providers.md §7).
fn handle_of(queue_base: &str, body: &[u8]) -> Result<Value, String> {
    let no_handle = || "fal took the video job but returned no handle to collect it by".to_string();
    let payload = crate::wire::json_object(body, SUBMIT_LABEL).map_err(|_| no_handle())?;
    let text = |name: &str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    };
    let (Some(request_id), Some(status_url), Some(response_url)) =
        (text("request_id"), text("status_url"), text("response_url"))
    else {
        return Err(no_handle());
    };
    if !super::is_safe_id(request_id) {
        return Err(no_handle());
    }
    let prefix = format!("{queue_base}/");
    let path = |url: &str| {
        url.strip_prefix(&prefix)
            .filter(|reference| is_queue_ref(reference))
            .map(str::to_string)
            .ok_or_else(|| "a fal video job handle points outside fal's queue".to_string())
    };
    Ok(json!({
        "request_id": request_id,
        "status_path": path(status_url)?,
        "response_path": path(response_url)?,
    }))
}

/// What one status or result read saw.
enum Read {
    /// A JSON object.
    Json(Map<String, Value>),
    /// Nothing: poll again.
    Nothing,
    /// Collecting stops here.
    Stop(Collected),
}

impl FalVideo {
    /// Steps 1 to 5 of spec/providers.md §5, then the submit request (module doc).
    fn prepare(&self, call: &CallRequest) -> Result<HttpRequest, String> {
        let client = &self.clients.queue;
        if let Some(refusal) = super::contract_refusal(call, CONTRACT_ADAPTER, &["video.generate"])
        {
            return Err(refusal);
        }
        crate::capabilities::check_request(&call.route.capability, &call.request)?;
        let credential = client.credential(super::CREDENTIAL_HEADER, super::CREDENTIAL_PREFIX)?;
        let clip = clip(&call.request)?;
        let first = super::picture(call, clip.first_frame, "first_frame")?;
        let last = match clip.last_frame {
            Some(frame) => Some(super::picture(call, frame, "last_frame")?),
            None => None,
        };

        let mut body = Map::new();
        body.insert("prompt".into(), Value::String(clip.prompt.to_string()));
        body.insert("image_url".into(), Value::String(first));
        if let Some(last) = last {
            body.insert("end_image_url".into(), Value::String(last));
        }
        body.insert("aspect_ratio".into(), json!(clip.aspect_ratio));
        body.insert("resolution".into(), json!(clip.resolution));
        body.insert("duration".into(), json!(clip.seconds));

        let url = client.url(super::endpoint(&call.route.model));
        Ok(HttpRequest::new(Method::Post, url, Lane::Provider)
            .credential(credential)
            .body(Body::Json(Value::Object(body)))
            .timeout(SUBMIT_DEADLINE)
            .max_response_bytes(JSON_RESPONSE_BYTES))
    }

    async fn submit_once(&self, call: &CallRequest) -> Submitted {
        let client = &self.clients.queue;
        let request = match self.prepare(call) {
            Ok(request) => request,
            Err(reason) => {
                return Submitted::Refused {
                    reason: client.reason(&reason),
                };
            }
        };
        match client.send(request).await {
            Ok(response) => self.submitted(&response),
            Err(error) => submit_transport(client, &error),
        }
    }

    /// The submit table of the module doc for an answered POST.
    fn submitted(&self, response: &HttpResponse) -> Submitted {
        let client = &self.clients.queue;
        let status = response.status;
        let uncertain = |text: &str| Submitted::Uncertain {
            reason: client.reason(text),
        };
        let text = super::status_text(SUBMIT_LABEL, response);
        match status {
            200..=299 => match handle_of(&client.base, &response.body) {
                Ok(handle) => Submitted::Accepted { handle },
                Err(sentence) => match returned_id(response) {
                    Some(id) => uncertain(&format!("{sentence} (request {id})")),
                    None => uncertain(&sentence),
                },
            },
            408 => super::submit_not_received(client, &text, super::retry_after(response)),
            429 => super::submit_not_received(
                client,
                &format!("{SUBMIT_LABEL} was rate limited (HTTP 429)"),
                super::retry_after(response),
            ),
            400..=499 => Submitted::Failed {
                reason: client.reason(&text),
                cost: Some(Usd::ZERO),
                retryable: false,
            },
            500 | 502 | 503 => {
                super::submit_not_received(client, &text, super::retry_after(response))
            }
            300..=399 => uncertain(&format!("{SUBMIT_LABEL} was redirected (HTTP {status})")),
            _ => uncertain(&text),
        }
    }

    async fn collect_once(&self, call: &CallRequest, handle: &Value) -> Collected {
        let client = &self.clients.queue;
        let unreachable = |text: &str| Collected::Unreachable {
            reason: client.reason(text),
        };
        if let Some(refusal) = super::contract_refusal(call, CONTRACT_ADAPTER, &["video.generate"])
        {
            return unreachable(&refusal);
        }
        let Some(job) = Job::from_handle(handle) else {
            return unreachable("a fal video job handle is not one this adapter wrote");
        };
        let credential = match client.credential(super::CREDENTIAL_HEADER, super::CREDENTIAL_PREFIX)
        {
            Ok(credential) => credential,
            Err(reason) => return unreachable(&reason),
        };
        let deadline = self.clock.now() + COLLECT_DEADLINE;

        loop {
            match self
                .read(&credential, &job.status_path, deadline, STATUS_LABEL)
                .await
            {
                Read::Json(status) => {
                    if let Some(Value::String(id)) = status.get("request_id")
                        && *id != job.request_id
                    {
                        return unreachable(&format!("{STATUS_LABEL} answered about another job"));
                    }
                    if status.get("error").is_some_and(super::truthy) {
                        return Collected::Ended {
                            reason: client.reason(&job_error(&status, &client.redactor)),
                            cost: None,
                        };
                    }
                    if status.get("status").and_then(Value::as_str) == Some("COMPLETED") {
                        break;
                    }
                }
                Read::Nothing => {}
                Read::Stop(collected) => return collected,
            }
            if let Err(collected) = self.wait(deadline).await {
                return collected;
            }
        }

        let result = loop {
            match self
                .read(&credential, &job.response_path, deadline, RESULT_LABEL)
                .await
            {
                Read::Json(result) => break result,
                Read::Nothing => {}
                Read::Stop(collected) => return collected,
            }
            if let Err(collected) = self.wait(deadline).await {
                return collected;
            }
        };
        // fal finished the job, so what the result reports is its cost, whether the result
        // answers or is unusable.
        let cost = super::reported_cost(&result);
        let url = match result_video(super::answer_root(&result)) {
            Ok(url) => url,
            Err(sentence) => {
                return Collected::Ended {
                    reason: client.reason(&sentence),
                    cost,
                };
            }
        };
        self.download(url, deadline, cost).await
    }

    /// One authorized GET of a status or result (module doc: failed reads are polls that saw
    /// nothing).
    async fn read(
        &self,
        credential: &Credential,
        path: &str,
        deadline: Duration,
        label: &str,
    ) -> Read {
        let client = &self.clients.queue;
        let remaining = deadline.saturating_sub(self.clock.now());
        if remaining.is_zero() {
            return Read::Stop(outstanding(client));
        }
        let request = HttpRequest::new(Method::Get, client.url(path), Lane::Provider)
            .credential(credential.clone())
            .timeout(remaining.min(READ_DEADLINE))
            .max_response_bytes(JSON_RESPONSE_BYTES);
        let response = match client.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::Refused => {
                return Read::Stop(Collected::Unreachable {
                    reason: client.reason(&format!("{label} was not sent: {}", error.reason)),
                });
            }
            Err(_) => return Read::Nothing,
        };
        let text = || client.reason(&super::status_text(label, &response));
        match response.status {
            200..=299 => match crate::wire::json_object(&response.body, label) {
                Ok(payload) => Read::Json(payload),
                Err(_) => Read::Nothing,
            },
            408 | 429 | 500..=599 => Read::Nothing,
            300..=399 | 401 | 403 => Read::Stop(Collected::Unreachable { reason: text() }),
            400..=499 => Read::Stop(Collected::Ended {
                reason: text(),
                cost: None,
            }),
            _ => Read::Nothing,
        }
    }

    /// Sleeps one poll interval, unless that would pass the deadline.
    #[allow(clippy::result_large_err)] // `Collected` is what every caller returns at once
    async fn wait(&self, deadline: Duration) -> Result<(), Collected> {
        if self.clock.now() + POLL > deadline {
            return Err(outstanding(&self.clients.queue));
        }
        self.clock.sleep(POLL).await;
        Ok(())
    }

    /// The clip download, made once, and its answer (module doc). A failure a later collect may
    /// not repeat is `Unreachable`: the record stays `submitted`, and the next collect reads the
    /// result again and downloads once more. The bytes decide what the file is.
    async fn download(&self, url: &str, deadline: Duration, cost: Option<Usd>) -> Collected {
        let client = &self.clients.queue;
        let mut redactor = client.redactor.with(url);
        if let Ok(parsed) = url::Url::parse(url) {
            redactor = redactor.with(parsed.as_str());
        }
        let ended = |text: &str| Collected::Ended {
            reason: redactor.reason(text),
            cost,
        };
        let unreachable = |text: &str| Collected::Unreachable {
            reason: redactor.reason(text),
        };
        let remaining = deadline.saturating_sub(self.clock.now());
        if remaining.is_zero() {
            return outstanding(client);
        }
        // The provider's text, byte for byte (spec/providers.md §8).
        let request = HttpRequest::new(Method::Get, url, Lane::Download)
            .timeout(remaining)
            .max_response_bytes(MAX_VIDEO_BYTES);
        let response = match client.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::TooLarge => {
                return ended(&format!("{DOWNLOAD_LABEL} failed: {}", error.reason));
            }
            Err(error) if error.kind == TransportErrorKind::Refused => {
                return unreachable(&format!("{DOWNLOAD_LABEL} was not sent: {}", error.reason));
            }
            Err(error) => {
                return unreachable(&format!("{DOWNLOAD_LABEL} failed: {}", error.reason));
            }
        };
        match response.status {
            200..=299 => {}
            400..=499 if !matches!(response.status, 401 | 403 | 408 | 429) => {
                return ended(&super::status_text(DOWNLOAD_LABEL, &response));
            }
            _ => return unreachable(&super::status_text(DOWNLOAD_LABEL, &response)),
        }
        let bytes = response.body;
        if bytes.is_empty() {
            return ended("fal output video download was empty");
        }
        if bytes.len() as u64 > MAX_VIDEO_BYTES {
            return ended(&format!(
                "{DOWNLOAD_LABEL} failed: the response is larger than {MAX_VIDEO_BYTES} bytes"
            ));
        }
        if !crate::wire::matches_signature("video/mp4", &bytes) {
            return ended("fal output video is not an MP4 file");
        }
        match ClipFacts::of_mp4(&bytes) {
            Ok(facts) => Collected::Answered(
                Answer::new(json!({"facts": facts.to_json()}), cost).with_file(
                    "video",
                    "video/mp4",
                    bytes,
                ),
            ),
            Err(sentence) => ended(&sentence),
        }
    }
}

/// The result's video URL, as fal gave it, when it is https with a host and no userinfo.
/// Otherwise the `Ended` sentence. A declared `content_type` is not read: the bytes decide.
fn result_video(root: &Map<String, Value>) -> Result<&str, String> {
    let Some(Value::Object(video)) = root.get("video") else {
        return Err(format!("{RESULT_LABEL} carries no video"));
    };
    let url = match video.get("url") {
        Some(Value::String(url)) if !url.trim().is_empty() => url.as_str(),
        _ => return Err("fal output video url must be non-empty".into()),
    };
    let https = url::Url::parse(url).ok().is_some_and(|parsed| {
        parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
    });
    if !https {
        return Err("fal output video url must be https".into());
    }
    Ok(url)
}

/// The job id a 2xx submit answer names, for an `Uncertain` reason: the body's `request_id` when
/// it is a safe id, else the response's request-id header ([`crate::wire::request_id`]).
fn returned_id(response: &HttpResponse) -> Option<String> {
    crate::wire::json_object(&response.body, SUBMIT_LABEL)
        .ok()
        .and_then(|payload| match payload.get("request_id") {
            Some(Value::String(id)) if super::is_safe_id(id) => Some(id.clone()),
            _ => None,
        })
        .or_else(|| crate::wire::request_id(response))
}

/// `fal video job failed (<error_type>): <error>`. Each provider text is redacted **first**, then
/// collapsed and cut (the error to 500 characters, `error_type` to 100), so a key that straddles
/// a cut is still removed whole (spec/providers.md §8).
fn job_error(status: &Map<String, Value>, redactor: &crate::redact::Redactor) -> String {
    let cut = |text: &str, max: usize| crate::redact::bounded(&redactor.redact(text), max);
    let error = match status.get("error") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let error = cut(&error, 500);
    match status.get("error_type").and_then(Value::as_str) {
        Some(kind) => format!("fal video job failed ({}): {error}", cut(kind, 100)),
        None => format!("fal video job failed: {error}"),
    }
}

fn outstanding(client: &Client) -> Collected {
    Collected::Unreachable {
        reason: client.reason(OUTSTANDING),
    }
}

/// A failed submit exchange (spec/providers.md §4.2).
fn submit_transport(client: &Client, error: &TransportError) -> Submitted {
    match (error.kind, error.phase) {
        (TransportErrorKind::Refused, _) => Submitted::Refused {
            reason: client.reason(&format!("{SUBMIT_LABEL} was not sent: {}", error.reason)),
        },
        (_, Phase::NotSent) => super::submit_not_received(
            client,
            &format!("{SUBMIT_LABEL} was not sent: {}", error.reason),
            None,
        ),
        (_, Phase::AfterSend) => Submitted::Uncertain {
            reason: client.reason(&format!(
                "{SUBMIT_LABEL} ended without an answer; it may have been taken: {}",
                error.reason
            )),
        },
    }
}

impl LongJob for FalVideo {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(self.submit_once(call))
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(self.collect_once(call, handle))
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let clip = clip(&call.request)?;
        let Some(file) = answer.files.get("video") else {
            return Err("the answer carries no video".into());
        };
        let facts = ClipFacts::of_mp4(&file.bytes)?;
        check_clip(
            &facts,
            clip.resolution,
            clip.aspect_ratio,
            clip.seconds as f64,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_refused_in_order() {
        let frame = json!({"file": "a".repeat(64)});
        let ok = json!({"prompt": "p", "first_frame": frame, "duration": 3.0,
                        "resolution": null, "aspect_ratio": null});
        let clip_of = |request: &Value| {
            clip(request).map(|c| {
                (
                    c.seconds,
                    c.resolution.to_string(),
                    c.aspect_ratio.to_string(),
                )
            })
        };
        assert_eq!(clip_of(&ok), Ok((3, "720p".into(), "9:16".into())));
        let with = |member: &str, value: Value| {
            let mut request = ok.clone();
            request[member] = value;
            clip(&request).unwrap_err()
        };
        assert_eq!(
            with("prompt", json!("  ")),
            "a clip needs its prompt as text"
        );
        assert_eq!(
            with("prompt", json!("é".repeat(20_001))),
            "the prompt is longer than 20000 characters"
        );
        assert!(
            clip(&json!({"prompt": "é".repeat(20_000), "first_frame": frame, "duration": 3}))
                .is_ok()
        );
        assert_eq!(
            with("first_frame", Value::Null),
            "this route draws from a first frame"
        );
        for (duration, shown) in [
            (json!(12), "12"),
            (json!(3.5), "3.5"),
            (json!(2), "2"),
            (json!(-3), "-3"),
            (Value::Null, "null"),
        ] {
            assert_eq!(
                with("duration", duration),
                format!("this route draws whole seconds from 3 to 10, not {shown}")
            );
        }
        assert_eq!(
            with("resolution", json!("8k")),
            "this route draws no 8k clip at 9:16"
        );
        assert_eq!(
            with("aspect_ratio", json!("4:3")),
            "this route draws no 720p clip at 4:3"
        );
        let mut no_duration = ok.clone();
        no_duration.as_object_mut().unwrap().remove("duration");
        assert_eq!(
            clip(&no_duration).unwrap_err(),
            "this route draws whole seconds from 3 to 10, not null"
        );
    }

    #[test]
    fn queue_paths_are_relative_and_plain() {
        for good in [
            "google/gemini-omni-flash/requests/req-1/status",
            "google/gemini-omni-flash/requests/0b9e7f1c-2a4d-4c55-9d1e-6f7a8b9c0d1e",
            "a",
        ] {
            assert!(is_queue_path(good), "{good}");
        }
        for bad in [
            "",
            "/abs/path",
            "https://queue.fal.run/x",
            "https:x",
            "a/../b",
            "..",
            "a/./b",
            "a//b",
            "a/",
            "a?x=1",
            "a#f",
            "a\\b",
            "a%2e%2e/b",
            "a b",
        ] {
            assert!(!is_queue_path(bad), "{bad}");
        }
        assert!(!is_queue_path(&"a".repeat(MAX_PATH_CHARS + 1)));
    }

    #[test]
    fn queue_refs_keep_a_plain_query_only() {
        for good in [
            "google/gemini-omni-flash/requests/req-1/status",
            "google/gemini-omni-flash/requests/req-1/status?logs=1",
            "a?logs=1&verbose",
            "a?x=a+b,c:d@e",
            "a?",
        ] {
            assert!(is_queue_ref(good), "{good}");
        }
        for bad in [
            "a?x=%2F",
            "a?x=1#f",
            "a?x=a/b",
            "a?x=1?y=2",
            "a?x=1=2",
            "a?x=1&&y=2",
            "a?=1",
            "a?token=abc",
            "a?X-Amz-Signature=deadbeef",
            "a?Expires=1",
            "a?api_key=1",
            "a?x-goog-date=1",
            "a?Signature=1",
            "a?x='y'",
            "a/../b?x=1",
            "/a?x=1",
        ] {
            assert!(!is_queue_ref(bad), "{bad}");
        }
        assert!(!is_queue_ref(&format!(
            "a?x={}",
            "1".repeat(MAX_PATH_CHARS)
        )));
    }

    #[test]
    fn handles_are_paths_under_the_queue() {
        let base = "https://queue.fal.run";
        let body = json!({
            "request_id": "req-1",
            "status_url": "https://queue.fal.run/google/gemini-omni-flash/requests/req-1/status",
            "response_url": "https://queue.fal.run/google/gemini-omni-flash/requests/req-1",
            "cancel_url": "https://queue.fal.run/google/gemini-omni-flash/requests/req-1/cancel",
        });
        let handle = handle_of(base, body.to_string().as_bytes()).unwrap();
        assert_eq!(
            handle,
            json!({"request_id": "req-1",
                   "status_path": "google/gemini-omni-flash/requests/req-1/status",
                   "response_path": "google/gemini-omni-flash/requests/req-1"})
        );
        assert_eq!(
            Job::from_handle(&handle),
            Some(Job {
                request_id: "req-1".into(),
                status_path: "google/gemini-omni-flash/requests/req-1/status".into(),
                response_path: "google/gemini-omni-flash/requests/req-1".into(),
            })
        );
        let mut foreign = body.clone();
        foreign["status_url"] = json!("https://queue.fal.run.evil.example/x/status");
        assert_eq!(
            handle_of(base, foreign.to_string().as_bytes()).unwrap_err(),
            "a fal video job handle points outside fal's queue"
        );
        let mut missing = body.clone();
        missing.as_object_mut().unwrap().remove("request_id");
        assert_eq!(
            handle_of(base, missing.to_string().as_bytes()).unwrap_err(),
            "fal took the video job but returned no handle to collect it by"
        );
        assert!(handle_of(base, b"<html>ok</html>").is_err());
        assert_eq!(Job::from_handle(&json!("x")), None);
        let mut extra = handle.clone();
        extra["status_url"] = json!("https://queue.fal.run/x");
        assert_eq!(Job::from_handle(&extra), None);
    }

    #[test]
    fn expected_sizes_for_every_tier_and_ratio() {
        let table = [
            ("360p", "9:16", (360, 640)),
            ("360p", "16:9", (640, 360)),
            ("720p", "9:16", (720, 1280)),
            ("720p", "16:9", (1280, 720)),
            ("1080p", "9:16", (1080, 1920)),
            ("1080p", "16:9", (1920, 1080)),
            ("4k", "9:16", (2160, 3840)),
            ("4k", "16:9", (3840, 2160)),
        ];
        for (resolution, aspect, size) in table {
            assert_eq!(
                expected_size(resolution, aspect),
                Some(size),
                "{resolution} {aspect}"
            );
        }
        assert_eq!(expected_size("8k", "9:16"), None);
        assert_eq!(expected_size("720p", "1:1"), None);
    }

    #[test]
    fn job_errors_are_cut() {
        let redactor = crate::redact::Redactor::new(Vec::new());
        let status = json!({"status": "COMPLETED", "error": "boom ".repeat(200), "error_type": "model_error"});
        let reason = job_error(status.as_object().unwrap(), &redactor);
        assert!(reason.starts_with("fal video job failed (model_error): boom boom"));
        assert!(reason.ends_with('…'), "{reason}");
        assert_eq!(
            reason.chars().count(),
            "fal video job failed (model_error): ".chars().count() + 500
        );
        let untyped = json!({"error": {"detail": "x"}, "error_type": 7});
        assert_eq!(
            job_error(untyped.as_object().unwrap(), &redactor),
            r#"fal video job failed: {"detail":"x"}"#
        );
    }

    #[test]
    fn a_key_straddling_a_cut_is_redacted_whole() {
        let key = "test-fal-key-0123456789abcdef";
        let redactor = crate::redact::Redactor::new(vec![crate::keys::Secret::new(key)]);
        let error = format!("x{}yyyyyyyyy{key}", " ".repeat(470));
        let error_type = format!("{}ab{key}", "ab. ".repeat(22));
        let status = json!({"error": error, "error_type": error_type});
        let reason = job_error(status.as_object().unwrap(), &redactor);
        assert!(!reason.contains(&key[..8]), "{reason}");
        assert!(reason.contains("yyyyyyyyy[redacted]"), "{reason}");
    }
}
