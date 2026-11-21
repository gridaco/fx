//! Durable waiting-step requests. Responders and the scheduler share a short-lived request lock,
//! independent of the invocation's run.lock. Only the scheduler writes invocation events.

use crate::store::Store;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use grida_fx_core::val::{Collection, Val, Verdict};
use grida_fx_core::value::{canon, check_markers, digest, is_digest, parse_json};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub const REQUEST_DIRECTORY: &str = "awaiting";
pub const REQUEST_LOCK: &str = "awaiting.lock";
const RECORD_KIND: &str = "fx-await-request-v1";
const MAX_SPEC_BYTES: usize = 1024 * 1024;
const MAX_ANSWER_BYTES: usize = 1024 * 1024;
const MAX_RECORD_BYTES: usize = 3 * 1024 * 1024;
const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;
const MAX_START_BYTES: usize = 4 * 1024 * 1024;

/// Encode a reviewed value without losing nested typed files or collection verdicts.
pub fn encode_reviewed(value: &Val) -> Value {
    match value {
        Val::Object(items) if items.values().any(has_typed_member) => {
            let fields: Map<String, Value> = items
                .iter()
                .map(|(key, value)| (key.clone(), encode_reviewed(value)))
                .collect();
            json!({"object": fields})
        }
        Val::List(items) => json!({"list": items.iter().map(encode_reviewed).collect::<Vec<_>>()}),
        Val::Collection(collection) => {
            let items: Vec<Value> = collection
                .items
                .iter()
                .map(|(key, value)| json!([key, encode_reviewed(value)]))
                .collect();
            let mut encoded = json!({"collection": items});
            if !collection.verdicts.is_empty() {
                let verdicts: Map<String, Value> = collection
                    .verdicts
                    .iter()
                    .map(|(key, verdict)| {
                        let value = match verdict {
                            Some(Verdict::Accept) => json!("accept"),
                            Some(Verdict::Reject) => json!("reject"),
                            Some(Verdict::Failed) => json!("failed"),
                            None => Value::Null,
                        };
                        (key.clone(), value)
                    })
                    .collect();
                encoded["verdicts"] = Value::Object(verdicts);
            }
            encoded
        }
        Val::Null => json!({"value": null}),
        _ => crate::events::encode(value),
    }
}

fn has_typed_member(value: &Val) -> bool {
    match value {
        Val::Null | Val::Bool(_) | Val::Number(_) | Val::Str(_) => false,
        Val::Object(value) => value.values().any(has_typed_member),
        Val::List(value) => value.iter().any(has_typed_member),
        _ => true,
    }
}

/// Restore a frozen candidate. A file is bound only through the content-addressed store.
pub fn decode_reviewed(value: &Value, store: &Store) -> Result<Val, String> {
    let invalid = || "The request contains an invalid reviewed value.".to_string();
    if let Some(items) = value.get("collection") {
        let mut collection = Collection::default();
        for item in items.as_array().ok_or_else(invalid)? {
            let Some([Value::String(key), encoded]) = item.as_array().map(Vec::as_slice) else {
                return Err(invalid());
            };
            collection
                .items
                .push((key.clone(), decode_reviewed(encoded, store)?));
        }
        if let Some(verdicts) = value.get("verdicts") {
            for (key, value) in verdicts.as_object().ok_or_else(invalid)? {
                let verdict = match value {
                    Value::Null => None,
                    Value::String(value) if value == "accept" => Some(Verdict::Accept),
                    Value::String(value) if value == "reject" => Some(Verdict::Reject),
                    Value::String(value) if value == "failed" => Some(Verdict::Failed),
                    _ => return Err(invalid()),
                };
                collection.verdicts.insert(key.clone(), verdict);
            }
        }
        return Ok(Val::Collection(Box::new(collection)));
    }
    if let Some(items) = value.get("list") {
        return items
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|item| decode_reviewed(item, store))
            .collect::<Result<_, _>>()
            .map(Val::List);
    }
    if let Some(items) = value.get("object") {
        return items
            .as_object()
            .ok_or_else(invalid)?
            .iter()
            .map(|(name, item)| Ok((name.clone(), decode_reviewed(item, store)?)))
            .collect::<Result<_, _>>()
            .map(Val::Object);
    }
    if value.get("none") == Some(&Value::Bool(true)) {
        return Ok(Val::Missing);
    }
    crate::events::decode(value, store)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutPolicy {
    #[default]
    Fail,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseChannel {
    Cli,
    Sdk,
    Viewer,
}

/// Evaluated review inputs, frozen when the concrete instance first opens this request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestSpec {
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub schema: Value,
    pub show: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<IndexMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    pub on_timeout: TimeoutPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum RequestState {
    Pending,
    Answered {
        answer: Value,
        channel: ResponseChannel,
        at: String,
    },
    Expired {
        at: String,
    },
}

impl RequestState {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Answered { .. } => "answered",
            Self::Expired { .. } => "expired",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub spec: RequestSpec,
    pub opened_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    pub state: RequestState,
}

impl Request {
    /// Resolve the selected key against the original, frozen candidates.
    pub fn selected(&self, answer: &Value) -> Option<&Value> {
        self.spec
            .choices
            .as_ref()?
            .get(choice_key(&self.spec, answer)?)
    }

    fn expire(&mut self, now: DateTime<Utc>) -> bool {
        if matches!(self.state, RequestState::Pending)
            && let Some(deadline) = self.deadline.as_deref().and_then(parse_time)
            && now >= deadline
        {
            self.state = RequestState::Expired {
                at: time_text(deadline),
            };
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub struct WaitingError {
    code: &'static str,
    message: &'static str,
    request: Option<Box<Request>>,
}

impl WaitingError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            request: None,
        }
    }

    fn with_request(mut self, request: Request) -> Self {
        self.request = Some(Box::new(request));
        self
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn request(&self) -> Option<&Request> {
        self.request.as_deref()
    }
}

impl fmt::Display for WaitingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for WaitingError {}

fn invalid_run() -> WaitingError {
    WaitingError::new(
        "invalid_run",
        "The selected run evidence is invalid or inaccessible.",
    )
}

fn invalid_record() -> WaitingError {
    WaitingError::new(
        "invalid_run",
        "The saved waiting request does not match this run.",
    )
}

fn storage_error() -> WaitingError {
    WaitingError::new(
        "storage_error",
        "The waiting request could not be read or recorded.",
    )
}

fn not_found() -> WaitingError {
    WaitingError::new("not_found", "No such request exists in the selected run.")
}

fn time_text(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(time: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(time).ok().map(|t| t.to_utc())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    kind: String,
    plan: String,
    run: String,
    request: Request,
}

struct Run {
    root: PathBuf,
    plan: String,
    id: String,
}

impl Run {
    fn open(path: &Path, write: bool) -> Result<Self, WaitingError> {
        let root = path.canonicalize().map_err(|_| invalid_run())?;
        if root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(crate::folder::is_tombstone_name)
        {
            return Err(invalid_run());
        }
        let directory = fs::symlink_metadata(&root).map_err(|_| invalid_run())?;
        if !directory.is_dir() || (write && !owned(&directory)) {
            return Err(invalid_run());
        }

        let (plan, plan_meta) =
            read_value(&root.join("plan.json"), MAX_PLAN_BYTES).map_err(|_| invalid_run())?;
        let plan = plan["plan"]
            .as_str()
            .filter(|hash| is_digest(hash))
            .ok_or_else(invalid_run)?;
        let file =
            open_file(&root.join("events.jsonl"), false, false).map_err(|_| invalid_run())?;
        if write && (!owned(&plan_meta) || !owned(&file.metadata().map_err(|_| invalid_run())?)) {
            return Err(invalid_run());
        }
        let mut line = Vec::new();
        let count = BufReader::new(file)
            .take((MAX_START_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|_| invalid_run())?;
        if count > MAX_START_BYTES || line.pop() != Some(b'\n') {
            return Err(invalid_run());
        }
        let start = std::str::from_utf8(&line)
            .ok()
            .and_then(|text| parse_json(text).ok())
            .ok_or_else(invalid_run)?;
        if start["kind"] != "fx-run-events-v1"
            || start["event"] != "run_started"
            || start["plan"] != plan
        {
            return Err(invalid_run());
        }
        let id = start["invocation_id"]
            .as_str()
            .filter(|id| {
                id.len() == 16
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(invalid_run)?;
        Ok(Self {
            root,
            plan: plan.to_owned(),
            id: id.to_owned(),
        })
    }

    fn request_id(&self, step: &str) -> String {
        digest(&json!(["fx-await-request-v1", self.plan, self.id, step]))
    }

    fn directory(&self) -> PathBuf {
        self.root.join(REQUEST_DIRECTORY)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory().join(format!("{id}.json"))
    }

    fn read(&self, id: &str) -> Result<Option<Record>, WaitingError> {
        if !request_directory(&self.root, false)? {
            return Ok(None);
        }
        let value = match read_value(&self.path(id), MAX_RECORD_BYTES) {
            Ok((value, meta)) if owned(&meta) => value,
            Ok(_) => return Err(invalid_record()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid_record()),
        };
        let record: Record = serde_json::from_value(value).map_err(|_| invalid_record())?;
        if record.kind != RECORD_KIND
            || record.plan != self.plan
            || record.run != self.id
            || record.request.id != id
            || self.request_id(&record.request.spec.step) != id
            || parse_time(&record.request.opened_at).is_none()
            || record
                .request
                .deadline
                .as_deref()
                .is_some_and(|time| parse_time(time).is_none())
        {
            return Err(invalid_record());
        }
        Ok(Some(record))
    }

    fn write(&self, record: &Record) -> Result<(), WaitingError> {
        if !request_directory(&self.root, true)? {
            return Err(invalid_record());
        }
        let value = serde_json::to_value(record).map_err(|_| storage_error())?;
        // Persist compact JSON so indentation cannot inflate a valid submission over
        // the record limit. The request settings and answer already have independent limits.
        let bytes = canon(&value);
        if bytes.len() > MAX_RECORD_BYTES || parse_json(&bytes).is_err() {
            return Err(WaitingError::new(
                "invalid_request",
                "The waiting request exceeds its storage size or nesting limit.",
            ));
        }
        crate::store::atomic_write(&self.path(&record.request.id), bytes.as_bytes(), false)
            .map_err(|_| storage_error())?;
        self.sync()
    }

    fn sync(&self) -> Result<(), WaitingError> {
        sync_directory(&self.directory())?;
        sync_directory(&self.root)
    }
}

/// Open a request once, or restore its exact saved review inputs and resolution.
pub fn ensure_request(run: &Path, spec: RequestSpec) -> Result<Request, WaitingError> {
    ensure_at(run, spec, Utc::now)
}

fn ensure_at(
    run: &Path,
    spec: RequestSpec,
    now: impl FnOnce() -> DateTime<Utc>,
) -> Result<Request, WaitingError> {
    let run = Run::open(run, true)?;
    let _lock = lock(&run.root, true)?.ok_or_else(storage_error)?;
    let id = run.request_id(&spec.step);
    if let Some(record) = run.read(&id)? {
        let saved = serde_json::to_value(&record.request.spec).map_err(|_| invalid_record())?;
        let current = serde_json::to_value(&spec).map_err(|_| invalid_record())?;
        if canon(&saved) != canon(&current) {
            return Err(WaitingError::new(
                "invalid_request",
                "The reviewed inputs changed since this request opened. Start a new run.",
            ));
        }
        return Ok(record.request);
    }
    let validator = request_validator(&spec)?;
    if let Some(choices) = &spec.choices {
        grida_fx_core::builtins::waiting::validate_choice_keys(
            &spec.schema,
            &validator,
            choices.keys().map(String::as_str),
            spec.choice_field.as_deref(),
        )
        .map_err(|message| WaitingError::new("invalid_request", message))?;
    }
    // Use the recorded opening precision for both the opening and the attached timeout.
    let opened_at =
        DateTime::from_timestamp_millis(now().timestamp_millis()).ok_or_else(storage_error)?;
    let deadline = spec
        .timeout_seconds
        .map(|seconds| {
            i64::try_from(seconds)
                .ok()
                .and_then(TimeDelta::try_seconds)
                .and_then(|duration| opened_at.checked_add_signed(duration))
                .map(time_text)
                .filter(|deadline| parse_time(deadline).is_some())
                .ok_or_else(invalid_timeout)
        })
        .transpose()?;
    create_request_directory(&run.root)?;
    let record = Record {
        kind: RECORD_KIND.into(),
        plan: run.plan.clone(),
        run: run.id.clone(),
        request: Request {
            id,
            spec,
            opened_at: time_text(opened_at),
            deadline,
            state: RequestState::Pending,
        },
    };
    run.write(&record)?;
    Ok(record.request)
}

fn invalid_timeout() -> WaitingError {
    WaitingError::new(
        "invalid_request",
        "The waiting-step timeout must be a positive, bounded duration.",
    )
}

fn request_validator(spec: &RequestSpec) -> Result<jsonschema::Validator, WaitingError> {
    if serde_json::to_vec(spec).map_or(true, |bytes| bytes.len() > MAX_SPEC_BYTES) {
        return Err(WaitingError::new(
            "invalid_request",
            "The waiting request exceeds its size limit.",
        ));
    }
    if spec.step.is_empty()
        || spec
            .message
            .as_ref()
            .is_some_and(|message| message.trim().is_empty())
    {
        return Err(WaitingError::new(
            "invalid_request",
            "A waiting step needs a concrete instance and any supplied message must be nonempty.",
        ));
    }
    if spec.timeout_seconds == Some(0) {
        return Err(invalid_timeout());
    }
    if spec.choices.as_ref().is_some_and(|choices| {
        choices.is_empty() || choices.keys().any(|key| key.trim().is_empty())
    }) || spec
        .choice_field
        .as_ref()
        .is_some_and(|field| field.trim().is_empty() || spec.choices.is_none())
    {
        return Err(WaitingError::new(
            "invalid_request",
            "Waiting-step choices must be nonempty, and a choice field requires choices.",
        ));
    }
    grida_fx_core::builtins::waiting::answer_validator(&spec.schema).map_err(|_| {
        WaitingError::new(
            "invalid_request",
            "The answer schema must be a valid draft 2020-12 JSON Schema with only internal references.",
        )
    })
}

fn choice_key<'a>(spec: &RequestSpec, answer: &'a Value) -> Option<&'a str> {
    match &spec.choice_field {
        Some(field) => answer.get(field)?.as_str(),
        None => answer.as_str(),
    }
}

fn check_id(id: &str) -> Result<(), WaitingError> {
    if !is_digest(id) {
        return Err(WaitingError::new(
            "invalid_request",
            "A waiting request ID must be 64 lowercase hexadecimal characters.",
        ));
    }
    Ok(())
}

/// List the selected run's stored requests. An elapsed deadline is reflected without a write.
pub fn list(run: &Path) -> Result<Vec<Request>, WaitingError> {
    // Older or purely planned records can have no request directory or run_started evidence.
    // Avoid turning those records into errors on a read-only viewer attachment.
    if !fs::metadata(run).is_ok_and(|meta| meta.is_dir()) {
        return Err(invalid_run());
    }
    let directory = match fs::symlink_metadata(run.join(REQUEST_DIRECTORY)) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(storage_error()),
    };
    let has_lock = fs::symlink_metadata(run.join(REQUEST_LOCK))
        .is_ok_and(|meta| meta.is_file() && meta.len() == 0 && owned(&meta));
    // A caller can point a run at an existing user directory. Without a request lock,
    // consult the run log before treating anything named "awaiting" as engine data.
    let recorded = if has_lock {
        None
    } else {
        let steps = recorded_wait_steps(run)?;
        if steps.is_empty() {
            return Ok(Vec::new());
        }
        Some(steps)
    };
    if !directory.is_dir() {
        return Err(invalid_record());
    }
    let run = Run::open(run, false)?;
    let mut requests = Vec::new();
    let now = Utc::now();
    if let Some(steps) = recorded {
        // A copied folder may omit its ephemeral lock. Inspect just the files that
        // correspond to the waiting step occurrences in this run's event history.
        for (step, published) in steps {
            match run.read(&run.request_id(&step))? {
                Some(record) => requests.push(record.request),
                None if published => return Err(invalid_record()),
                None => {}
            }
        }
    } else {
        for item in fs::read_dir(run.directory()).map_err(|_| storage_error())? {
            let item = item.map_err(|_| storage_error())?;
            let filename = item.file_name();
            if let Some(id) = filename
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                && is_digest(id)
            {
                requests.push(run.read(id)?.ok_or_else(invalid_record)?.request);
            }
        }
    }
    for request in &mut requests {
        request.expire(now);
    }
    requests.sort_by(|left, right| {
        left.opened_at
            .cmp(&right.opened_at)
            .then_with(|| left.spec.step.cmp(&right.spec.step))
    });
    Ok(requests)
}

/// Even when an invocation stops between publishing the request and emitting
/// `waiting_request`, its preceding `node_started` identifies that exact occurrence.
fn recorded_wait_steps(
    run: &Path,
) -> Result<std::collections::BTreeMap<String, bool>, WaitingError> {
    let mut steps = std::collections::BTreeMap::new();
    let file = match open_file(&run.join("events.jsonl"), false, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(steps),
        Err(_) => return Err(invalid_run()),
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        reader
            .read_until(b'\n', &mut line)
            .map_err(|_| invalid_run())?;
        if line.last() != Some(&b'\n') {
            break;
        }
        let Ok(line) = std::str::from_utf8(&line) else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = parse_json(line) else { break };
        if let Some((step, published)) = recorded_step(&event) {
            *steps.entry(step.to_owned()).or_default() |= published;
        }
    }
    Ok(steps)
}

/// A concrete waiting step in the event log, and whether the engine observed a saved request.
pub(crate) fn recorded_step(event: &Value) -> Option<(&str, bool)> {
    if event["kind"] != "fx-run-events-v1" {
        return None;
    }
    let published = match event["event"].as_str()? {
        "waiting_request" => true,
        "node_started"
            if event["uses"]
                .as_str()?
                .strip_prefix("fx/await@")?
                .trim_start_matches('0')
                == "1" =>
        {
            false
        }
        _ => return None,
    };
    Some((event["id"].as_str()?, published))
}

/// Read one exact request from an existing run without writing.
pub fn lookup(run: &Path, id: &str) -> Result<Request, WaitingError> {
    check_id(id)?;
    if !request_directory(run, false)? {
        return Err(not_found());
    }
    let mut request = Run::open(run, false)?
        .read(id)?
        .ok_or_else(not_found)?
        .request;
    request.expire(Utc::now());
    Ok(request)
}

/// An attached scheduler observes its open requests together. Verify the run and read its
/// plan once per poll, even when a repeated step has many pending occurrences.
pub(crate) fn any_resolved<'a>(
    run: &Path,
    ids: impl Iterator<Item = &'a str>,
) -> Result<bool, WaitingError> {
    let run = Run::open(run, false)?;
    let now = Utc::now();
    for id in ids {
        check_id(id)?;
        let mut request = run.read(id)?.ok_or_else(not_found)?.request;
        request.expire(now);
        if !matches!(request.state, RequestState::Pending) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Accept a valid answer. A conflicting response returns the already recorded resolution.
pub fn respond(
    run: &Path,
    id: &str,
    answer: Value,
    channel: ResponseChannel,
) -> Result<Request, WaitingError> {
    respond_at(run, id, answer, channel, Utc::now)
}

fn respond_at(
    run: &Path,
    id: &str,
    answer: Value,
    channel: ResponseChannel,
    now: impl FnOnce() -> DateTime<Utc>,
) -> Result<Request, WaitingError> {
    check_id(id)?;
    if crate::events::nesting(&answer) > crate::events::VALUE_DEPTH {
        return Err(WaitingError::new(
            "invalid_answer",
            "The answer is too deeply nested for the run's record.",
        ));
    }
    let text = serde_json::to_string(&answer).map_err(|_| invalid_answer_value())?;
    if text.len() > MAX_ANSWER_BYTES {
        return Err(WaitingError::new(
            "invalid_answer",
            "The answer exceeds its size limit.",
        ));
    }
    // Native callers can construct unnormalized serde_json numbers directly. Apply exactly
    // the same profile as CLI and viewer text, before schema validation and idempotency comparison.
    let answer = parse_json(&text).map_err(|_| invalid_answer_value())?;
    check_markers(&answer, "answer").map_err(|_| {
        WaitingError::new(
            "invalid_answer",
            "The answer contains a reserved FX value marker.",
        )
    })?;
    let run = Run::open(run, true)?;
    let _lock = lock(&run.root, false)?.ok_or_else(not_found)?;
    let mut record = run.read(id)?.ok_or_else(not_found)?;
    if let RequestState::Answered {
        answer: recorded, ..
    } = &record.request.state
    {
        run.sync()?;
        if canon(recorded) == canon(&answer) {
            return Ok(record.request);
        }
        return Err(WaitingError::new(
            "answer_conflict",
            "The request already has a different answer.",
        )
        .with_request(record.request));
    }
    let now = now();
    if record.request.expire(now) {
        run.write(&record)?;
    }
    if matches!(record.request.state, RequestState::Expired { .. }) {
        return Err(
            WaitingError::new("expired", "The waiting request has expired.")
                .with_request(record.request),
        );
    }
    let validator = request_validator(&record.request.spec)?;
    if !validator.is_valid(&answer) {
        return Err(WaitingError::new(
            "invalid_answer",
            "The answer does not match the request's JSON Schema.",
        ));
    }
    if record.request.spec.choices.is_some() && record.request.selected(&answer).is_none() {
        return Err(WaitingError::new(
            "invalid_answer",
            "The answer does not identify one of the frozen choices.",
        ));
    }
    record.request.state = RequestState::Answered {
        answer,
        channel,
        at: time_text(now),
    };
    run.write(&record)?;
    Ok(record.request)
}

fn invalid_answer_value() -> WaitingError {
    WaitingError::new(
        "invalid_answer",
        "The answer is outside the FX JSON value profile.",
    )
}

/// Close an elapsed response window under the same lock that accepts answers.
pub fn expire_due(run: &Path, id: &str) -> Result<Request, WaitingError> {
    expire_at(run, id, Utc::now)
}

fn expire_at(
    run: &Path,
    id: &str,
    now: impl FnOnce() -> DateTime<Utc>,
) -> Result<Request, WaitingError> {
    check_id(id)?;
    let run = Run::open(run, true)?;
    let _lock = lock(&run.root, false)?.ok_or_else(not_found)?;
    let mut record = run.read(id)?.ok_or_else(not_found)?;
    if record.request.expire(now()) {
        run.write(&record)?;
    }
    Ok(record.request)
}

/// A remover first acquires run.lock, then this lock, before moving or removing the folder.
pub fn lock_for_removal(run: &Path) -> Result<Option<File>, WaitingError> {
    lock(run, false)
}

pub(crate) fn sweep(run: &Path) {
    let Ok(Some(_lock)) = lock(run, false) else {
        return;
    };
    if matches!(request_directory(run, true), Ok(true))
        && let Ok(entries) = fs::read_dir(run.join(REQUEST_DIRECTORY))
    {
        for entry in entries.flatten() {
            if crate::folder::is_temporary_name(&entry.file_name().to_string_lossy()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn lock(root: &Path, create: bool) -> Result<Option<File>, WaitingError> {
    let path = root.join(REQUEST_LOCK);
    let file = match open_file(&path, true, create) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(_) => return Err(storage_error()),
    };
    let metadata = file.metadata().map_err(|_| storage_error())?;
    if !owned(&metadata) || metadata.len() != 0 {
        return Err(invalid_run());
    }
    file.lock().map_err(|_| storage_error())?;
    if !crate::folder::same_file(&file, &path) {
        return Err(invalid_run());
    }
    Ok(Some(file))
}

fn request_directory(root: &Path, write: bool) -> Result<bool, WaitingError> {
    match fs::symlink_metadata(root.join(REQUEST_DIRECTORY)) {
        Ok(metadata) if metadata.is_dir() && (!write || owned(&metadata)) => Ok(true),
        Ok(_) => Err(invalid_record()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(storage_error()),
    }
}

fn create_request_directory(root: &Path) -> Result<(), WaitingError> {
    if !request_directory(root, true)? {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(root.join(REQUEST_DIRECTORY))
            .map_err(|_| storage_error())?;
        sync_directory(root)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), WaitingError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| storage_error())
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<(), WaitingError> {
    Ok(())
}

#[cfg(unix)]
fn owned(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o022 == 0
}

#[cfg(not(unix))]
fn owned(_: &fs::Metadata) -> bool {
    true
}

fn read_value(path: &Path, limit: usize) -> io::Result<(Value, fs::Metadata)> {
    let file = open_file(path, false, false)?;
    let metadata = file.metadata()?;
    let mut text = String::new();
    file.take((limit + 1) as u64).read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record exceeds its limit",
        ));
    }
    let value = parse_json(&text)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid JSON record"))?;
    Ok((value, metadata))
}

fn open_file(path: &Path, write: bool, create: bool) -> io::Result<File> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        let mut flags = OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        flags |= if write { OFlags::RDWR } else { OFlags::RDONLY };
        if create {
            flags |= OFlags::CREATE;
        }
        File::from(open(path, flags, Mode::RUSR | Mode::WUSR)?)
    };
    #[cfg(not(unix))]
    let file = {
        if let Ok(meta) = fs::symlink_metadata(path)
            && !meta.is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "record is not regular",
            ));
        }
        fs::OpenOptions::new()
            .read(true)
            .write(write)
            .create(create)
            .truncate(false)
            .open(path)?
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "record is not regular",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::value::write_json;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::TempDir;

    fn run(start: &str) -> TempDir {
        let root = tempfile::tempdir().unwrap();
        let plan = "a".repeat(64);
        fs::write(
            root.path().join("plan.json"),
            write_json(&json!({"kind": "fx-graph-v2", "plan": plan})),
        )
        .unwrap();
        fs::write(
            root.path().join("events.jsonl"),
            format!(
                "{}\n",
                canon(&json!({
                    "kind": "fx-run-events-v1",
                    "event": "run_started",
                    "plan": plan,
                    "invocation_id": start
                }))
            ),
        )
        .unwrap();
        root
    }

    fn now() -> DateTime<Utc> {
        parse_time("2026-01-01T00:00:00Z").unwrap()
    }

    fn reviewed() -> RequestSpec {
        RequestSpec {
            step: "review#1".into(),
            message: Some("Choose a variant.".into()),
            schema: json!({
                "type": "object", "required": ["variant"],
                "properties": {"variant": {"type": "string"}},
                "additionalProperties": false
            }),
            show: json!({"none": true}),
            choices: Some(IndexMap::from([
                ("a".into(), json!({"value": "first"})),
                ("b".into(), json!({"value": "second"})),
            ])),
            choice_field: Some("variant".into()),
            timeout_seconds: Some(10),
            on_timeout: TimeoutPolicy::Continue,
        }
    }

    #[test]
    fn same_run_replays_frozen_choices_and_fresh_occurrences_get_distinct_ids() {
        let original = run("0123456789abcdef");
        let fresh = run("fedcba9876543210");
        let first = ensure_at(original.path(), reviewed(), now).unwrap();
        let replayed =
            ensure_at(original.path(), reviewed(), || now() + TimeDelta::hours(1)).unwrap();
        assert_eq!(
            (replayed.id, replayed.deadline),
            (first.id.clone(), first.deadline)
        );
        let another_instance = ensure_at(
            original.path(),
            RequestSpec {
                step: "review#2".into(),
                ..reviewed()
            },
            now,
        )
        .unwrap();
        assert_ne!(another_instance.id, first.id);
        assert_ne!(
            ensure_at(fresh.path(), reviewed(), now).unwrap().id,
            first.id
        );
        assert_eq!(
            ensure_at(
                original.path(),
                RequestSpec {
                    show: json!({"value": "different"}),
                    ..reviewed()
                },
                now,
            )
            .unwrap_err()
            .code(),
            "invalid_request"
        );
    }

    #[test]
    fn selected_objects_restore_typed_files_inside_keyed_collections() {
        use grida_fx_core::val::FileValue;

        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path());
        let recorded = store.put_bytes(b"example").unwrap();
        let file = Val::File(Box::new(FileValue {
            digest: recorded.digest,
            size: recorded.size,
            kind: "text/plain".into(),
            name: "sample.txt".into(),
            key: None,
            content: None,
            location: None,
        }));
        let collection = Val::Collection(Box::new(Collection {
            items: vec![("draft".into(), file)],
            verdicts: IndexMap::from([("draft".into(), Some(Verdict::Accept))]),
        }));
        let value = Val::Object(IndexMap::from([
            ("files".into(), Val::List(vec![collection])),
            ("notes".into(), Val::Null),
        ]));
        let encoded = encode_reviewed(&value);
        assert!(encoded["object"]["files"]["list"][0]["collection"][0][1]["file"].is_object());
        assert_eq!(decode_reviewed(&encoded, &store).unwrap(), value);
    }

    #[test]
    fn two_clients_converge_and_invalid_choices_do_not_claim_the_request() {
        let folder = Arc::new(run("0123456789abcdef"));
        let request = ensure_at(
            folder.path(),
            RequestSpec {
                timeout_seconds: None,
                ..reviewed()
            },
            now,
        )
        .unwrap();
        for answer in [json!({"variant": "unknown"}), json!(false)] {
            assert_eq!(
                respond(folder.path(), &request.id, answer, ResponseChannel::Cli)
                    .unwrap_err()
                    .code(),
                "invalid_answer"
            );
        }
        assert!(matches!(
            lookup(folder.path(), &request.id).unwrap().state,
            RequestState::Pending
        ));

        let barrier = Arc::new(Barrier::new(2));
        let clients = [("a", ResponseChannel::Cli), ("b", ResponseChannel::Viewer)].map(
            |(choice, channel)| {
                let folder = Arc::clone(&folder);
                let barrier = Arc::clone(&barrier);
                let id = request.id.clone();
                thread::spawn(move || {
                    barrier.wait();
                    respond(folder.path(), &id, json!({"variant": choice}), channel)
                })
            },
        );
        let results = clients.map(|client| client.join().unwrap());
        let winner = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .unwrap();
        let disagreement = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .unwrap();
        assert_eq!(disagreement.code(), "answer_conflict");
        assert_eq!(
            serde_json::to_value(disagreement.request().unwrap()).unwrap(),
            serde_json::to_value(winner).unwrap()
        );
        let RequestState::Answered { answer, .. } = &winner.state else {
            panic!("winner was not recorded")
        };
        assert!(winner.selected(answer).is_some());
        let retry = respond(
            folder.path(),
            &request.id,
            answer.clone(),
            ResponseChannel::Sdk,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(retry).unwrap(),
            serde_json::to_value(winner).unwrap()
        );
    }

    #[test]
    fn elapsed_request_rejects_a_first_answer_but_identical_accepted_null_can_retry() {
        let folder = run("0123456789abcdef");
        let elapsed = ensure_at(folder.path(), reviewed(), now).unwrap();
        let timeout = respond_at(
            folder.path(),
            &elapsed.id,
            json!({"variant": "a"}),
            ResponseChannel::Cli,
            || now() + TimeDelta::seconds(10),
        )
        .unwrap_err();
        assert_eq!(timeout.code(), "expired");
        assert!(matches!(
            timeout.request().unwrap().state,
            RequestState::Expired { .. }
        ));
        let nullable = ensure_at(
            folder.path(),
            RequestSpec {
                step: "affirm#1".into(),
                schema: json!({"type": ["boolean", "null"]}),
                choices: None,
                choice_field: None,
                ..reviewed()
            },
            now,
        )
        .unwrap();
        let accepted = respond_at(
            folder.path(),
            &nullable.id,
            Value::Null,
            ResponseChannel::Cli,
            || now() + TimeDelta::seconds(9),
        )
        .unwrap();
        assert!(matches!(
            accepted.state,
            RequestState::Answered {
                answer: Value::Null,
                ..
            }
        ));
        let retry = respond_at(
            folder.path(),
            &nullable.id,
            Value::Null,
            ResponseChannel::Sdk,
            || now() + TimeDelta::seconds(11),
        )
        .unwrap();
        assert!(matches!(
            retry.state,
            RequestState::Answered {
                channel: ResponseChannel::Cli,
                ..
            }
        ));
    }

    #[test]
    fn expiry_and_a_response_at_the_deadline_agree_on_one_resolution() {
        let folder = Arc::new(run("0123456789abcdef"));
        let request = ensure_at(folder.path(), reviewed(), now).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let timer = {
            let folder = Arc::clone(&folder);
            let barrier = Arc::clone(&barrier);
            let id = request.id.clone();
            thread::spawn(move || {
                barrier.wait();
                expire_at(folder.path(), &id, || now() + TimeDelta::seconds(10)).unwrap()
            })
        };
        barrier.wait();
        let late = respond_at(
            folder.path(),
            &request.id,
            json!({"variant": "a"}),
            ResponseChannel::Viewer,
            || now() + TimeDelta::seconds(10),
        )
        .unwrap_err();
        assert_eq!(late.code(), "expired");
        let timer = timer.join().unwrap();
        assert_eq!(
            serde_json::to_value(late.request().unwrap()).unwrap(),
            serde_json::to_value(timer).unwrap()
        );
    }

    #[test]
    fn timer_and_acceptance_share_the_recorded_millisecond_boundary() {
        let folder = run("0123456789abcdef");
        let request = ensure_at(folder.path(), reviewed(), || {
            now() + TimeDelta::nanoseconds(999_999)
        })
        .unwrap();
        assert_eq!(request.opened_at, "2026-01-01T00:00:00.000Z");
        assert_eq!(
            request.deadline.as_deref(),
            Some("2026-01-01T00:00:10.000Z")
        );
        let accepted = respond_at(
            folder.path(),
            &request.id,
            json!({"variant":"a"}),
            ResponseChannel::Cli,
            || now() + TimeDelta::seconds(10) - TimeDelta::nanoseconds(1),
        )
        .unwrap();
        assert!(
            matches!(accepted.state, RequestState::Answered { at, .. } if at == "2026-01-01T00:00:09.999Z")
        );
    }

    #[test]
    fn a_deadline_outside_the_wire_calendar_is_refused_before_publication() {
        let folder = run("0123456789abcdef");
        let error = ensure_at(
            folder.path(),
            RequestSpec {
                timeout_seconds: Some(350_000_000_000),
                ..reviewed()
            },
            now,
        )
        .unwrap_err();
        assert_eq!(error.code(), "invalid_request");
        assert!(list(folder.path()).unwrap().is_empty());
    }

    #[test]
    fn equivalent_json_from_different_clients_retries_the_original_answer() {
        let folder = run("0123456789abcdef");
        let request = ensure_at(
            folder.path(),
            RequestSpec {
                schema: json!(true),
                choices: None,
                choice_field: None,
                timeout_seconds: None,
                ..reviewed()
            },
            now,
        )
        .unwrap();
        assert_eq!(
            respond(
                folder.path(),
                &request.id,
                json!(9_007_199_254_740_993_i64),
                ResponseChannel::Sdk
            )
            .unwrap_err()
            .code(),
            "invalid_answer"
        );
        let first = respond(
            folder.path(),
            &request.id,
            json!({"a":1.0,"b":-0.0}),
            ResponseChannel::Viewer,
        )
        .unwrap();
        let retry = respond(
            folder.path(),
            &request.id,
            json!({"b":0,"a":1}),
            ResponseChannel::Cli,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(retry).unwrap()
        );
    }

    #[test]
    fn a_removal_tombstone_cannot_be_answered_even_if_its_records_still_exist() {
        let parent = tempfile::tempdir().unwrap();
        let folder = run("0123456789abcdef");
        let request = ensure_at(folder.path(), reviewed(), now).unwrap();
        let tombstone = parent.path().join(".removing-0123456789abcdef");
        fs::rename(folder.path(), &tombstone).unwrap();
        assert!(tombstone.join("plan.json").is_file());
        assert_eq!(
            respond(
                &tombstone,
                &request.id,
                json!({"variant":"a"}),
                ResponseChannel::Cli
            )
            .unwrap_err()
            .code(),
            "invalid_run"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cannot_answer_through_a_replaced_request_path() {
        use std::os::unix::fs::symlink;

        let folder = run("0123456789abcdef");
        let request = ensure_at(folder.path(), reviewed(), now).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), b"outside").unwrap();
        let path = folder
            .path()
            .join(REQUEST_DIRECTORY)
            .join(format!("{}.json", request.id));
        fs::remove_file(&path).unwrap();
        symlink(outside.path(), &path).unwrap();
        for id in [&request.id, "../../outside"] {
            assert!(respond(folder.path(), id, Value::Null, ResponseChannel::Cli).is_err());
        }
        assert_eq!(fs::read(outside.path()).unwrap(), b"outside");
    }

    #[test]
    fn a_schema_is_local_and_validated_before_publication() {
        let folder = run("0123456789abcdef");
        let fault = ensure_at(
            folder.path(),
            RequestSpec {
                schema: json!({"$ref": "https://example.invalid/schema"}),
                ..reviewed()
            },
            now,
        )
        .unwrap_err();
        assert_eq!(fault.code(), "invalid_request");
        assert!(list(folder.path()).unwrap().is_empty());
        assert!(
            ensure_at(
                folder.path(),
                RequestSpec {
                    schema: json!({"$defs": {"text": {"type": "string"}}, "$ref": "#/$defs/text"}),
                    choice_field: None,
                    ..reviewed()
                },
                now
            )
            .is_ok()
        );
    }

    #[test]
    fn choices_must_allow_an_answer_before_the_request_opens() {
        let folder = run("0123456789abcdef");
        for (schema, field) in [
            (json!({"type": "boolean"}), None),
            (json!({"type": "string"}), Some("variant")),
            (
                json!({"properties": {"variant": {"enum": ["absent"]}}}),
                Some("variant"),
            ),
        ] {
            let error = ensure_at(
                folder.path(),
                RequestSpec {
                    schema,
                    choice_field: field.map(str::to_owned),
                    ..reviewed()
                },
                now,
            )
            .unwrap_err();
            assert_eq!(error.code(), "invalid_request");
        }
        assert!(list(folder.path()).unwrap().is_empty());

        // A sibling supplied by the responder selects the branch that admits the key.
        let request = ensure_at(
            folder.path(),
            RequestSpec {
                message: None,
                schema: json!({
                    "required": ["mode"],
                    "if": {"required": ["mode"]},
                    "then": {"properties": {"variant": {"enum": ["a"]}}},
                    "else": {"properties": {"variant": {"type": "boolean"}}}
                }),
                ..reviewed()
            },
            now,
        )
        .unwrap();
        assert!(json!(request).get("message").is_none());
        assert_eq!(
            respond_at(
                folder.path(),
                &request.id,
                json!({"mode": "print", "variant": "b"}),
                ResponseChannel::Sdk,
                now
            )
            .unwrap_err()
            .code(),
            "invalid_answer"
        );
        respond_at(
            folder.path(),
            &request.id,
            json!({"mode": "print", "variant": "a"}),
            ResponseChannel::Sdk,
            now,
        )
        .unwrap();
    }

    #[test]
    fn answers_preserve_the_workflow_value_domain_and_can_fill_the_record() {
        let folder = run("0123456789abcdef");
        let request = ensure_at(
            folder.path(),
            RequestSpec {
                schema: Value::Bool(true),
                choices: None,
                choice_field: None,
                timeout_seconds: None,
                ..reviewed()
            },
            now,
        )
        .unwrap();
        let nest = |mut value, levels| {
            for _ in 0..levels {
                value = Value::Array(vec![value]);
            }
            value
        };
        for invalid in [
            json!({"items":[{"missing": true}]}),
            nest(Value::Null, crate::events::VALUE_DEPTH + 1),
        ] {
            let error =
                respond(folder.path(), &request.id, invalid, ResponseChannel::Sdk).unwrap_err();
            assert_eq!(error.code(), "invalid_answer");
            assert!(matches!(
                lookup(folder.path(), &request.id).unwrap().state,
                RequestState::Pending
            ));
        }
        // Its compact JSON fits the input limit. Indenting each element at this depth
        // used to exceed the 3 MiB record limit during publication.
        let answer = nest(Value::Array(vec![json!(0); 32_000]), 100);
        respond(
            folder.path(),
            &request.id,
            answer.clone(),
            ResponseChannel::Sdk,
        )
        .unwrap();
        assert!(matches!(lookup(folder.path(), &request.id).unwrap().state,
            RequestState::Answered { answer: stored, .. } if stored == answer));
    }

    #[test]
    fn inspection_distinguishes_user_files_from_a_copied_waiting_step() {
        use std::io::Write;
        let root = run("0123456789abcdef");
        let directory = root.path().join(REQUEST_DIRECTORY);
        fs::write(&directory, "user file").unwrap();
        assert!(list(root.path()).unwrap().is_empty());
        fs::remove_file(&directory).unwrap();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("user.json"), "unrelated").unwrap();
        fs::write(
            directory.join(format!("{}.json", "a".repeat(64))),
            "unrelated",
        )
        .unwrap();
        assert!(list(root.path()).unwrap().is_empty());
        let request = ensure_at(root.path(), reviewed(), now).unwrap();
        fs::remove_file(root.path().join(REQUEST_LOCK)).unwrap();
        let mut events = fs::OpenOptions::new()
            .append(true)
            .open(root.path().join("events.jsonl"))
            .unwrap();
        write!(events, "{}", json!({"kind":"fx-run-events-v1","event":"node_started","id":request.spec.step,"uses":"fx/await@01"})).unwrap();
        assert!(
            list(root.path()).unwrap().is_empty(),
            "a torn event is invisible"
        );
        writeln!(events).unwrap();
        assert_eq!(list(root.path()).unwrap()[0].id, request.id);
        writeln!(events, "{}", json!({"kind":"fx-run-events-v1","event":"waiting_request","id":request.spec.step,"request_id":request.id})).unwrap();
        fs::write(directory.join(format!("{}.json", request.id)), "damaged").unwrap();
        assert_eq!(list(root.path()).unwrap_err().code(), "invalid_run");
    }
}
