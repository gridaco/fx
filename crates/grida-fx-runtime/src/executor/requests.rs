//! Serving one run's host requests (spec/protocol.md §6): the [`RunHandler`] a leased host's
//! router hands every request whose `run_id` is this run's.
//!
//! | method | answer |
//! |---|---|
//! | `capability` | `calls::call` with the job's call site, the run's counter and files; the result `{key, cached, cost_usd, files: {name: file ref}, data}` (refs staged with facts, put in the run's files) |
//! | `agent.run` | `agent::run_agent_into` with this handler's turns (`agent.turn` through `calls::call`) and the leased host's connection for `tool.invoke` and `agent.check` (both carry `run_id` and `agent_id`); images and tool pictures as file values the run was handed; an error that ends a loop that had started carries the transcript so far in `data.transcript` |
//! | `fact` | `{}`; refused with `-32602` for `cost_usd`, a reserved marker, or a value nested deeper than `events::VALUE_DEPTH` (which the run's record could not hold); recorded at once, in order |
//! | `annotate` | `{}`; refused with `-32602` for a malformed mark (a shape without its geometry, coordinates outside 0..1, `points` with fewer than 2 points), a reserved marker, or a mark nested deeper than `events::VALUE_DEPTH`; recorded at once |
//! | `progress` (notification) | shown on stderr as `<instance id>: <text>[ (<n>%)]` |
//! | `prompt.render` | `path` must be one of the type's resources exactly (`undeclared_resource`); the file is decoded (`text::decode_text`), its comments removed (`text::prompt_text`), and rendered (`grida_fx_core::expr`) over the run's params with `variables` on top (file values become files the run was handed); an unknown name or a bad template is `expression_error` |
//! | `file.put` | exactly one of `work_path` (inside the work dir, an existing file: else `outside_work_dir`), `base64` (RFC 4648 with padding: else `-32602`) or `json` (no reserved marker; written as spec/identity.md §5 "Writing JSON"), else `-32602` `file.put takes exactly one of work_path, base64, json`; `kind` defaults to the suffix rule, `json`, `file`; the result is the stored file's ref |
//!
//! Every request names this run (`run_id`, else `-32602`); a method the engine does not serve is
//! `-32601`. Params FX cannot read are `-32602` `<method>: <why>`, the why a sentence
//! (`host::read_reason`), never the reader's own text. After the run has ended ([`RunHandler::close`]) or was cancelled, requests are
//! answered `cancelled`, while a paid call already in flight still completes and settles: each
//! `capability` request, and each agent turn, runs its call on a task of its own, and the run's
//! cancellation answers the host without waiting for it. The call counts as running from before
//! its task is spawned (`Services::track_call`), so the invocation waits for it before it ends
//! (`Services::calls_settled`).
//!
//! Smaller rules:
//! - `fact` refuses an empty name; a fact reported again keeps its first place with the later
//!   value.
//! - the percentage of `progress` is `fraction × 100` rounded to a whole number.
//! - `prompt.render` sees the type's params (the instance's with-values that are params, files
//!   as files) and `variables` on top; a text or JSON file renders as its content. A resource that
//!   cannot be read is `internal`; one that is not UTF-8 text is `expression_error`.
//! - Faults: a request that meets a fault of the engine's own (a store that cannot be read or
//!   written, an unreadable job record, a paid call's task that ended without an answer) is
//!   answered `internal`, and the first such fault is kept
//!   ([`RunHandler::fault`]): the attempt then stops the run, even when the body caught the
//!   error. Other `internal` answers (a resource that cannot be read, a host that answered
//!   `tool.invoke` or `agent.check` with something FX cannot read) fail only the request.
//! - `file.put` names the stored file by the work path's last segment, else `file`; an empty
//!   `kind` is refused with `-32602`. A work file that cannot be read, or that changes while it
//!   is stored, is the body's doing, not a fault: the request is refused with `-32602`. A `work_path` is POSIX and relative; `.` segments are
//!   ignored, `..`, `\` and an absolute path are refused, and the file it names (after links) must
//!   lie inside the work dir.

use crate::agent::{AGENT_TURN, AgentBody, AgentError, TurnCaller, run_agent_into};
use crate::calls::{self, CallAnswer, CallCounter, CallError, file_value_digest, file_values};
use crate::engine::{Cancel, RunFiles, Services};
use crate::executor::InstanceJob;
use crate::executor::stage::file_ref;
use crate::host::connection::{Connection, ConnectionError};
use crate::host::pool::RunRequests;
use crate::store::records::FileEntry;
use crate::store::{Store, StoreError};
use base64::Engine as _;
use grida_fx_core::expr::{self, ExprError, Scope};
use grida_fx_core::kinds::{is_json, is_text, kind_of};
use grida_fx_core::text::{decode_text, prompt_text, py_repr_str};
use grida_fx_core::val::{FileContent, FileValue, Val, ViewId};
use grida_fx_core::value::{check_markers, format_number, parse_json, write_json};
use grida_fx_protocol::{
    AgentCheckParams, AgentCheckResult, AgentRunParams, AnnotateParams, CapabilityParams,
    CapabilityResult, ErrorCode, FactParams, FilePutParams, FilePutSource, Mark, MarkShape,
    ProgressParams, PromptRenderParams, RpcError, ToolInvokeParams, ToolInvokeResult, method,
};
use grida_fx_providers::BoxFuture;
use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// The requests of one run (module doc).
pub struct RunHandler {
    pub services: Arc<Services>,
    pub job: Arc<InstanceJob>,
    pub run_id: String,
    pub work_dir: PathBuf,
    pub files: Arc<RunFiles>,
    pub counter: Arc<CallCounter>,
    /// The leased host's connection, for `tool.invoke` and `agent.check`.
    pub host: Connection,
    pub cancel: Cancel,
    facts: Mutex<IndexMap<String, Value>>,
    marks: Mutex<Vec<Mark>>,
    fault: Mutex<Option<String>>,
    closed: std::sync::atomic::AtomicBool,
}

impl RunHandler {
    pub fn new(
        services: Arc<Services>,
        job: Arc<InstanceJob>,
        run_id: String,
        work_dir: PathBuf,
        files: Arc<RunFiles>,
        host: Connection,
        cancel: Cancel,
    ) -> RunHandler {
        RunHandler {
            services,
            job,
            run_id,
            work_dir,
            files,
            counter: Arc::new(CallCounter::new()),
            host,
            cancel,
            facts: Mutex::new(IndexMap::new()),
            marks: Mutex::new(Vec::new()),
            fault: Mutex::new(None),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The facts reported with `fact`, in order.
    pub fn facts(&self) -> IndexMap<String, Value> {
        self.facts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The marks reported with `annotate`, in order.
    pub fn marks(&self) -> Vec<Mark> {
        self.marks.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The first fault of the engine's own that a request of this run met (module doc,
    /// "Faults"), if any.
    pub fn fault(&self) -> Option<String> {
        self.fault.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Keeps `message` as the run's fault (the first one wins) and answers `internal`.
    fn faulted(&self, message: impl Into<String>) -> RpcError {
        let message = message.into();
        self.fault
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(|| message.clone());
        internal(message)
    }

    /// The answer for a failed paid call: a store fault is the run's fault.
    fn call_failed(&self, error: CallError) -> RpcError {
        match error {
            CallError::Fault(message) => self.faulted(message),
            other => other.to_rpc(),
        }
    }

    /// Ends the run: later requests are answered `cancelled`.
    pub fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether requests are answered `cancelled` now.
    fn ended(&self) -> bool {
        self.closed.load(Ordering::SeqCst) || self.cancel.is_cancelled()
    }

    /// Serves one request (module doc).
    async fn serve(self: Arc<Self>, method: &str, params: Value) -> Result<Value, RpcError> {
        let served = [
            method::CAPABILITY,
            method::AGENT_RUN,
            method::FACT,
            method::ANNOTATE,
            method::PROMPT_RENDER,
            method::FILE_PUT,
        ];
        if !served.contains(&method) {
            return Err(RpcError::new(
                ErrorCode::MethodNotFound,
                format!("the engine serves no {method} here"),
            ));
        }
        match params.get("run_id") {
            Some(Value::String(run_id)) if *run_id == self.run_id => {}
            Some(Value::String(run_id)) => {
                return Err(invalid(format!("no run {run_id} is pending on this host")));
            }
            _ => return Err(invalid(format!("{method} names its run_id"))),
        }
        if self.ended() {
            return Err(cancelled());
        }
        match method {
            method::CAPABILITY => self.capability(params).await,
            method::AGENT_RUN => self.agent_run(params).await,
            method::FACT => self.fact(params),
            method::ANNOTATE => self.annotate(params),
            method::PROMPT_RENDER => self.prompt_render(params),
            _ => self.file_put(params),
        }
    }

    /// `capability` (spec/protocol.md §6.1).
    async fn capability(&self, params: Value) -> Result<Value, RpcError> {
        let params: CapabilityParams = read(method::CAPABILITY, params)?;
        let request = Value::Object(params.request.into_iter().collect());
        let answer = spawn_call(self.paid(), params.capability, request)
            .await
            .map_err(|error| self.call_failed(error))?;
        let store = &self.services.engine.store;
        let mut refs = IndexMap::new();
        for (name, file) in &answer.files {
            let reference =
                file_ref(store, file, &self.files).map_err(|e| self.faulted(e.to_string()))?;
            refs.insert(name.clone(), reference);
        }
        let result = CapabilityResult {
            key: answer.key,
            cached: answer.cached,
            cost_usd: answer.cost.and_then(|cost| cost.to_value().as_f64()),
            files: refs,
            data: answer.data,
        };
        to_json(&result)
    }

    /// What a paid call of this run needs.
    fn paid(&self) -> Paid {
        Paid {
            services: Arc::clone(&self.services),
            site: Arc::new(self.job.call_site(self.cancel.clone())),
            counter: Arc::clone(&self.counter),
            files: Arc::clone(&self.files),
            cancel: self.cancel.clone(),
        }
    }

    /// `agent.run` (spec/protocol.md §6.2).
    async fn agent_run(&self, params: Value) -> Result<Value, RpcError> {
        let params: AgentRunParams = read(method::AGENT_RUN, params)?;
        for (i, image) in params.images.iter().flatten().enumerate() {
            self.handed(&image.file, &format!("images[{i}]"))?;
        }
        let turns = Turns { paid: self.paid() };
        let body = HostBody {
            host: self.host.clone(),
            run_id: self.run_id.clone(),
            cancel: self.cancel.clone(),
            files: Arc::clone(&self.files),
        };
        let mut transcript = Vec::new();
        let result = run_agent_into(&params, &turns, &body, &mut transcript)
            .await
            .map_err(|error| {
                let error = match error {
                    AgentError::Call(error) => self.call_failed(error),
                    AgentError::Rpc(error) => error,
                };
                with_transcript(error, &transcript)
            })?;
        to_json(&result)
    }

    /// Refuses a picture the run was not handed.
    fn handed(&self, digest: &str, where_: &str) -> Result<(), RpcError> {
        handed(&self.files, digest, where_)
    }

    /// `fact` (spec/protocol.md §6.3).
    fn fact(&self, params: Value) -> Result<Value, RpcError> {
        let params: FactParams = read(method::FACT, params)?;
        if params.name.is_empty() {
            return Err(invalid("a fact has a name"));
        }
        if params.name == "cost_usd" {
            return Err(invalid(
                "cost_usd is the engine's own fact; name yours otherwise",
            ));
        }
        let what = format!("fact {}", params.name);
        check_markers(&params.value, &what).map_err(|refused| invalid(refused.message))?;
        crate::events::check_depth(&params.value, &what).map_err(invalid)?;
        self.facts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(params.name, params.value);
        Ok(Value::Object(Map::new()))
    }

    /// `annotate` (spec/protocol.md §6.4).
    fn annotate(&self, params: Value) -> Result<Value, RpcError> {
        let params: AnnotateParams = read(method::ANNOTATE, params)?;
        check_mark(&params.mark).map_err(invalid)?;
        let raw = to_json(&params.mark)?;
        check_markers(&raw, "mark").map_err(|refused| invalid(refused.message))?;
        crate::events::check_depth(&raw, "mark").map_err(invalid)?;
        self.marks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(params.mark);
        Ok(Value::Object(Map::new()))
    }

    /// `prompt.render` (spec/protocol.md §6.6).
    fn prompt_render(&self, params: Value) -> Result<Value, RpcError> {
        let params: PromptRenderParams = read(method::PROMPT_RENDER, params)?;
        let Some(location) = self.job.resources.get(&params.path) else {
            return Err(RpcError::new(
                ErrorCode::UndeclaredResource,
                format!(
                    "{} is not one of {}'s declared resources",
                    params.path, self.job.spec.name
                ),
            ));
        };
        let store = &self.services.engine.store;
        let mut variables = IndexMap::new();
        for (name, value) in &params.variables {
            let where_ = format!("variables.{name}");
            file_values(value, &where_, &self.files).map_err(|refusal| refusal.to_rpc())?;
            variables.insert(name.clone(), variable(value, &self.files, store));
        }
        let bytes = std::fs::read(location).map_err(|error| {
            internal(format!(
                "cannot read {}: {}",
                params.path,
                io_reason(&error)
            ))
        })?;
        let text = decode_text(&bytes, &params.path)
            .map_err(|error| expression_error(error.to_string()))?;
        let mut run_params = IndexMap::new();
        for (name, value) in &self.job.with {
            if self.job.spec.params.contains_key(name) {
                run_params.insert(name.clone(), value.clone());
            }
        }
        let rendered = render_prompt(&prompt_text(&text), &run_params, &variables)
            .map_err(expression_error)?;
        let mut result = Map::new();
        result.insert("text".into(), Value::String(rendered));
        Ok(Value::Object(result))
    }

    /// `file.put` (spec/protocol.md §6.7).
    fn file_put(&self, params: Value) -> Result<Value, RpcError> {
        let params = file_put_params(params)?;
        if params.kind.as_deref() == Some("") {
            return Err(invalid("a file's kind is not empty"));
        }
        let store = &self.services.engine.store;
        // A source that cannot be read, or that changes while it is stored, is the body's: the
        // request is refused. Only the store's own faults stop the run.
        let stored_error = |error: StoreError| match error {
            StoreError::Source(sentence) => invalid(sentence),
            other => self.faulted(other.to_string()),
        };
        let (stored, kind, name) = match &params.source {
            FilePutSource::WorkPath { work_path } => {
                let path = work_file(&self.work_dir, work_path)?;
                let stored = store.put_file(&path, work_path).map_err(stored_error)?;
                let name = work_path
                    .rsplit('/')
                    .find(|segment| !segment.is_empty() && *segment != ".")
                    .unwrap_or("file");
                (stored, kind_of(work_path).to_string(), name.to_string())
            }
            FilePutSource::Base64 { base64 } => {
                let bytes = decode_base64(base64)?;
                let stored = store.put_bytes(&bytes).map_err(stored_error)?;
                (stored, "file".to_string(), "file".to_string())
            }
            FilePutSource::Json { json } => {
                check_markers(json, "json").map_err(|refused| invalid(refused.message))?;
                let stored = store
                    .put_bytes(write_json(json).as_bytes())
                    .map_err(stored_error)?;
                (stored, "json".to_string(), "file".to_string())
            }
        };
        let entry = FileEntry {
            digest: stored.digest,
            kind: params.kind.unwrap_or(kind),
            name: params.name.unwrap_or(name),
            size: stored.size,
            key: None,
        };
        let file = store.file_value(&entry).map_err(stored_error)?;
        self.files.insert(&file);
        let reference = file_ref(store, &file, &self.files).map_err(|e| self.faulted(e))?;
        to_json(&reference)
    }

    /// The `progress` notification (spec/protocol.md §6.5).
    fn progress(&self, params: Value) {
        let Ok(params) = serde_json::from_value::<ProgressParams>(params) else {
            return;
        };
        if params.run_id != self.run_id || self.ended() {
            return;
        }
        eprintln!(
            "{}",
            progress_line(&self.job.id, &params.text, params.fraction)
        );
    }
}

impl RunRequests for RunHandler {
    fn request(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        Box::pin(async move { self.serve(&method, params).await })
    }

    fn notify(&self, method: String, params: Value) {
        if method == method::PROGRESS {
            self.progress(params);
        }
    }

    /// The run's deadline passed, or it was stopped: later requests are answered `cancelled`,
    /// and a paid call in flight goes on on its own task and settles.
    fn stop(&self) {
        self.cancel.cancel();
    }
}

/// Everything a paid call of one run takes onto its own task.
#[derive(Clone)]
struct Paid {
    services: Arc<Services>,
    site: Arc<calls::CallSite>,
    counter: Arc<CallCounter>,
    files: Arc<RunFiles>,
    cancel: Cancel,
}

/// An error that ended an agent loop, with the transcript so far in `data.transcript` once the
/// loop had started (module doc); data that is not an object is left as it is.
fn with_transcript(
    mut error: RpcError,
    transcript: &[grida_fx_protocol::AgentMessage],
) -> RpcError {
    if transcript.is_empty() {
        return error;
    }
    let Ok(messages) = serde_json::to_value(transcript) else {
        return error;
    };
    if let Value::Object(data) = error.data.get_or_insert_with(|| Value::Object(Map::new())) {
        data.insert("transcript".into(), messages);
    }
    error
}

/// Runs a paid call on a task of its own and waits for it, or for the run to be cancelled
/// (module doc): the call goes on and settles either way, counted as running from before its
/// task is spawned.
async fn spawn_call(
    paid: Paid,
    capability: String,
    request: Value,
) -> Result<CallAnswer, CallError> {
    let cancel = paid.cancel.clone();
    let running = paid.services.track_call();
    let task = tokio::spawn(async move {
        let _running = running;
        calls::call(
            &paid.services,
            &paid.site,
            &paid.counter,
            &paid.files,
            &capability,
            request,
        )
        .await
    });
    tokio::select! {
        biased;
        joined = task => joined.unwrap_or_else(|error| {
            Err(CallError::Fault(format!(
                "a paid call stopped inside the engine: {error}"
            )))
        }),
        () = cancel.cancelled() => Err(CallError::Cancelled),
    }
}

/// The run's `agent.turn` calls.
struct Turns {
    paid: Paid,
}

impl TurnCaller for Turns {
    fn turn(&self, request: Value) -> BoxFuture<'_, Result<CallAnswer, CallError>> {
        Box::pin(spawn_call(
            self.paid.clone(),
            AGENT_TURN.to_string(),
            request,
        ))
    }
}

/// The body's tools and check, over the leased host's connection.
struct HostBody {
    host: Connection,
    run_id: String,
    cancel: Cancel,
    files: Arc<RunFiles>,
}

impl HostBody {
    async fn ask<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, RpcError> {
        let answer = self
            .host
            .request_cancellable(method, Some(params), &self.cancel)
            .await
            .map_err(|error| match error {
                ConnectionError::Rpc(error) => error,
                other => internal(other.to_string()),
            })?;
        serde_json::from_value(answer).map_err(|error| {
            internal(format!(
                "the node host answered {method} with something else: {}",
                crate::host::read_reason(&error)
            ))
        })
    }
}

impl AgentBody for HostBody {
    fn invoke<'a>(
        &'a self,
        agent_id: &'a str,
        call_id: Option<&'a str>,
        name: &'a str,
        arguments: &'a IndexMap<String, Value>,
    ) -> BoxFuture<'a, Result<ToolInvokeResult, RpcError>> {
        Box::pin(async move {
            let params = to_json(&ToolInvokeParams {
                run_id: self.run_id.clone(),
                agent_id: agent_id.to_string(),
                call_id: call_id.map(str::to_string),
                name: name.to_string(),
                arguments: arguments.clone(),
            })?;
            let result: ToolInvokeResult = self.ask(method::TOOL_INVOKE, params).await?;
            if let ToolInvokeResult::Content {
                images: Some(images),
                ..
            } = &result
            {
                for (i, image) in images.iter().enumerate() {
                    handed(&self.files, &image.file, &format!("{name}: images[{i}]"))?;
                }
            }
            Ok(result)
        })
    }

    fn check<'a>(
        &'a self,
        agent_id: &'a str,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<Option<String>, RpcError>> {
        Box::pin(async move {
            let params = to_json(&AgentCheckParams {
                run_id: self.run_id.clone(),
                agent_id: agent_id.to_string(),
                value: value.clone(),
            })?;
            let result: AgentCheckResult = self.ask(method::AGENT_CHECK, params).await?;
            Ok(result.refusal)
        })
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn internal(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::Internal, message)
}

fn cancelled() -> RpcError {
    CallError::Cancelled.to_rpc()
}

fn expression_error(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::ExpressionError, message)
}

/// Reads typed params; a mismatch is `-32602`, said as a sentence (`host::read_reason`).
fn read<T: DeserializeOwned>(method: &str, params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|error| invalid(format!("{method}: {}", crate::host::read_reason(&error))))
}

/// The sources of `file.put`: a request gives exactly one.
const FILE_PUT_SOURCES: [&str; 3] = ["work_path", "base64", "json"];

/// Reads `file.put` params: exactly one source (module doc), then as [`read`] reads them.
fn file_put_params(params: Value) -> Result<FilePutParams, RpcError> {
    if let Value::Object(members) = &params {
        let sources = FILE_PUT_SOURCES
            .iter()
            .filter(|name| members.contains_key(**name))
            .count();
        if sources != 1 {
            return Err(invalid(
                "file.put takes exactly one of work_path, base64, json",
            ));
        }
        let known = |name: &str| {
            FILE_PUT_SOURCES.contains(&name) || matches!(name, "run_id" | "kind" | "name")
        };
        if let Some(name) = members.keys().find(|name| !known(name)) {
            return Err(invalid(format!(
                "file.put: {name} is not one of its fields"
            )));
        }
        for name in ["work_path", "base64"] {
            if members.get(name).is_some_and(|value| !value.is_string()) {
                return Err(invalid(format!("file.put: {name} is text")));
            }
        }
    }
    read(method::FILE_PUT, params)
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| {
        internal(format!(
            "the engine could not write its answer: {}",
            crate::host::read_reason(&error)
        ))
    })
}

/// `unknown_file` unless the run was handed `digest`.
fn handed(files: &RunFiles, digest: &str, where_: &str) -> Result<(), RpcError> {
    match files.get(digest) {
        Some(_) => Ok(()),
        None => Err(RpcError::new(
            ErrorCode::UnknownFile,
            format!("{where_} names the file {digest}, which this run was not handed"),
        )),
    }
}

/// An I/O error's reason without the path it names.
fn io_reason(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "no such file".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => error.to_string(),
    }
}

/// A `progress` line (module doc).
pub(crate) fn progress_line(instance_id: &str, text: &str, fraction: Option<f64>) -> String {
    match fraction {
        Some(fraction) if fraction.is_finite() => format!(
            "{instance_id}: {text} ({}%)",
            format_number((fraction * 100.0).round())
        ),
        _ => format!("{instance_id}: {text}"),
    }
}

/// Checks a mark's geometry (spec/protocol.md §6.4; the types are checked when it is read).
pub(crate) fn check_mark(mark: &Mark) -> Result<(), String> {
    match mark.shape {
        Some(MarkShape::Point) if mark.at.is_none() => {
            return Err("a point mark needs at: [x, y]".into());
        }
        Some(MarkShape::Points) if mark.points.as_ref().is_none_or(|p| p.len() < 2) => {
            return Err("a points mark needs points: at least 2 [x, y] pairs".into());
        }
        Some(MarkShape::Box) if mark.box_.is_none() => {
            return Err("a box mark needs box: [x0, y0, x1, y1]".into());
        }
        _ => {}
    }
    let mut coordinates: Vec<(&str, f64)> = Vec::new();
    if let Some(at) = &mark.at {
        coordinates.extend(at.iter().map(|x| ("at", *x)));
    }
    for point in mark.points.iter().flatten() {
        coordinates.extend(point.iter().map(|x| ("points", *x)));
    }
    if let Some(corners) = &mark.box_ {
        coordinates.extend(corners.iter().map(|x| ("box", *x)));
    }
    for (member, x) in coordinates {
        if !(0.0..=1.0).contains(&x) {
            let shown = if x.is_finite() {
                format_number(x)
            } else {
                x.to_string()
            };
            return Err(format!(
                "a mark's {member} lies in fractions of the image from 0 to 1, not {shown}"
            ));
        }
    }
    Ok(())
}

/// The file a `work_path` names inside the work dir (module doc), or `outside_work_dir`.
pub(crate) fn work_file(work_dir: &Path, work_path: &str) -> Result<PathBuf, RpcError> {
    let refused = || {
        RpcError::new(
            ErrorCode::OutsideWorkDir,
            format!("{work_path} names nothing inside the work dir"),
        )
    };
    if work_path.is_empty()
        || work_path.starts_with('/')
        || work_path.contains('\\')
        || work_path.contains('\0')
    {
        return Err(refused());
    }
    let mut path = work_dir.to_path_buf();
    for segment in work_path.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(refused()),
            segment => path.push(segment),
        }
    }
    let root = work_dir.canonicalize().map_err(|_| refused())?;
    let real = path.canonicalize().map_err(|_| refused())?;
    if real == root || !real.starts_with(&root) || !real.is_file() {
        return Err(refused());
    }
    Ok(real)
}

/// Strict base64 with padding (RFC 4648 §4), else `-32602`.
pub(crate) fn decode_base64(text: &str) -> Result<Vec<u8>, RpcError> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| invalid("base64 is not RFC 4648 base64 with padding"))
}

/// A prompt variable as a runtime value: file values become the files the run was handed (the
/// caller has checked every one), with their content when they are text or JSON.
fn variable(value: &Value, files: &RunFiles, store: &Store) -> Val {
    match value {
        Value::Object(map) => match file_value_digest(map).and_then(|digest| files.get(digest)) {
            Some(file) => Val::File(Box::new(with_content(file, store))),
            None => Val::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), variable(v, files, store)))
                    .collect(),
            ),
        },
        Value::Array(items) => Val::List(items.iter().map(|v| variable(v, files, store)).collect()),
        other => Val::from_json(other),
    }
}

/// A text or JSON file with its content read (from its own copy, else the store's), so it renders
/// as its content (spec/identity.md §5); unchanged when it cannot be read.
fn with_content(mut file: FileValue, store: &Store) -> FileValue {
    if file.content.is_some() || !(is_json(&file.kind) || is_text(&file.kind)) {
        return file;
    }
    let bytes = file.read_bytes().ok().or_else(|| {
        store
            .file_path(&file.digest)
            .ok()
            .and_then(|path| std::fs::read(path).ok())
    });
    let Some(text) = bytes.and_then(|bytes| decode_text(&bytes, &file.name).ok()) else {
        return file;
    };
    file.content = if is_json(&file.kind) {
        parse_json(&text).ok().map(FileContent::Json)
    } else {
        Some(FileContent::Text(text))
    };
    file
}

/// The prompt scope: `variables` first, then the run's params; any other name is unknown.
struct PromptScope<'a> {
    params: &'a IndexMap<String, Val>,
    variables: &'a IndexMap<String, Val>,
}

impl Scope for PromptScope<'_> {
    fn root(&mut self, name: &str) -> Result<Val, ExprError> {
        self.variables
            .get(name)
            .or_else(|| self.params.get(name))
            .cloned()
            .ok_or_else(|| {
                ExprError::new(format!(
                    "the prompt names {}, which nobody gave it",
                    py_repr_str(name)
                ))
            })
    }

    fn view_member(&mut self, _view: ViewId, _name: &str) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_item(&mut self, _view: ViewId, _index: &Val) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_every(&mut self, _view: ViewId) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn view_len(&mut self, _view: ViewId) -> Result<usize, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn finish_view(&mut self, _view: ViewId) -> Result<Val, ExprError> {
        Err(ExprError::names_a_step())
    }

    fn facts(&mut self, value: &Val) -> Result<Val, ExprError> {
        let Val::File(file) = value else {
            return Err(ExprError::new(format!(
                "facts() needs a file, not {}",
                value.kind_word()
            )));
        };
        file.file_facts()
            .map(|facts| Val::from_json(&facts))
            .map_err(|reason| ExprError::new(format!("{}: {reason}", file.name)))
    }
}

/// Renders a prompt's text (comments already removed) over the run's params with `variables` on
/// top (spec/identity.md §5): the rendered text, or the expression error's sentence.
pub(crate) fn render_prompt(
    source: &str,
    params: &IndexMap<String, Val>,
    variables: &IndexMap<String, Val>,
) -> Result<String, String> {
    let Some(template) = expr::template(source).map_err(|e| e.to_string())? else {
        return Ok(source.to_string());
    };
    let mut scope = PromptScope { params, variables };
    let rendered = expr::render(&template, &mut scope).map_err(|e| e.to_string())?;
    rendered.text().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mark(value: Value) -> Mark {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn marks_need_their_geometry_inside_the_image() {
        assert_eq!(check_mark(&mark(json!({"label": "whole"}))), Ok(()));
        assert_eq!(
            check_mark(&mark(json!({"shape": "point", "at": [0, 1]}))),
            Ok(())
        );
        assert_eq!(
            check_mark(&mark(
                json!({"shape": "points", "points": [[0.1, 0.2], [0.3, 0.4]], "closed": true})
            )),
            Ok(())
        );
        assert_eq!(
            check_mark(&mark(
                json!({"shape": "box", "box": [0, 0, 0.5, 0.5], "tag": "t"})
            )),
            Ok(())
        );
        assert_eq!(
            check_mark(&mark(json!({"shape": "point"}))).unwrap_err(),
            "a point mark needs at: [x, y]"
        );
        assert_eq!(
            check_mark(&mark(json!({"shape": "points", "points": [[0.1, 0.2]]}))).unwrap_err(),
            "a points mark needs points: at least 2 [x, y] pairs"
        );
        assert_eq!(
            check_mark(&mark(json!({"shape": "box"}))).unwrap_err(),
            "a box mark needs box: [x0, y0, x1, y1]"
        );
        assert_eq!(
            check_mark(&mark(json!({"shape": "box", "box": [0, 0, 1.5, 1]}))).unwrap_err(),
            "a mark's box lies in fractions of the image from 0 to 1, not 1.5"
        );
        assert_eq!(
            check_mark(&mark(json!({"at": [-0.25, 0.5]}))).unwrap_err(),
            "a mark's at lies in fractions of the image from 0 to 1, not -0.25"
        );
    }

    #[test]
    fn progress_lines() {
        assert_eq!(
            progress_line("draw#1", "half", Some(0.5)),
            "draw#1: half (50%)"
        );
        assert_eq!(progress_line("draw#1", "busy", None), "draw#1: busy");
        assert_eq!(progress_line("a#2.1", "x", Some(0.333)), "a#2.1: x (33%)");
    }

    #[test]
    fn work_paths_stay_inside_the_work_dir() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("out")).unwrap();
        std::fs::write(work.join("out/a.png"), b"x").unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"s").unwrap();
        let real = work.join("out/a.png").canonicalize().unwrap();
        assert_eq!(work_file(&work, "out/a.png").unwrap(), real);
        assert_eq!(work_file(&work, "./out/./a.png").unwrap(), real);
        for refused in [
            "",
            "/etc/passwd",
            "../secret.txt",
            "out/../../secret.txt",
            "out\\a.png",
            "out/missing.png",
            "out",
            ".",
        ] {
            let error = work_file(&work, refused).unwrap_err();
            assert_eq!(error.code, ErrorCode::OutsideWorkDir.code(), "{refused:?}");
            assert_eq!(
                error.message,
                format!("{refused} names nothing inside the work dir")
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret.txt"), work.join("link.txt"))
                .unwrap();
            assert!(work_file(&work, "link.txt").is_err());
        }
    }

    #[test]
    fn base64_is_strict() {
        assert_eq!(decode_base64("AAE=").unwrap(), vec![0, 1]);
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
        for refused in ["AAE", "AA=E", "A A=", "AAE=\n", "AAF=", "*AE="] {
            let error = decode_base64(refused).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidParams.code(), "{refused:?}");
        }
    }

    fn vals(pairs: Value) -> IndexMap<String, Val> {
        let Value::Object(map) = pairs else {
            panic!("an object")
        };
        map.iter()
            .map(|(k, v)| (k.clone(), Val::from_json(v)))
            .collect()
    }

    #[test]
    fn prompts_render_params_with_variables_on_top() {
        let params = vals(json!({"who": "W", "n": 1e16, "flag": true, "vars": {"x": 2}}));
        let variables = vals(json!({"flag": false}));
        assert_eq!(
            render_prompt(
                "Hello ${{ who }}!\r\nLine two ${{ n }} ${{ flag }} ${{ vars.x }}\r\n",
                &params,
                &variables
            )
            .unwrap(),
            "Hello W!\r\nLine two 10000000000000000 false 2\r\n"
        );
        assert_eq!(
            render_prompt("no template", &params, &variables).unwrap(),
            "no template"
        );
        assert_eq!(
            render_prompt("${{ vars }}", &params, &variables).unwrap(),
            "{\"x\":2}"
        );
        assert_eq!(
            render_prompt("Hi ${{ nobody }}", &params, &variables).unwrap_err(),
            "the prompt names 'nobody', which nobody gave it"
        );
        let unclosed = render_prompt("Hi ${{ who", &params, &variables).unwrap_err();
        assert!(unclosed.starts_with("unclosed ${{ in "), "{unclosed}");
    }

    #[test]
    fn prompt_files_render_as_their_content() {
        let mut variables = IndexMap::new();
        variables.insert(
            "brief".to_string(),
            Val::File(Box::new(FileValue {
                digest: format!("{:064x}", 1),
                kind: "text/plain".into(),
                name: "brief.txt".into(),
                size: 6,
                key: None,
                content: Some(FileContent::Text("a cat".into())),
                location: None,
            })),
        );
        variables.insert(
            "pic".to_string(),
            Val::File(Box::new(FileValue {
                digest: format!("{:064x}", 2),
                kind: "image/png".into(),
                name: "pic.png".into(),
                size: 6,
                key: None,
                content: None,
                location: None,
            })),
        );
        assert_eq!(
            render_prompt(
                "Draw ${{ brief }} like ${{ pic }}",
                &IndexMap::new(),
                &variables
            )
            .unwrap(),
            format!("Draw a cat like sha256:{:064x}", 2)
        );
    }
}
