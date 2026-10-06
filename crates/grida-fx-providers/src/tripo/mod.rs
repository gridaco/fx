//! Tripo: `mesh.generate` (multiview to model) and `mesh.rig`, both long jobs
//! (spec/providers.md §9.4; spec/capabilities.md §7–§8).
//!
//! Credential `authorization: Bearer <TRIPO_API_KEY>` on every request to the API base
//! (`https://openapi.tripo3d.ai/v3`, no override variable), never on a model download. Every API
//! answer is the envelope `{"code": 0, "data": {...}}` with HTTP 200 exactly ([`TripoApi::data`]);
//! Tripo's error text is never read into a reason ("body withheld"). A submit runs a **free
//! phase** first (uploads; for a rig, the riggability check) whose failures are `NotReceived` or
//! `Refused`, then posts the one **paid** task exactly once. `collect` polls `GET /tasks/<id>`
//! every [`POLL`] through the injected clock, then downloads every model URL of the finished task
//! (Tripo's own hosts only, no credential, at most [`MAX_MODEL_BYTES`] each) and sniffs each file.
//! The cost is `credits_consumed` of the finished task × USD 0.01, rounded up
//! ([`crate::wire::decimal_micros_ceil`] with 10 000 micro-dollars per credit).
//!
//! Outcomes (spec/providers.md §7, §9.4):
//! - free phase: 400, 401, 403, 404, 413, 415 and 422 are `Refused`, and so is a request the
//!   transport refused itself (the network turned off); every other failure is `NotReceived`;
//! - the paid POST: `not_sent` and 429 are `NotReceived`; 400, 401, 403, 404 and 422 are
//!   `Failed { cost: None, retryable: false }`; anything else that is not a usable task id is
//!   `Uncertain`, and when Tripo returned a task id that is a safe field
//!   (`^[A-Za-z0-9_.:-]{1,96}$`) but not one FX collects, the reason names it so a person can find
//!   the task;
//! - collecting: a status read that fails is a poll that saw nothing, except a credential or proxy
//!   problem (401, 403, a 3xx) or a transport refusal, which end polling at once as `Unreachable`;
//!   a terminal status other than `success`, an unusable output and a file of the wrong kind are
//!   `Ended`; still running at the deadline, an answer about another task, an unknown status and a
//!   download that fails or is over the cap are `Unreachable`.
//!
//! A model URL is downloaded at Tripo's text, byte for byte; the riggability check's `rig_type`
//! reaches a handle or an answer only as a short plain string, else `null`; and a [`Task`]'s
//! `Debug` withholds its output's values, which are signed URLs.

pub mod mesh;
pub mod rig;

use crate::adapter::{Adapter, CallRequest, Collected, Submitted};
use crate::clock::Clock;
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::{Client, Setup};
use crate::transport::{
    Body, Credential, HttpRequest, HttpResponse, Lane, Method, Part, Phase, TransportError,
    TransportErrorKind,
};
use grida_fx_core::money::Usd;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// The polling interval.
pub const POLL: Duration = Duration::from_secs(5);

/// The deadline of each upload, status read and download.
pub const READ_DEADLINE: Duration = Duration::from_secs(300);

/// The paid POST's deadline.
pub const POST_DEADLINE: Duration = Duration::from_secs(180);

/// The most bytes of one model.
pub const MAX_MODEL_BYTES: u64 = 150_000_000;

/// Micro-dollars per Tripo credit.
pub const MICROS_PER_CREDIT: i64 = 10_000;

/// The task statuses that are not over yet.
const RUNNING: [&str; 2] = ["queued", "running"];

/// The task statuses that end a task without a result.
const ENDED: [&str; 4] = ["failed", "cancelled", "banned", "expired"];

/// Free-phase statuses that are deterministic refusals (spec/providers.md §9.4 "Free phase").
const FREE_REFUSED: [u16; 7] = [400, 401, 403, 404, 413, 415, 422];

/// The ` (code <n>)` a refusal names (spec/providers.md §9.4 "The envelope"): the integer
/// `code` of a JSON object body, which says why Tripo refused; nothing else of the body is read.
fn error_code(body: &[u8]) -> String {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(envelope)) => match envelope.get("code") {
            Some(Value::Number(code)) if code.is_i64() || code.is_u64() => {
                format!(" (code {code})")
            }
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// Paid-POST statuses that say Tripo refused the task (spec/providers.md §9.4 "Paid POST").
const PAID_REFUSED: [u16; 5] = [400, 401, 403, 404, 422];

/// The most model URLs one finished task may name.
const MAX_MODEL_URLS: usize = 8;

/// The longest model URL, in characters.
const MAX_URL_CHARS: usize = 20_480;

/// The storage host outside `tripo3d.ai` that Tripo serves models from.
const STORAGE_HOST: &str = "tripo-data.rg1.data.tripo3d.com";

static FILE_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,256}$").expect("a valid pattern"));

static TASK_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").expect("a valid pattern"));

static OUTPUT_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_]+$").expect("a valid pattern"));

static SAFE_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:-]{1,96}$").expect("a valid pattern"));

/// What the mesh and rig adapters share: the API client, the clock, and the request helpers.
#[derive(Clone)]
pub struct TripoApi {
    pub client: Client,
    pub clock: Arc<dyn Clock>,
}

/// One status read of a task (spec/providers.md §9.4 "Tasks"). Its `Debug` names the output's
/// members but never their values, which hold signed model URLs (spec/providers.md §8).
#[derive(Clone, PartialEq)]
pub struct Task {
    pub status: String,
    pub output: Map<String, Value>,
    pub credits_consumed: Option<Value>,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("status", &self.status)
            .field("output", &self.output.keys().collect::<Vec<_>>())
            .field("credits_consumed", &self.credits_consumed)
            .finish()
    }
}

/// How waiting for a task ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Waited {
    /// `success`.
    Done(Task),
    /// A terminal status other than success (`failed`, `cancelled`, `banned`, `expired`): the
    /// status.
    Ended(String),
    /// Still `queued`/`running` at the deadline, or every read failed until it, or an answer for
    /// another task or with an unknown status: the reason.
    Unreachable(String),
}

impl TripoApi {
    pub fn new(setup: &Setup) -> TripoApi {
        TripoApi {
            client: setup.client(KeyName::Tripo, &setup.endpoints.tripo),
            clock: Arc::clone(&setup.clock),
        }
    }

    /// The `data` object of an API answer: HTTP 200, a JSON object, `code` a number equal to 0, and
    /// `data` an object. The refusal is a fixed sentence (`Tripo HTTP <status>; body withheld`,
    /// `Tripo answered with something other than JSON`, `Tripo answered with an unsuccessful
    /// envelope`, `Tripo answered without data`).
    pub fn data(response: &HttpResponse) -> Result<Map<String, Value>, String> {
        if response.status != 200 {
            return Err(format!("Tripo HTTP {}; body withheld", response.status));
        }
        let Ok(value) = serde_json::from_slice::<Value>(&response.body) else {
            return Err("Tripo answered with something other than JSON".into());
        };
        let Value::Object(mut envelope) = value else {
            return Err("Tripo answered with an unsuccessful envelope".into());
        };
        let succeeded = matches!(
            envelope.get("code"),
            Some(Value::Number(code)) if grida_fx_core::value::as_f64(code) == 0.0
        );
        if !succeeded {
            return Err("Tripo answered with an unsuccessful envelope".into());
        }
        match envelope.remove("data") {
            Some(Value::Object(data)) => Ok(data),
            _ => Err("Tripo answered without data".into()),
        }
    }

    /// Uploads one file (`POST /files`, multipart field `file`) and returns its `file_token`.
    /// A failure is free-phase (spec/providers.md §9.4): `Refused` when it is deterministic (400,
    /// 401, 403, 404, 413, 415, 422, a missing key, the network off), else `NotReceived`.
    pub async fn upload(
        &self,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, Submitted> {
        let credential = self.credential().map_err(|reason| self.refused(&reason))?;
        let request = HttpRequest::new(Method::Post, self.client.url("/files"), Lane::Upload)
            .credential(credential)
            .body(Body::Multipart(vec![Part::file(
                "file",
                filename,
                content_type,
                bytes,
            )]))
            .timeout(READ_DEADLINE);
        let response = match self.send(request).await {
            Ok(response) => response,
            Err(error) if refused_by_transport(&error) => {
                return Err(
                    self.refused(&format!("the Tripo upload was not sent: {}", error.reason))
                );
            }
            Err(error) => {
                return Err(
                    self.not_received(&format!("the Tripo upload failed: {}", error.reason))
                );
            }
        };
        if FREE_REFUSED.contains(&response.status) {
            return Err(self.refused(&format!(
                "Tripo refused the upload with HTTP {}{}",
                response.status,
                error_code(&response.body)
            )));
        }
        let data = Self::data(&response).map_err(|reason| self.took_nothing(reason, &response))?;
        match data.get("file_token") {
            Some(Value::String(token)) if FILE_TOKEN.is_match(token) => Ok(token.clone()),
            _ => Err(self.not_received("Tripo's upload answer has no usable file handle")),
        }
    }

    /// Polls a task until it ends or `deadline` passes (measured from the call).
    ///
    /// The first read always happens. A read that fails (a transport failure, a 429, a 5xx, an
    /// unreadable envelope) is a poll that saw nothing; after each read the deadline is checked,
    /// then the clock sleeps [`POLL`]. A 401, a 403, a 3xx or a transport refusal ends polling at
    /// once, since reading again cannot help (spec/providers.md §7).
    pub async fn wait(&self, task_id: &str, deadline: Duration) -> Waited {
        if !TASK_ID.is_match(task_id) {
            return Waited::Unreachable("not a Tripo task id".into());
        }
        let credential = match self.credential() {
            Ok(credential) => credential,
            Err(reason) => return Waited::Unreachable(self.client.reason(&reason)),
        };
        let stop = |text: String| Waited::Unreachable(self.client.reason(&text));
        let start = self.clock.now();
        let mut last_status: Option<String> = None;
        loop {
            let request = HttpRequest::new(
                Method::Get,
                self.client.url(&format!("/tasks/{task_id}")),
                Lane::Provider,
            )
            .credential(credential.clone())
            .timeout(READ_DEADLINE);
            match self.send(request).await {
                Err(error) if refused_by_transport(&error) => {
                    return stop(format!(
                        "Tripo task {task_id} could not be read: {}",
                        error.reason
                    ));
                }
                Err(_) => {}
                Ok(response) if matches!(response.status, 300..=399 | 401 | 403) => {
                    return stop(format!(
                        "Tripo task {task_id} could not be read: HTTP {}",
                        response.status
                    ));
                }
                Ok(response) => {
                    if let Ok(mut data) = Self::data(&response) {
                        let same_task =
                            data.get("task_id").and_then(Value::as_str) == Some(task_id);
                        let status = match data.get("status").and_then(Value::as_str) {
                            Some(status)
                                if same_task
                                    && (status == "success"
                                        || RUNNING.contains(&status)
                                        || ENDED.contains(&status)) =>
                            {
                                status.to_string()
                            }
                            _ => {
                                return stop(
                                    "Tripo answered for another task, or with an unknown status"
                                        .into(),
                                );
                            }
                        };
                        if status == "success" {
                            let output = match data.remove("output") {
                                Some(Value::Object(output)) => output,
                                _ => Map::new(),
                            };
                            return Waited::Done(Task {
                                status,
                                output,
                                credits_consumed: data.remove("credits_consumed"),
                            });
                        }
                        if ENDED.contains(&status.as_str()) {
                            return Waited::Ended(status);
                        }
                        last_status = Some(status);
                    }
                }
            }
            if self.clock.now().saturating_sub(start) >= deadline {
                return stop(match &last_status {
                    Some(status) => {
                        format!("Tripo task {task_id} is still {status}; collect it later")
                    }
                    None => format!("Tripo task {task_id} could not be read; collect it later"),
                });
            }
            self.clock.sleep(POLL).await;
        }
    }

    /// The model URLs of a finished task's `output`, in first-appearance order (spec/providers.md
    /// §9.4 "Model URLs"): `(field path, url)`, 1 to 8 distinct.
    ///
    /// A depth-first walk of objects only, in key order (arrays are not entered; a key that does
    /// not match `[A-Za-z0-9_]+` is skipped with its subtree). A string is a model URL when it
    /// starts with `https://` and some key on its path contains `model` or `mesh`. A URL longer
    /// than 20 480 characters or holding a control character refuses the whole output; a URL
    /// named twice keeps its first field.
    pub fn model_urls(output: &Map<String, Value>) -> Result<Vec<(String, String)>, String> {
        let mut found: IndexMap<String, String> = IndexMap::new();
        let mut path = Vec::new();
        visit_output(output, &mut path, &mut found)?;
        if !(1..=MAX_MODEL_URLS).contains(&found.len()) {
            return Err("a finished Tripo task has no model to download".into());
        }
        Ok(found.into_iter().map(|(url, field)| (field, url)).collect())
    }

    /// Downloads one model from Tripo's own hosts and sniffs its kind: `Ok((kind, bytes))`;
    /// `Err(Download::…)` says whether the job is over (`Ended`) or may be collected again
    /// (`Unreachable`).
    pub async fn download(&self, url: &str) -> Result<(String, Vec<u8>), Download> {
        check_model_host(url).map_err(|reason| Download::Ended(reason.into()))?;
        let redactor = self.client.redactor.with(url);
        let request = HttpRequest::new(Method::Get, url, Lane::Download)
            .timeout(READ_DEADLINE)
            .max_response_bytes(MAX_MODEL_BYTES);
        let too_large = || Download::Unreachable("a Tripo model is larger than 150 MB".into());
        let response = match self.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::TooLarge => return Err(too_large()),
            Err(_) => {
                return Err(Download::Unreachable(
                    redactor.reason("Tripo download failed; URL withheld"),
                ));
            }
        };
        if response.status != 200 {
            return Err(Download::Unreachable(
                redactor.reason(&format!("Tripo download HTTP {}", response.status)),
            ));
        }
        if response.body.len() as u64 > MAX_MODEL_BYTES {
            return Err(too_large());
        }
        match crate::wire::sniff_model(&response.body) {
            Some(kind) => Ok((kind.to_string(), response.body)),
            None => Err(Download::Ended(
                "Tripo returned a model that is neither GLB nor binary FBX".into(),
            )),
        }
    }

    /// Sends one request through the client (no retry).
    pub async fn send(
        &self,
        request: crate::transport::HttpRequest,
    ) -> Result<HttpResponse, TransportError> {
        self.client.send(request).await
    }

    /// The API credential, or `TRIPO_API_KEY is not set`.
    fn credential(&self) -> Result<Credential, String> {
        self.client.credential("authorization", "Bearer ")
    }

    /// Posts the free riggability check (`POST /animations/rig-check`) and returns its task id.
    /// Every failure is free-phase: `Refused` or `NotReceived`.
    pub(crate) async fn post_check(&self, file_token: &str) -> Result<String, Submitted> {
        let credential = self.credential().map_err(|reason| self.refused(&reason))?;
        let request = HttpRequest::new(
            Method::Post,
            self.client.url("/animations/rig-check"),
            Lane::Provider,
        )
        .credential(credential)
        .body(Body::Json(serde_json::json!({"input": file_token})))
        .timeout(POST_DEADLINE);
        let response = match self.send(request).await {
            Ok(response) => response,
            Err(error) if refused_by_transport(&error) => {
                return Err(self.refused(&format!(
                    "the Tripo riggability check was not sent: {}",
                    error.reason
                )));
            }
            Err(error) => {
                return Err(self.not_received(&format!(
                    "the Tripo riggability check failed: {}",
                    error.reason
                )));
            }
        };
        if FREE_REFUSED.contains(&response.status) {
            return Err(self.refused(&format!(
                "Tripo refused the riggability check with HTTP {}",
                response.status
            )));
        }
        let data = Self::data(&response).map_err(|reason| self.took_nothing(reason, &response))?;
        match data.get("task_id") {
            Some(Value::String(id)) if TASK_ID.is_match(id) => Ok(id.clone()),
            _ => Err(self.not_received("Tripo's answer to the riggability check has no task id")),
        }
    }

    /// Posts a paid task exactly once (spec/providers.md §9.4 "Paid POST") and returns its task
    /// id, or the outcome the submit reports. An `Uncertain` after a 200 names the task id Tripo
    /// returned when it is a safe field, so a person can find the task.
    pub(crate) async fn post_task(&self, path: &str, body: Value) -> Result<String, Submitted> {
        let credential = self.credential().map_err(|reason| self.refused(&reason))?;
        let request = HttpRequest::new(Method::Post, self.client.url(path), Lane::Provider)
            .credential(credential)
            .body(Body::Json(body))
            .timeout(POST_DEADLINE);
        let uncertain = |detail: &str| Submitted::Uncertain {
            reason: self.client.reason(&format!(
                "Tripo may have taken the task; it is not posted again ({detail})"
            )),
        };
        let response = match self.send(request).await {
            Ok(response) => response,
            Err(error) if refused_by_transport(&error) => {
                return Err(self.refused(&format!("the Tripo task was not sent: {}", error.reason)));
            }
            Err(error) if error.phase == Phase::NotSent => {
                return Err(
                    self.not_received(&format!("the Tripo task was not sent: {}", error.reason))
                );
            }
            Err(error) => return Err(uncertain(&error.reason)),
        };
        if response.status == 429 {
            return Err(self.took_nothing("Tripo took no task (HTTP 429)".into(), &response));
        }
        if PAID_REFUSED.contains(&response.status) {
            return Err(Submitted::Failed {
                reason: self.client.reason(&format!(
                    "Tripo refused the task with HTTP {}{}",
                    response.status,
                    error_code(&response.body)
                )),
                cost: None,
                retryable: false,
            });
        }
        let data = Self::data(&response).map_err(|reason| uncertain(&reason))?;
        match data.get("task_id") {
            Some(Value::String(id)) if TASK_ID.is_match(id) => Ok(id.clone()),
            Some(Value::String(id)) if SAFE_FIELD.is_match(id) => Err(uncertain(&format!(
                "Tripo's answer names task {id}, which is not a task id FX collects"
            ))),
            Some(Value::String(id)) if !id.trim().is_empty() => {
                Err(uncertain("Tripo's answer has a malformed task id"))
            }
            _ => Err(uncertain("Tripo's answer has no task id")),
        }
    }

    /// Waits for a submitted task, then downloads every model it names, in order: the finished
    /// task and the `(kind, bytes)` of each model, or the outcome the collect reports.
    #[allow(clippy::result_large_err)] // `Collected` is what every caller returns at once
    pub(crate) async fn finished_models(
        &self,
        task_id: &str,
        deadline: Duration,
    ) -> Result<(Task, Vec<(String, Vec<u8>)>), Collected> {
        let task = match self.wait(task_id, deadline).await {
            Waited::Done(task) => task,
            Waited::Ended(status) => {
                return Err(Collected::Ended {
                    reason: self
                        .client
                        .reason(&format!("Tripo ended task {task_id} as {status}")),
                });
            }
            Waited::Unreachable(reason) => return Err(Collected::Unreachable { reason }),
        };
        let urls = Self::model_urls(&task.output).map_err(|reason| Collected::Ended {
            reason: self.client.reason(&reason),
        })?;
        let mut models = Vec::with_capacity(urls.len());
        for (_, url) in &urls {
            match self.download(url).await {
                Ok(model) => models.push(model),
                Err(Download::Ended(reason)) => return Err(Collected::Ended { reason }),
                Err(Download::Unreachable(reason)) => {
                    return Err(Collected::Unreachable { reason });
                }
            }
        }
        Ok((task, models))
    }

    /// `Submitted::Refused` with a redacted reason.
    pub(crate) fn refused(&self, reason: &str) -> Submitted {
        Submitted::Refused {
            reason: self.client.reason(reason),
        }
    }

    /// `Submitted::NotReceived` with a redacted reason and no wait asked for.
    pub(crate) fn not_received(&self, reason: &str) -> Submitted {
        Submitted::not_received(self.client.reason(reason))
    }

    /// `Submitted::NotReceived` for an answer that took nothing. Tripo documents no
    /// `retry-after`, but a response that sends one is honoured (spec/providers.md §4.3): the
    /// header sets `retry_after`, and a 429's reason names it.
    pub(crate) fn took_nothing(&self, reason: String, response: &HttpResponse) -> Submitted {
        Submitted::NotReceived {
            reason: self.client.reason(&with_retry_after(reason, response)),
            retry_after: crate::adapter::retry_after(response),
        }
    }

    /// `Collected::Unreachable` with a redacted reason.
    pub(crate) fn unreachable(&self, reason: &str) -> Collected {
        Collected::Unreachable {
            reason: self.client.reason(reason),
        }
    }
}

/// Why a download failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Download {
    /// The task's output is unusable: `Collected::Ended`.
    Ended(String),
    /// Try again in a later collect: `Collected::Unreachable`.
    Unreachable(String),
}

impl std::fmt::Debug for TripoApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TripoApi")
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

/// Registers the two adapters on provider `tripo`.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    let api = TripoApi::new(setup);
    adapters.register(
        "mesh.generate",
        "tripo",
        Adapter::Job(Arc::new(mesh::TripoMesh::new(api.clone()))),
    );
    adapters.register(
        "mesh.rig",
        "tripo",
        Adapter::Job(Arc::new(rig::TripoRig::new(api))),
    );
}

/// The route check of spec/providers.md §5 step 1: the call is on `capability`, and the route's
/// contract names `adapter` when it names one.
pub(crate) fn check_route(
    call: &CallRequest,
    capability: &str,
    adapter: &str,
) -> Result<(), String> {
    let named = match call.route.contract.get("adapter") {
        None | Some(Value::Null) => true,
        Some(Value::String(name)) => name == adapter,
        Some(_) => false,
    };
    if call.route.capability == capability && named {
        Ok(())
    } else {
        Err(format!(
            "{} is not a {capability} route this adapter serves",
            call.route.id()
        ))
    }
}

/// The task id of a handle this adapter wrote, if it has a usable one.
pub(crate) fn handle_task_id(handle: &Value) -> Option<&str> {
    handle
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|id| TASK_ID.is_match(id))
}

/// The reported cost of a finished task: `credits_consumed`, a number or a decimal string, ×
/// USD 0.01, rounded up (spec/providers.md §6). Booleans, `null`, objects, lists, a negative or
/// non-decimal value: `None`.
pub(crate) fn credits_cost(credits: Option<&Value>) -> Option<Usd> {
    let text = match credits? {
        Value::Number(number) => {
            let x = grida_fx_core::value::as_f64(number);
            if !x.is_finite() {
                return None;
            }
            grida_fx_core::value::format_number(x)
        }
        Value::String(text) => text.trim().to_string(),
        _ => return None,
    };
    crate::wire::decimal_micros_ceil(&text, MICROS_PER_CREDIT)
}

/// A value from Tripo kept in a handle or an answer's `data`: a short plain string
/// (`^[A-Za-z0-9_.:-]{1,96}$`) as is; anything else is `null`, since a handle and an answer never
/// hold a provider URL or other free text (spec/providers.md §4.4, §7).
pub(crate) fn plain_or_null(value: &Value) -> Value {
    match value {
        Value::String(text) if SAFE_FIELD.is_match(text) => value.clone(),
        _ => Value::Null,
    }
}

/// A value from Tripo shown in a reason: `null`, a boolean, or a short plain string as JSON;
/// anything else is withheld (spec/providers.md §8).
pub(crate) fn shown(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) => value.to_string(),
        Value::String(text) if SAFE_FIELD.is_match(text) => value.to_string(),
        _ => "(withheld)".to_string(),
    }
}

/// Whether the transport refused the request itself (spec/providers.md §4.2): the network is off
/// or the request broke the transport's rules. Nothing left.
fn refused_by_transport(error: &TransportError) -> bool {
    error.phase == Phase::NotSent && error.kind == TransportErrorKind::Refused
}

/// A 429's `retry-after` header, when it is a plain number of seconds, appended to `reason`
/// (spec/providers.md §4.3).
fn with_retry_after(reason: String, response: &HttpResponse) -> String {
    match response.header("retry-after").map(str::trim) {
        Some(seconds)
            if response.status == 429
                && !seconds.is_empty()
                && seconds.len() <= 6
                && seconds.bytes().all(|b| b.is_ascii_digit()) =>
        {
            format!("{reason}; retry-after {seconds} s")
        }
        _ => reason,
    }
}

/// The walk of [`TripoApi::model_urls`]: `found` maps each URL to its first field path.
fn visit_output<'a>(
    object: &'a Map<String, Value>,
    path: &mut Vec<&'a str>,
    found: &mut IndexMap<String, String>,
) -> Result<(), String> {
    for (key, child) in object {
        if !OUTPUT_KEY.is_match(key) {
            continue;
        }
        path.push(key);
        match child {
            Value::Object(inner) => visit_output(inner, path, found)?,
            Value::String(url)
                if url.starts_with("https://")
                    && path
                        .iter()
                        .any(|field| field.contains("model") || field.contains("mesh")) =>
            {
                if url.chars().count() > MAX_URL_CHARS || url.chars().any(|c| c < '\u{20}') {
                    return Err("a Tripo model address is malformed; details withheld".into());
                }
                if !found.contains_key(url) {
                    found.insert(url.clone(), path.join("."));
                }
            }
            _ => {}
        }
        path.pop();
    }
    Ok(())
}

/// A model URL is on one of Tripo's own hosts (spec/providers.md §9.4 "Tasks"): `https`, no
/// userinfo, port absent or 443, and the host `tripo3d.ai`, a subdomain of it, or Tripo's storage
/// host.
fn check_model_host(url: &str) -> Result<(), &'static str> {
    let Ok(parsed) = url::Url::parse(url) else {
        return Err("a Tripo model address is malformed; details withheld");
    };
    let outside = "a Tripo model address is outside Tripo's own hosts";
    let Some(url::Host::Domain(host)) = parsed.host() else {
        return Err(outside);
    };
    let host = host.to_ascii_lowercase();
    let ours = host == "tripo3d.ai" || host.ends_with(".tripo3d.ai") || host == STORAGE_HOST;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.port(), None | Some(443))
        || !ours
    {
        return Err(outside);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("an object"),
        }
    }

    #[test]
    fn the_envelope_is_http_200_with_code_0_and_data() {
        let ok = HttpResponse::json(200, &json!({"code": 0, "data": {"file_token": "t"}}));
        assert_eq!(
            TripoApi::data(&ok).unwrap(),
            object(json!({"file_token": "t"}))
        );
        let zero_float = HttpResponse::json(200, &json!({"code": 0.0, "data": {}}));
        assert!(TripoApi::data(&zero_float).is_ok());
        let cases = [
            (
                HttpResponse::json(201, &json!({"code": 0, "data": {}})),
                "Tripo HTTP 201; body withheld",
            ),
            (
                HttpResponse::new(500, b"secret text".to_vec()),
                "Tripo HTTP 500; body withheld",
            ),
            (
                HttpResponse::new(200, b"<html>".to_vec()),
                "Tripo answered with something other than JSON",
            ),
            (
                HttpResponse::json(200, &json!([0])),
                "Tripo answered with an unsuccessful envelope",
            ),
            (
                HttpResponse::json(200, &json!({"code": false, "data": {}})),
                "Tripo answered with an unsuccessful envelope",
            ),
            (
                HttpResponse::json(200, &json!({"code": "0", "data": {}})),
                "Tripo answered with an unsuccessful envelope",
            ),
            (
                HttpResponse::json(200, &json!({"code": 2001, "message": "no", "data": {}})),
                "Tripo answered with an unsuccessful envelope",
            ),
            (
                HttpResponse::json(200, &json!({"data": {}})),
                "Tripo answered with an unsuccessful envelope",
            ),
            (
                HttpResponse::json(200, &json!({"code": 0})),
                "Tripo answered without data",
            ),
            (
                HttpResponse::json(200, &json!({"code": 0, "data": []})),
                "Tripo answered without data",
            ),
        ];
        for (response, sentence) in cases {
            assert_eq!(TripoApi::data(&response).unwrap_err(), sentence);
        }
    }

    #[test]
    fn model_urls_walk_objects_in_key_order() {
        let a = "https://a.tripo3d.ai/a.glb";
        let b = "https://b.tripo3d.ai/b.fbx";
        let found = TripoApi::model_urls(&object(json!({
            "rendered_image": "https://a.tripo3d.ai/x.png",
            "pbr_model": b,
            "result": {"mesh": {"url": a}, "other": {"url": "https://c.tripo3d.ai/c.glb"}},
            "models": [a],
            "pbr-model": "https://d.tripo3d.ai/d.glb",
            "model": "http://e.tripo3d.ai/e.glb",
            "base_model": b,
            "Model": "https://f.tripo3d.ai/f.glb",
        })))
        .unwrap();
        assert_eq!(
            found,
            vec![
                ("pbr_model".to_string(), b.to_string()),
                ("result.mesh.url".to_string(), a.to_string()),
            ]
        );
    }

    #[test]
    fn model_urls_need_one_to_eight_well_formed_addresses() {
        let none = "a finished Tripo task has no model to download";
        assert_eq!(TripoApi::model_urls(&Map::new()).unwrap_err(), none);
        assert_eq!(
            TripoApi::model_urls(&object(json!({"image": "https://a.tripo3d.ai/a.png"})))
                .unwrap_err(),
            none
        );
        let eight: Map<String, Value> = (0..8)
            .map(|i| {
                (
                    format!("model_{i}"),
                    json!(format!("https://a.tripo3d.ai/{i}")),
                )
            })
            .collect();
        assert_eq!(TripoApi::model_urls(&eight).unwrap().len(), 8);
        let mut nine = eight.clone();
        nine.insert("model_8".into(), json!("https://a.tripo3d.ai/8"));
        assert_eq!(TripoApi::model_urls(&nine).unwrap_err(), none);
        let malformed = "a Tripo model address is malformed; details withheld";
        assert_eq!(
            TripoApi::model_urls(&object(json!({"model": "https://a.tripo3d.ai/\nx"})))
                .unwrap_err(),
            malformed
        );
        let long = format!("https://a.tripo3d.ai/{}", "x".repeat(MAX_URL_CHARS));
        assert_eq!(
            TripoApi::model_urls(&object(json!({"model": long}))).unwrap_err(),
            malformed
        );
        let exact = format!(
            "https://a.tripo3d.ai/{}",
            "x".repeat(MAX_URL_CHARS - "https://a.tripo3d.ai/".len())
        );
        assert_eq!(exact.chars().count(), MAX_URL_CHARS);
        assert!(TripoApi::model_urls(&object(json!({"model": exact}))).is_ok());
        // A malformed string that is not a candidate does not matter.
        assert!(
            TripoApi::model_urls(&object(json!({
                "image": "https://a.tripo3d.ai/\nx", "model": "https://a.tripo3d.ai/m.glb"
            })))
            .is_ok()
        );
    }

    #[test]
    fn model_hosts_are_tripos_own() {
        for good in [
            "https://tripo3d.ai/m.glb",
            "https://api.tripo3d.ai/m.glb",
            "https://API.Tripo3D.ai:443/m.glb",
            "https://tripo-data.rg1.data.tripo3d.com/out/model.fbx?sig=1",
        ] {
            assert_eq!(check_model_host(good), Ok(()), "{good}");
        }
        let outside = "a Tripo model address is outside Tripo's own hosts";
        for bad in [
            "https://example.com/model.glb",
            "http://api.tripo3d.ai/m.glb",
            "https://user@api.tripo3d.ai/m.glb",
            "https://user:pw@api.tripo3d.ai/m.glb",
            "https://api.tripo3d.ai:8443/m.glb",
            "https://eviltripo3d.ai/m.glb",
            "https://tripo3d.ai.example.com/m.glb",
            "https://other.data.tripo3d.com/m.glb",
            "https://1.2.3.4/m.glb",
        ] {
            assert_eq!(check_model_host(bad), Err(outside), "{bad}");
        }
        for malformed in ["not a url", "https://api.tripo3d.ai:99999/m.glb"] {
            assert_eq!(
                check_model_host(malformed),
                Err("a Tripo model address is malformed; details withheld"),
                "{malformed}"
            );
        }
    }

    #[test]
    fn credits_cost_rounds_up_to_micro_dollars() {
        let cases = [
            (json!(125), Some(Usd(1_250_000))),
            (json!("125"), Some(Usd(1_250_000))),
            (json!(" 125 "), Some(Usd(1_250_000))),
            (json!(12.5), Some(Usd(125_000))),
            (json!(0), Some(Usd(0))),
            (json!(0.00001), Some(Usd(1))),
            (json!(0.00005), Some(Usd(1))),
            (json!(0.00015), Some(Usd(2))),
            (json!(true), None),
            (json!(-1), None),
            (json!("-1"), None),
            (Value::Null, None),
            (json!("abc"), None),
            (json!({}), None),
            (json!([125]), None),
        ];
        for (credits, cost) in cases {
            assert_eq!(credits_cost(Some(&credits)), cost, "{credits}");
        }
        assert_eq!(credits_cost(None), None);
    }

    #[test]
    fn only_plain_strings_are_kept() {
        assert_eq!(plain_or_null(&json!("quadruped")), json!("quadruped"));
        for other in [
            json!(null),
            json!(true),
            json!(3),
            json!("has spaces"),
            json!("https://tripo-data.rg1.data.tripo3d.com/a.json?Signature=1"),
            json!({"preview": "x"}),
            json!(["biped"]),
            json!("x".repeat(97)),
        ] {
            assert_eq!(plain_or_null(&other), Value::Null, "{other}");
        }
    }

    #[test]
    fn a_task_debug_withholds_output_values() {
        let task = Task {
            status: "success".into(),
            output: object(
                json!({"model": "https://tripo-data.rg1.data.tripo3d.com/m.glb?Signature=s3cr3t"}),
            ),
            credits_consumed: Some(json!(30)),
        };
        let shown = format!("{:?}", Waited::Done(task));
        assert!(!shown.contains("s3cr3t"), "{shown}");
        assert!(!shown.contains("https://"), "{shown}");
        assert!(shown.contains("\"model\""), "{shown}");
    }

    #[test]
    fn only_plain_values_are_shown_in_reasons() {
        assert_eq!(shown(&json!(false)), "false");
        assert_eq!(shown(&Value::Null), "null");
        assert_eq!(shown(&json!("quadruped")), "\"quadruped\"");
        assert_eq!(shown(&json!("has spaces")), "(withheld)");
        assert_eq!(shown(&json!({"a": 1})), "(withheld)");
        assert_eq!(shown(&json!(3)), "(withheld)");
    }

    #[test]
    fn a_retry_after_goes_into_a_429_reason_only() {
        let limited = HttpResponse::new(429, Vec::new()).with_header("Retry-After", "30");
        assert_eq!(
            with_retry_after("x".into(), &limited),
            "x; retry-after 30 s"
        );
        let dated = HttpResponse::new(429, Vec::new())
            .with_header("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT");
        assert_eq!(with_retry_after("x".into(), &dated), "x");
        let other = HttpResponse::new(503, Vec::new()).with_header("retry-after", "30");
        assert_eq!(with_retry_after("x".into(), &other), "x");
    }

    #[test]
    fn handles_need_a_usable_task_id() {
        assert_eq!(
            handle_task_id(&json!({"task_id": "task-1_a"})),
            Some("task-1_a")
        );
        for bad in [
            json!({}),
            json!({"task_id": 7}),
            json!({"task_id": ""}),
            json!({"task_id": "a/b"}),
            json!({"task_id": "x".repeat(129)}),
            json!("task1"),
        ] {
            assert_eq!(handle_task_id(&bad), None, "{bad}");
        }
    }
}
