//! GG jobs: what the video, 3D and rigging adapters share (spec/providers.md §9.7, §7). Each
//! submits one paid request that GG answers with a job, then reads `GET {base}/jobs/<job_id>`
//! until GG has the result, downloads its one file once, and waits a little for GG to settle the
//! job's charge. A [`Kind`] names what differs: the labels, where the result's file is, its cap,
//! and what its bytes must be.
//!
//! Submit statuses: a transport refusal is `Refused`, `not_sent` is `NotReceived`, `after_send` is
//! `Uncertain`. A 2xx without a usable job, a 3xx and a 5xx are `Uncertain`; 408 and 429 (but
//! `key_budget_exceeded`; one that asks for a new key renews it, as [`super::Gateway::exchange`]
//! does) are `NotReceived`; any other 4xx in GG's error envelope
//! is `Failed { Some(0), false }` (GG refuses before it takes the job, and does not bill a
//! refusal), and without the envelope `Failed { None, false }`. An `Uncertain` names GG's request
//! id and the attempt's `Idempotency-Key`, so a person can find the job in GG's console. A 2xx job
//! whose `id` is one of GG's ids (`^[A-Za-z0-9][A-Za-z0-9_.-]{0,159}$`: never `.` or `..`, since it
//! goes in a path) becomes the handle `{"job_id"}`.
//!
//! Collect: reads the job every [`POLL`] until GG has the result, or ends the job without one,
//! within [`COLLECT_DEADLINE`] from the start of `collect`, each read bounded by the smaller of the
//! time left and [`READ_DEADLINE`]. Then downloads the result's file **once** (https, no
//! credential, the URL exactly as GG gave it, at most the kind's cap), and the kind's
//! [`Deliver`] judges the bytes. When GG has not settled the job's billing yet, the job is read
//! again, each [`POLL`], for at most [`SETTLE_WAIT`]. The answer's cost is
//! `billing.charged_mills` once `billing.status` is `settled`, else none: the engine then charges
//! the whole hold. `billing.authorized_mills` is GG's authorization, never its charge, and is not
//! read.
//!
//! Collect outcomes (spec/providers.md §7): a handle this adapter did not write is `Unreachable`
//! with nothing requested. A read that fails (a transport failure, 408, 429, a 5xx, a body that is
//! not a JSON object) saw nothing, and polling goes on. A 3xx, 401, 403, 404, another 4xx, a job
//! with another id, a status GG does not document and the deadline are `Unreachable`: the job may
//! still run. A 410 (`job_expired`) is `Ended`. A job that `succeeded` or `failed` with its result
//! `unavailable` or `expired` is `Ended`, at its settled charge, else none. The download is made
//! once: a transport failure, a 3xx, 401, 403, 408, 429 or 5xx is `Unreachable` (a later collect
//! reads the job and downloads again); another 4xx, an empty body, one over the cap or bytes the
//! kind refuses is `Ended`, at the job's settled charge, else none.

use super::Gateway;
use crate::adapter::{Answer, CallRequest, Collected, Submitted};
use crate::clock::Clock;
use crate::transport::{
    Credential, HttpRequest, HttpResponse, Lane, Method, Phase, TransportError, TransportErrorKind,
};
use crate::wire;
use grida_fx_core::money::Usd;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// The submit's deadline.
pub const SUBMIT_DEADLINE: Duration = Duration::from_secs(120);

/// How long one `collect` waits for the job, from its start.
pub const COLLECT_DEADLINE: Duration = Duration::from_secs(1800);

/// The polling interval. GG asks for no more than one read a second.
pub const POLL: Duration = Duration::from_secs(5);

/// The longest single read of the job.
pub const READ_DEADLINE: Duration = Duration::from_secs(60);

/// How long a collect that has the file waits for GG to settle the job's billing.
pub const SETTLE_WAIT: Duration = Duration::from_secs(60);

/// The response cap of the submit and of each job read (small JSON objects).
pub const JSON_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

static JOB_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,159}$").expect("a valid pattern"));

/// What a collected file's bytes become: the answer (its file and data, without a cost), or the
/// sentence that ends the job.
pub type Deliver = fn(Vec<u8>) -> Result<Answer, String>;

/// What differs between GG's jobs (module doc).
#[derive(Debug, Clone, Copy)]
pub struct Kind {
    /// What GG makes, for sentences: `video`, `3D model`, `rigged model`.
    pub noun: &'static str,
    /// The label of the submit's reasons.
    pub submit_label: &'static str,
    /// The label of a job read's reasons.
    pub job_label: &'static str,
    /// The label of the download's reasons.
    pub download_label: &'static str,
    /// The sentence of a job still running at the deadline.
    pub outstanding: &'static str,
    /// The result's file reference (an object with `url`), from the job's `result`.
    pub reference: fn(&Value) -> Option<&Value>,
    /// The most bytes of the file.
    pub max_bytes: u64,
    /// What the bytes become.
    pub deliver: Deliver,
}

/// The job id a handle holds, when it is exactly one these adapters write.
pub fn job_id(handle: &Value) -> Option<&str> {
    let members = handle.as_object()?;
    if members.len() != 1 {
        return None;
    }
    members
        .get("job_id")
        .and_then(Value::as_str)
        .filter(|id| JOB_ID.is_match(id))
}

/// The handle of a 2xx submit answer, or the `Uncertain` sentence.
pub fn handle_of(body: &[u8], kind: &Kind) -> Result<Value, String> {
    let lost = || {
        format!(
            "GG took the {} job but returned no job to collect it by",
            kind.noun
        )
    };
    let job = wire::json_object(body, kind.submit_label).map_err(|_| lost())?;
    match job.get("id").and_then(Value::as_str) {
        Some(id) if JOB_ID.is_match(id) => Ok(json!({"job_id": id})),
        _ => Err(lost()),
    }
}

/// What a job's `billing` says it cost: its `charged_mills` once `status` is `settled`; `None`
/// before, or for a charge that is not GG's money.
pub fn billing(job: &Map<String, Value>) -> Option<Usd> {
    let billing = job.get("billing")?.as_object()?;
    if billing.get("status").and_then(Value::as_str) != Some("settled") {
        return None;
    }
    billing
        .get("charged_mills")
        .and_then(Value::as_str)
        .and_then(super::mills)
}

/// What a job read saw.
enum Read {
    /// The job, as GG answered it.
    Job(Map<String, Value>),
    /// Nothing: poll again.
    Nothing,
    /// Collecting stops here.
    Stop(Collected),
}

/// Where a job stands.
#[derive(Debug, PartialEq)]
pub enum Stage {
    /// Still running, or its result is still coming.
    Waiting,
    /// GG has the result document.
    Available,
    /// Ended without a result; the sentence.
    Ended(String),
    /// Not something FX can collect; the sentence.
    Unknown(String),
}

/// Where `job` stands (module doc); `label` names it in sentences.
pub fn stage(job: &Map<String, Value>, label: &str) -> Stage {
    let status = job.get("status").and_then(Value::as_str).unwrap_or("");
    let result_status = job
        .get("result_status")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !matches!(
        status,
        "accepted" | "running" | "unknown" | "succeeded" | "failed"
    ) {
        return Stage::Unknown(format!("{label} has a status FX does not know"));
    }
    match result_status {
        "available" => Stage::Available,
        "unavailable" | "expired" if matches!(status, "succeeded" | "failed") => {
            Stage::Ended(job_error(job, label, status, result_status))
        }
        "pending" | "unavailable" | "expired" => Stage::Waiting,
        _ => Stage::Unknown(format!("{label} has a result status FX does not know")),
    }
}

/// The URL of the result's file, when it is https with a host, no userinfo and no fragment.
/// Otherwise the `Ended` sentence. A declared `media_type` is not read: the bytes decide.
pub fn result_file<'a>(job: &'a Map<String, Value>, kind: &Kind) -> Result<&'a str, String> {
    let url = job
        .get("result")
        .and_then(kind.reference)
        .and_then(|reference| reference.get("url"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{} result carries no {}", kind.job_label, kind.noun))?;
    let https = url::Url::parse(url).ok().is_some_and(|parsed| {
        parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.fragment().is_none()
    });
    if https {
        Ok(url)
    } else {
        Err(format!("GG output {} url must be https", kind.noun))
    }
}

/// `<label> <status> without a result (<result_status>)`, with GG's error `code` when it is a plain
/// name.
fn job_error(job: &Map<String, Value>, label: &str, status: &str, result_status: &str) -> String {
    static CODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,63}$").expect("a valid pattern"));
    let code = job
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .filter(|code| CODE.is_match(code));
    match code {
        Some(code) => format!("{label} {status} without a result ({result_status}): code={code}"),
        None => format!("{label} {status} without a result ({result_status})"),
    }
}

/// The submit, collect and charge of GG's jobs over one gateway and clock (module doc).
#[derive(Clone)]
pub struct Jobs {
    pub gateway: Gateway,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jobs")
            .field("gateway", &self.gateway)
            .finish_non_exhaustive()
    }
}

impl Jobs {
    pub fn new(gateway: Gateway, clock: Arc<dyn Clock>) -> Jobs {
        Jobs { gateway, clock }
    }

    /// The clock's time.
    pub fn now(&self) -> Duration {
        self.clock.now()
    }

    /// Sends `call`'s prepared submit once, and reads its answer by the submit table of the
    /// module doc. A refusal sentence from preparing it is `Refused`.
    pub async fn submit(
        &self,
        call: &CallRequest,
        kind: &Kind,
        prepared: Result<HttpRequest, String>,
    ) -> Submitted {
        let request = match prepared {
            Ok(request) => request,
            Err(reason) => {
                return Submitted::Refused {
                    reason: self.gateway.reason(&reason),
                };
            }
        };
        match self.gateway.client.send(request).await {
            Ok(response) => {
                self.gateway.renew_if_asked(call, &response);
                self.submitted(call, kind, &response)
            }
            Err(error) => self.submit_failure(call, kind, &error),
        }
    }

    /// The submit table of the module doc for an answered POST.
    fn submitted(&self, call: &CallRequest, kind: &Kind, response: &HttpResponse) -> Submitted {
        let status = response.status;
        let code = super::error_code(&response.body);
        let uncertain = |text: &str| Submitted::Uncertain {
            reason: self.gateway.reason(&format!(
                "{} (Idempotency-Key {})",
                super::with_request_id(text.to_string(), response),
                self.gateway.idempotency_key(call)
            )),
        };
        let text = super::status_text(kind.submit_label, response, &self.gateway.client);
        match (status, code.as_deref()) {
            (200..=299, _) => match handle_of(&response.body, kind) {
                Ok(handle) => Submitted::Accepted { handle },
                Err(sentence) => uncertain(&sentence),
            },
            (300..=399, _) => uncertain(&format!(
                "{} was redirected (HTTP {status}); check {}",
                kind.submit_label,
                super::BASE_URL_VARIABLE
            )),
            (429, Some("key_budget_exceeded")) => Submitted::Failed {
                reason: self.gateway.reason(&text),
                cost: Some(Usd::ZERO),
                retryable: false,
            },
            (408 | 429, _) => Submitted::NotReceived {
                reason: self.gateway.reason(&text),
                retry_after: crate::adapter::retry_after(response),
            },
            (400..=499, _) => Submitted::Failed {
                reason: self.gateway.reason(&text),
                cost: wire::error_envelope(&response.body)
                    .is_some()
                    .then_some(Usd::ZERO),
                retryable: false,
            },
            _ => uncertain(&format!(
                "GG may have taken the {} job; it is not submitted again ({text})",
                kind.noun
            )),
        }
    }

    /// A failed submit exchange (spec/providers.md §4.2).
    fn submit_failure(&self, call: &CallRequest, kind: &Kind, error: &TransportError) -> Submitted {
        let label = kind.submit_label;
        match (error.kind, error.phase) {
            (TransportErrorKind::Refused, Phase::NotSent) => Submitted::Refused {
                reason: self
                    .gateway
                    .reason(&format!("{label} was not sent: {}", error.reason)),
            },
            (_, Phase::NotSent) => Submitted::not_received(
                self.gateway
                    .reason(&format!("{label} was not sent: {}", error.reason)),
            ),
            (_, Phase::AfterSend) => Submitted::Uncertain {
                reason: self.gateway.reason(&format!(
                    "{label} ended without an answer; it may have been taken: {} \
                     (Idempotency-Key {})",
                    error.reason,
                    self.gateway.idempotency_key(call)
                )),
            },
        }
    }

    /// Collects job `id`, the one a handle names (module doc); `None` when the handle is not one
    /// the adapter wrote. `contract` is the adapter's own reading of the call's route: a refusal
    /// there is `Unreachable`, with nothing requested.
    pub async fn collect(
        &self,
        id: Option<&str>,
        kind: &Kind,
        contract: Result<(), String>,
    ) -> Collected {
        let unreachable = |text: &str| Collected::Unreachable {
            reason: self.gateway.reason(text),
        };
        if let Err(refusal) = contract {
            return unreachable(&refusal);
        }
        let Some(id) = id else {
            return unreachable(&format!(
                "a GG {} job handle is not one this adapter wrote",
                kind.noun
            ));
        };
        let credential = match self.gateway.credential() {
            Ok(credential) => credential,
            Err(reason) => return unreachable(&reason),
        };
        let deadline = self.clock.now() + COLLECT_DEADLINE;
        let job = match self.await_result(&credential, id, kind, deadline).await {
            Ok(job) => job,
            Err(collected) => return collected,
        };
        let url = match result_file(&job, kind) {
            Ok(url) => url.to_string(),
            Err(sentence) => {
                return Collected::Ended {
                    reason: self.gateway.reason(&sentence),
                    cost: billing(&job),
                };
            }
        };
        let path = format!("jobs/{id}");
        let mut answer = match self.download(&url, deadline, billing(&job), kind).await {
            Ok(answer) => answer,
            Err(collected) => return collected,
        };
        answer.cost = self
            .settled(&credential, &path, id, deadline, &job, kind)
            .await;
        Collected::Answered(answer)
    }

    /// Reads job `id` every [`POLL`] until GG has its result document, which is returned; or the
    /// job ends without one (`Ended`, at its settled charge), or cannot be collected now
    /// (`Unreachable`, at `deadline` too).
    #[allow(clippy::result_large_err)] // `Collected` is what every caller returns at once
    pub async fn await_result(
        &self,
        credential: &Credential,
        id: &str,
        kind: &Kind,
        deadline: Duration,
    ) -> Result<Map<String, Value>, Collected> {
        let path = format!("jobs/{id}");
        loop {
            match self.read(credential, &path, id, deadline, kind).await {
                Read::Job(job) => match stage(&job, kind.job_label) {
                    Stage::Waiting => {}
                    Stage::Available => return Ok(job),
                    Stage::Ended(sentence) => {
                        return Err(Collected::Ended {
                            reason: self.gateway.reason(&sentence),
                            cost: billing(&job),
                        });
                    }
                    Stage::Unknown(sentence) => {
                        return Err(Collected::Unreachable {
                            reason: self.gateway.reason(&sentence),
                        });
                    }
                },
                Read::Nothing => {}
                Read::Stop(collected) => return Err(collected),
            }
            self.wait(deadline, kind).await?;
        }
    }

    /// Sends a free submit (a pre-check GG does not bill) and reads its job id: the free phase of
    /// spec/providers.md §7. A deterministic refusal (a 4xx other than 408 and 429, or a request
    /// the transport refuses itself) is `Refused`; anything else that is not a job, `NotReceived`,
    /// and the engine sends the whole submit again, under the same key, which GG answers with the
    /// same job.
    #[allow(clippy::result_large_err)] // `Submitted` is what every caller returns at once
    pub async fn submit_free(
        &self,
        call: &CallRequest,
        kind: &Kind,
        request: HttpRequest,
    ) -> Result<String, Submitted> {
        let label = kind.submit_label;
        let response = match self.gateway.client.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::Refused => {
                return Err(Submitted::Refused {
                    reason: self
                        .gateway
                        .reason(&format!("{label} was not sent: {}", error.reason)),
                });
            }
            Err(error) => {
                return Err(Submitted::not_received(
                    self.gateway
                        .reason(&format!("{label} failed: {}", error.reason)),
                ));
            }
        };
        self.gateway.renew_if_asked(call, &response);
        let text = super::status_text(label, &response, &self.gateway.client);
        match response.status {
            200..=299 => match handle_of(&response.body, kind) {
                Ok(handle) => Ok(job_id(&handle).unwrap_or_default().to_string()),
                Err(sentence) => Err(Submitted::not_received(self.gateway.reason(&sentence))),
            },
            408 | 429 => Err(Submitted::NotReceived {
                reason: self.gateway.reason(&text),
                retry_after: crate::adapter::retry_after(&response),
            }),
            400..=499 => Err(Submitted::Refused {
                reason: self.gateway.reason(&text),
            }),
            _ => Err(Submitted::not_received(self.gateway.reason(&text))),
        }
    }

    /// The job's charge, waiting up to [`SETTLE_WAIT`] (and never past `deadline`) for GG to settle
    /// it.
    async fn settled(
        &self,
        credential: &Credential,
        path: &str,
        id: &str,
        deadline: Duration,
        job: &Map<String, Value>,
        kind: &Kind,
    ) -> Option<Usd> {
        let mut charged = billing(job);
        let until = (self.clock.now() + SETTLE_WAIT).min(deadline);
        while charged.is_none() && self.clock.now() + POLL <= until {
            self.clock.sleep(POLL).await;
            if let Read::Job(job) = self.read(credential, path, id, deadline, kind).await {
                charged = billing(&job);
            }
        }
        charged
    }

    /// One authorized read of the job (module doc: failed reads are polls that saw nothing).
    async fn read(
        &self,
        credential: &Credential,
        path: &str,
        id: &str,
        deadline: Duration,
        kind: &Kind,
    ) -> Read {
        let client = &self.gateway.client;
        let label = kind.job_label;
        let remaining = deadline.saturating_sub(self.clock.now());
        if remaining.is_zero() {
            return Read::Stop(self.outstanding(kind));
        }
        let request = HttpRequest::new(Method::Get, client.url(path), Lane::Provider)
            .credential(credential.clone())
            .timeout(remaining.min(READ_DEADLINE))
            .max_response_bytes(JSON_RESPONSE_BYTES);
        let response = match client.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::Refused => {
                return Read::Stop(Collected::Unreachable {
                    reason: client.reason(&format!("{label} was not read: {}", error.reason)),
                });
            }
            Err(_) => return Read::Nothing,
        };
        let text = || client.reason(&super::status_text(label, &response, client));
        match response.status {
            200..=299 => match wire::json_object(&response.body, label) {
                Ok(job) if job.get("id").and_then(Value::as_str) == Some(id) => Read::Job(job),
                Ok(_) => Read::Stop(Collected::Unreachable {
                    reason: client.reason(&format!("{label} answered about another job")),
                }),
                Err(_) => Read::Nothing,
            },
            408 | 429 | 500..=599 => Read::Nothing,
            410 => Read::Stop(Collected::Ended {
                reason: text(),
                cost: None,
            }),
            _ => Read::Stop(Collected::Unreachable { reason: text() }),
        }
    }

    /// Sleeps one poll interval, unless that would pass the deadline.
    #[allow(clippy::result_large_err)] // `Collected` is what every caller returns at once
    async fn wait(&self, deadline: Duration, kind: &Kind) -> Result<(), Collected> {
        if self.clock.now() + POLL > deadline {
            return Err(self.outstanding(kind));
        }
        self.clock.sleep(POLL).await;
        Ok(())
    }

    fn outstanding(&self, kind: &Kind) -> Collected {
        Collected::Unreachable {
            reason: self.gateway.reason(kind.outstanding),
        }
    }

    /// The file's download, made once, and what its bytes become (module doc). A failure a later
    /// collect may not repeat is `Unreachable`; one it would repeat is `Ended` at `cost`.
    #[allow(clippy::result_large_err)] // `Collected` is what the caller returns at once
    async fn download(
        &self,
        url: &str,
        deadline: Duration,
        cost: Option<Usd>,
        kind: &Kind,
    ) -> Result<Answer, Collected> {
        let label = kind.download_label;
        let client = &self.gateway.client;
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
            return Err(self.outstanding(kind));
        }
        // The provider's text, byte for byte (spec/providers.md §8).
        let request = HttpRequest::new(Method::Get, url, Lane::Download)
            .timeout(remaining)
            .max_response_bytes(kind.max_bytes);
        let response = match client.send(request).await {
            Ok(response) => response,
            Err(error) if error.kind == TransportErrorKind::TooLarge => {
                return Err(ended(&format!("{label} failed: {}", error.reason)));
            }
            Err(error) if error.kind == TransportErrorKind::Refused => {
                return Err(unreachable(&format!(
                    "{label} was not sent: {}",
                    error.reason
                )));
            }
            Err(error) => {
                return Err(unreachable(&format!("{label} failed: {}", error.reason)));
            }
        };
        let status = wire::status_reason(label, response.status, None);
        match response.status {
            200..=299 => {}
            400..=499 if !matches!(response.status, 401 | 403 | 408 | 429) => {
                return Err(ended(&status));
            }
            _ => return Err(unreachable(&status)),
        }
        let bytes = response.body;
        if bytes.is_empty() {
            return Err(ended(&format!("{label} was empty")));
        }
        if bytes.len() as u64 > kind.max_bytes {
            return Err(ended(&format!(
                "{label} failed: the response is larger than {} bytes",
                kind.max_bytes
            )));
        }
        (kind.deliver)(bytes).map_err(|sentence| ended(&sentence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIND: Kind = Kind {
        noun: "thing",
        submit_label: "GG thing submission",
        job_label: "GG thing job",
        download_label: "GG output thing download",
        outstanding: "not yet",
        reference: |result| result.get("thing"),
        max_bytes: 16,
        deliver: |bytes| Ok(Answer::new(Value::Null, None).with_file("thing", "x/y", bytes)),
    };

    fn job(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn stages_follow_status_and_result_status() {
        let label = KIND.job_label;
        for (status, result_status) in [
            ("accepted", "pending"),
            ("running", "pending"),
            ("unknown", "pending"),
            ("succeeded", "pending"),
            ("failed", "pending"),
            ("running", "unavailable"),
            ("unknown", "expired"),
        ] {
            let job = job(json!({"status": status, "result_status": result_status}));
            assert_eq!(
                stage(&job, label),
                Stage::Waiting,
                "{status} {result_status}"
            );
        }
        let available = job(json!({"status": "succeeded", "result_status": "available",
                                  "result": {"thing": {"url": "https://h.example/t?s=1"}}}));
        assert_eq!(stage(&available, label), Stage::Available);
        assert_eq!(
            result_file(&available, &KIND),
            Ok("https://h.example/t?s=1")
        );
        let ended = job(json!({"status": "failed", "result_status": "unavailable",
                              "error": {"code": "upstream_rejected"}}));
        assert_eq!(
            stage(&ended, label),
            Stage::Ended(
                "GG thing job failed without a result (unavailable): code=upstream_rejected".into()
            )
        );
        assert_eq!(
            result_file(
                &job(json!({"result": {"thing": {"url": "http://h.example/t"}}})),
                &KIND
            ),
            Err("GG output thing url must be https".into())
        );
        assert_eq!(
            result_file(&job(json!({"result": {}})), &KIND),
            Err("GG thing job result carries no thing".into())
        );
        assert!(matches!(
            stage(
                &job(json!({"status": "cancelled", "result_status": "pending"})),
                label
            ),
            Stage::Unknown(_)
        ));
        assert!(matches!(
            stage(
                &job(json!({"status": "running", "result_status": "partial"})),
                label
            ),
            Stage::Unknown(_)
        ));
    }

    #[test]
    fn handles_hold_a_job_id_only() {
        assert_eq!(
            handle_of(br#"{"id": "req_Fb-1.x", "object": "job"}"#, &KIND),
            Ok(json!({"job_id": "req_Fb-1.x"}))
        );
        for bad in [
            &br#"{"id": "a/b"}"#[..],
            br#"{"id": ""}"#,
            br#"{"id": ".."}"#,
            br#"{"object": "job"}"#,
            b"<html>",
        ] {
            assert_eq!(
                handle_of(bad, &KIND).unwrap_err(),
                "GG took the thing job but returned no job to collect it by"
            );
        }
        assert_eq!(job_id(&json!({"job_id": "req_1"})), Some("req_1"));
        assert_eq!(job_id(&json!({"job_id": "req_1", "url": "x"})), None);
        assert_eq!(job_id(&json!({"job_id": "../x"})), None);
        assert_eq!(job_id(&json!({"job_id": ".."})), None);
        assert_eq!(job_id(&json!({"job_id": "."})), None);
        assert_eq!(job_id(&json!("req_1")), None);
    }

    #[test]
    fn billing_reads_the_settled_charge_only() {
        let settled = job(
            json!({"billing": {"status": "settled", "authorized_mills": "6200",
                                            "charged_mills": "100.5"}}),
        );
        assert_eq!(billing(&settled), Some(Usd(100_500)));
        let pending = job(
            json!({"billing": {"status": "pending", "authorized_mills": "6200",
                                            "charged_mills": "1"}}),
        );
        assert_eq!(billing(&pending), None, "an authorization is not a charge");
        let zero = job(json!({"billing": {"status": "settled", "charged_mills": "0"}}));
        assert_eq!(billing(&zero), Some(Usd::ZERO));
        let unreadable = job(json!({"billing": {"status": "settled", "charged_mills": 100}}));
        assert_eq!(billing(&unreadable), None);
        assert_eq!(billing(&job(json!({}))), None);
    }
}
