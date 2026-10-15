//! A project's own adapters in a live run (spec/adapters.md §9; spec/protocol.md §5.8, §6.8).
//!
//! [`register`] gives each provider the planning project declares with an adapter, which planning
//! loaded (`grida_fx_core::adapters`), one [`AdapterHost`] and one [`ProjectAdapter`] for each
//! capability the adapter declares: a request adapter or a long job by its kind, the default of
//! its capability on the provider, named by the adapter's identity (`<path>#<attribute>`, and its
//! version or `source:<digest>`), billing dollars. The engine's own checks wrap it as any adapter
//! (`calls::checked`): the contract and the request before a send or a submit.
//!
//! **The adapter host** is started on the first call that needs it ([`Launch`]: the node host
//! program of the module's language, found from the planning project and started in its root,
//! outside the pools, with the environment the caller gives it), with the network off too. It
//! loads the adapter again with `adapter.load`, which must answer what planning learned: the same
//! version, capabilities and kinds, and a source closure of the same digest. Otherwise, or when it
//! cannot start or load, the host is refused for the rest of the run, and every call is refused
//! before anything is sent (a collect is unreachable): `the adapter <name> changed since the run
//! was planned`, `the adapter host for <name> could not be started: <message>`. The host is kept
//! as a runtime of the registry ([`grida_fx_providers::adapter::Runtime`]), so the run's cleanup
//! shuts it down and counts it in its verification; a host this process started is also ended by
//! `host::end_every_host`. A host that died or would not serve, or whose start was abandoned (the
//! call that started it dropped, as a run stopped while the adapter loads drops it), is ended on a
//! task of its own, which the cleanup waits for and counts too, and a host process dropped before
//! it was reaped leaves the cleanup unverified.
//!
//! **A call** goes to the host as `adapter.send`, `adapter.submit` or `adapter.collect`, with a
//! `call_id` unique in the host's session (`c<n>`), on a task of its own, so several are pending
//! at once. Before it, a send or a submit whose declaration has a key that is not set is refused
//! (`<VARIABLE> is not set`; spec/adapters.md §4 step 3). A call the engine drops is cancelled
//! with `$/cancel`, its pending `fetch` requests are answered `cancelled`, and whatever the host
//! answers later is discarded.
//!
//! **Outcomes** map one to one (spec/adapters.md §9's table): a `reason` must not be blank, a
//! `cost_usd` is read with `wire::usd_ceil`, a `retry_after_s` is a wait of at least 0 seconds,
//! and every reason is redacted with the invocation's keys and the URLs the call downloaded
//! through `fetch`. An answer is held to its shape first, as a stand-in's
//! (`grida_fx_providers::stand_in::shape`): a refusal fails the attempt as billed and may be
//! retried, as a check's refusal (`Sent::Failed { retryable: true }`, `Collected::Ended`), at the
//! reported cost. An accepted handle whose text holds a key's value is `Uncertain`. The host lost
//! while the call is pending, an error answer, or a result FX cannot read ends the call as an
//! unknown outcome demands: a send `Failed { cost: None, retryable: false }`, a submit
//! `Uncertain`, a collect `Unreachable`, with `the adapter host exited with status <n>` (`… was
//! killed by signal <n> (<NAME>)`, `… exited`), `the adapter <name> failed: <message>`, `the
//! adapter <name> answered something FX cannot read: <where>`, or `the adapter host broke the
//! protocol: <message>`. The next call starts a new host. A send or a submit whose request was
//! never written whole, the host's connection having ended first, provably did not reach it: it
//! is `NotReceived`, with the same sentence, and the engine sends it again, to a new host (a
//! collect is `Unreachable`).
//!
//! **`fetch`** (spec/protocol.md §6.8) makes one exchange through the setup's transport (the
//! `Offline` one when the network is off). Refused with `-32602`: a `call_id` that names no
//! pending call, a body on a `GET`, a header name that is not a lowercase token, a body whose
//! bytes are not base64, a `timeout_s` outside (0, 300] or a `max_response_bytes` outside 1 to
//! 64 MiB. A call the engine dropped, before or during the exchange: `cancelled`. Refused before
//! it leaves (`phase: refused`): a credential asked for on the download lane, or for a URL whose
//! origin is not the declaration's base URL's; the declaration's own credential header set by
//! the adapter; a download outside the declaration's download hosts; and whatever the transport
//! refuses (the network off, a URL it cannot use). The key is attached only when asked, as the
//! declaration says; a declaration without a credential sends none, and one whose key is not set
//! refuses the exchange, `<VARIABLE> is not set`.

use crate::engine::Cancel;
use crate::host::connection::{Connection, ConnectionError, Incoming, Unanswered};
use crate::host::process::{HostProcess, HostSpec};
use crate::host::{GRACE, exit_text, read_reason};
use base64::Engine as _;
use grida_fx_core::money::Usd;
use grida_fx_core::providers::{Declaration, DeclaredAdapter, LoadedAdapter};
use grida_fx_protocol::{
    AdapterAnswer, AdapterCallParams, AdapterCollectParams, AdapterCollectResult, AdapterFile,
    AdapterLoadResult, AdapterRoute, AdapterSendResult, AdapterSubmitResult, AdapterTarget,
    CallKind, ErrorCode, FetchBody, FetchFailure, FetchLane, FetchMethod, FetchParams, FetchPhase,
    FetchResult, RpcError, method,
};
use grida_fx_providers::adapter::{
    Adapter, Answer, CallRequest, Collected, Identity, LongJob, RequestAdapter, Runtime, Sent,
    Submitted, Usage,
};
use grida_fx_providers::redact::Redactor;
use grida_fx_providers::setup::{Client, Setup};
use grida_fx_providers::stand_in::{StandInFile, shape};
use grida_fx_providers::transport::{Body, HttpRequest, Lane, Method, Phase, TransportErrorKind};
use grida_fx_providers::{Adapters, BoxFuture};
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::watch;

/// The longest deadline and the largest response a `fetch` may ask for (spec/protocol.md §6.8).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(300);
pub const FETCH_MAX_BYTES: u64 = 64 * 1024 * 1024;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// How an adapter host is reached (module doc): a node host process ([`ProcessLaunch`]), or for
/// tests a peer on a connection the caller makes.
pub trait Launch: Send + Sync {
    /// Starts a host whose requests `incoming` serves, its session initialized. A process it
    /// starts that is dropped before it is reaped, its caller dropped while it starts included,
    /// clears `cleanup_verified`. The error is the reason, after `the adapter host for <name>
    /// could not be started: `.
    fn launch(
        &self,
        incoming: Arc<dyn Incoming>,
        cleanup_verified: Arc<AtomicBool>,
    ) -> BoxFuture<'_, Result<Launched, String>>;
}

/// A started adapter host: its connection, and its process when the engine started one.
pub struct Launched {
    pub connection: Connection,
    pub process: Option<HostProcess>,
}

/// The node host program of the adapter module's language, as `spec` says (spec/protocol.md §1);
/// `Err` is why there is none.
pub struct ProcessLaunch {
    spec: Result<HostSpec, String>,
}

impl ProcessLaunch {
    pub fn new(spec: Result<HostSpec, String>) -> ProcessLaunch {
        ProcessLaunch { spec }
    }
}

impl Launch for ProcessLaunch {
    fn launch(
        &self,
        incoming: Arc<dyn Incoming>,
        cleanup_verified: Arc<AtomicBool>,
    ) -> BoxFuture<'_, Result<Launched, String>> {
        Box::pin(async move {
            let spec = self.spec.as_ref().map_err(String::clone)?;
            let process = HostProcess::start_accountable(spec, incoming, cleanup_verified).await?;
            Ok(Launched {
                connection: process.connection().clone(),
                process: Some(process),
            })
        })
    }
}

/// A launcher that never starts a host: every call is refused with its reason. `doctor` builds
/// its registry with it, to say which routes an adapter serves.
pub struct NoLaunch(pub String);

impl Launch for NoLaunch {
    fn launch(
        &self,
        _: Arc<dyn Incoming>,
        _: Arc<AtomicBool>,
    ) -> BoxFuture<'_, Result<Launched, String>> {
        Box::pin(async move { Err(self.0.clone()) })
    }
}

/// Registers the project adapters of `setup`'s declarations (module doc): for each declaration
/// whose adapter planning loaded, an [`AdapterHost`] started by `launch(declaration, adapter)`,
/// kept as a runtime, and one adapter for each capability it declares.
pub fn register(
    adapters: &mut Adapters,
    setup: &Setup,
    launch: impl Fn(&Declaration, &DeclaredAdapter) -> Arc<dyn Launch>,
) {
    for declaration in setup.declarations().declared() {
        let Some(adapter) = &declaration.adapter else {
            continue;
        };
        let Some(loaded) = &adapter.loaded else {
            continue;
        };
        let host = Arc::new(AdapterHost::new(
            adapter,
            loaded.clone(),
            setup.declared_client(declaration),
            setup
                .keys
                .secrets()
                .iter()
                .map(|s| s.expose().to_string())
                .collect(),
            launch(declaration, adapter),
        ));
        adapters.keep_runtime(Arc::clone(&host) as Arc<dyn Runtime>);
        for (capability, kind) in &loaded.capabilities {
            let served = Arc::new(ProjectAdapter {
                host: Arc::clone(&host),
            });
            let registered = match kind {
                CallKind::Request => Adapter::Request(served),
                CallKind::Job => Adapter::Job(served),
            };
            adapters.register(capability, &declaration.name, registered);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The host

/// One declaration's adapter host (module doc).
pub struct AdapterHost {
    /// `<path>#<attribute>`.
    name: String,
    target: AdapterTarget,
    identity: Identity,
    /// What planning learned.
    loaded: LoadedAdapter,
    launch: Arc<dyn Launch>,
    /// The declaration's client: the transport, its credential and key, base URL and download
    /// hosts, and the redactor over every key of the invocation.
    client: Client,
    /// Every key's value of the invocation, which a handle must not hold.
    secrets: Vec<String>,
    calls: Arc<Calls>,
    state: tokio::sync::Mutex<State>,
    next_call: AtomicU64,
    /// The hosts being ended on tasks of their own ([`AdapterHost::end_apart`]), which
    /// `shutdown` waits for.
    endings: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    cleanup_verified: Arc<AtomicBool>,
}

/// Where the host stands.
enum State {
    Idle,
    /// It cannot serve this run: every call's refusal.
    Refused(String),
    Running(Arc<Running>),
}

/// What one call came to before its outcome is read.
enum Asked {
    /// The host's result, and the redactor of the call's reasons.
    Result(Value, Redactor),
    /// Refused before anything was sent.
    Refused(String),
    /// Never written to the host, whose connection had ended: provably not received.
    NotSent(String),
    /// Ended as the loss of the host would end it.
    Lost(String),
}

/// A refusal of an answer: one FX cannot read, or one whose shape is refused (billed at `cost`).
enum Refusal {
    Unreadable(String),
    Shape { reason: String, cost: Option<Usd> },
}

impl AdapterHost {
    /// The host of `adapter`, which planning loaded as `loaded`, reaching its provider through
    /// `client` and started by `launch`; `secrets` are every key's value of the invocation.
    pub fn new(
        adapter: &DeclaredAdapter,
        loaded: LoadedAdapter,
        client: Client,
        secrets: Vec<String>,
        launch: Arc<dyn Launch>,
    ) -> AdapterHost {
        let name = adapter.name();
        let behavior = grida_fx_core::value::canon(&loaded.behavior());
        AdapterHost {
            identity: Identity::interned(&name, &behavior),
            name,
            target: AdapterTarget {
                path: adapter.path.clone(),
                attribute: adapter.attribute.clone(),
            },
            loaded,
            launch,
            client,
            secrets: secrets.into_iter().filter(|s| !s.is_empty()).collect(),
            calls: Arc::new(Calls::default()),
            state: tokio::sync::Mutex::new(State::Idle),
            next_call: AtomicU64::new(1),
            endings: Mutex::new(Vec::new()),
            cleanup_verified: Arc::new(AtomicBool::new(true)),
        }
    }

    /// The contract it serves.
    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// The running host, started when there is none or the last one is gone.
    async fn running(&self) -> Result<Arc<Running>, String> {
        let mut state = self.state.lock().await;
        match &*state {
            State::Refused(reason) => return Err(reason.clone()),
            State::Running(running) if running.alive() => return Ok(Arc::clone(running)),
            State::Running(running) => {
                let ended = self.end_apart(Arc::clone(running));
                *state = State::Idle;
                let _ = ended.await;
            }
            State::Idle => {}
        }
        match self.start().await {
            Ok(running) => {
                *state = State::Running(Arc::clone(&running));
                Ok(running)
            }
            Err(reason) => {
                *state = State::Refused(reason.clone());
                Err(reason)
            }
        }
    }

    /// Starts a host and loads the adapter; the refusal of every call when it cannot serve.
    async fn start(&self) -> Result<Arc<Running>, String> {
        let incoming = Arc::new(Fetches {
            calls: Arc::clone(&self.calls),
            client: self.client.clone(),
        });
        let launched = self
            .launch
            .launch(incoming, Arc::clone(&self.cleanup_verified))
            .await
            .map_err(|why| self.not_started(&why))?;
        // From here, a caller dropped before the host is kept or refused leaves it ended apart.
        let starting = Starting {
            host: self,
            running: Some(Running::watch(launched)),
        };
        let loaded = self.load(starting.running()).await;
        let running = starting.take();
        match loaded {
            Ok(()) => Ok(running),
            Err(refusal) => {
                let _ = self.end_apart(running).await;
                Err(refusal)
            }
        }
    }

    /// Loads the adapter on a started host; the refusal of every call when it loads another
    /// one than the plan's, or cannot load it.
    async fn load(&self, running: &Running) -> Result<(), String> {
        let not_started = |why: &str| self.not_started(why);
        let params =
            serde_json::to_value(&self.target).map_err(|e| not_started(&read_reason(&e)))?;
        let answered = running
            .connection
            .request(method::ADAPTER_LOAD, Some(params))
            .await;
        Err(match answered {
            Ok(value) => match serde_json::from_value::<AdapterLoadResult>(value) {
                Ok(loaded) if self.same_as_planned(&loaded) => return Ok(()),
                Ok(_) => format!(
                    "the adapter {} changed since the run was planned",
                    self.name
                ),
                Err(error) => not_started(&format!(
                    "it answered adapter.load with something FX cannot read: {}",
                    read_reason(&error)
                )),
            },
            Err(ConnectionError::Rpc(error)) => not_started(&error.message),
            Err(ConnectionError::Closed) => {
                let status = running.status().await;
                not_started(&format!(
                    "it {} before it answered adapter.load",
                    exit_text(status)
                ))
            }
            Err(ConnectionError::Protocol(message)) => {
                not_started(&format!("it broke the protocol: {message}"))
            }
        })
    }

    /// Why the host cannot serve the run: it did not start, or could not load the adapter.
    fn not_started(&self, why: &str) -> String {
        format!(
            "the adapter host for {} could not be started: {why}",
            self.name
        )
    }

    /// Ends `running` on a task of its own, so that it ends though its caller is dropped
    /// meanwhile, keeping whether its cleanup was verified; `shutdown` waits for it. The
    /// receiver resolves once it has ended.
    fn end_apart(&self, running: Arc<Running>) -> tokio::sync::oneshot::Receiver<()> {
        let (ended, receiver) = tokio::sync::oneshot::channel();
        let verified = Arc::clone(&self.cleanup_verified);
        let ending = async move {
            if !running.end().await {
                verified.store(false, Ordering::SeqCst);
            }
            let _ = ended.send(());
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => locked(&self.endings).push(runtime.spawn(ending)),
            // No runtime is left to end it on: it is killed when its process is dropped.
            Err(_) => self.cleanup_verified.store(false, Ordering::SeqCst),
        }
        receiver
    }

    /// Whether the host's `adapter.load` answers what planning learned (module doc).
    fn same_as_planned(&self, loaded: &AdapterLoadResult) -> bool {
        let planned = &self.loaded;
        loaded.adapter.version == planned.version
            && loaded.adapter.capabilities == planned.capabilities
            && grida_fx_core::registry::closure_digests(&loaded.closure)
                .is_ok_and(|source| source.digest() == planned.source.digest())
    }

    /// The refusal of a send or a submit whose declaration's key is not set (spec/adapters.md §4
    /// step 3).
    fn key(&self) -> Result<(), String> {
        self.client.optional_credential().map(|_| ())
    }

    /// Asks the host `method` for `call` (module doc).
    async fn ask(&self, method: &'static str, call: &CallRequest, handle: Option<&Value>) -> Asked {
        let running = match self.running().await {
            Ok(running) => running,
            Err(reason) => return Asked::Refused(reason),
        };
        let call_id = format!("c{}", self.next_call.fetch_add(1, Ordering::SeqCst));
        let params = match self.params(call, &call_id, handle) {
            Ok(params) => params,
            Err(reason) => return Asked::Refused(reason),
        };
        let mut guard = self.calls.open(&call_id);
        // The request runs on a task of its own: when the call is dropped it still sends
        // `$/cancel`, and an answer that arrives later is discarded with it.
        let connection = running.connection.clone();
        let token = guard.pending.cancel.clone();
        let asked = tokio::spawn(async move {
            connection
                .try_request_cancellable(method, Some(params), &token)
                .await
        });
        let answered = asked.await.unwrap_or(Err(Unanswered {
            error: ConnectionError::Closed,
            written: true,
        }));
        guard.done = true;
        let redactor = guard.redactor(&self.client.redactor);
        let Unanswered { error, written } = match answered {
            Ok(value) => return Asked::Result(value, redactor),
            Err(unanswered) => unanswered,
        };
        let reason = match error {
            ConnectionError::Rpc(error) => {
                format!("the adapter {} failed: {}", self.name, error.message)
            }
            ConnectionError::Closed => {
                let status = running.status().await;
                format!("the adapter host {}", exit_text(status))
            }
            ConnectionError::Protocol(message) => {
                format!("the adapter host broke the protocol: {message}")
            }
        };
        let reason = redactor.reason(&reason);
        if written {
            Asked::Lost(reason)
        } else {
            Asked::NotSent(reason)
        }
    }

    /// A call's params (spec/protocol.md §5.8), with `handle` for a collect. The error is a
    /// refusal before anything is sent.
    fn params(
        &self,
        call: &CallRequest,
        call_id: &str,
        handle: Option<&Value>,
    ) -> Result<Value, String> {
        let object = |value: &Value| -> IndexMap<String, Value> {
            match value {
                Value::Object(members) => members.clone().into_iter().collect(),
                _ => IndexMap::new(),
            }
        };
        let mut files = IndexMap::new();
        for (digest, file) in &call.files {
            let path = file.path.to_str().ok_or_else(|| {
                "a file's path is not UTF-8, which the node protocol needs".to_string()
            })?;
            files.insert(
                digest.clone(),
                AdapterFile {
                    digest: file.digest.clone(),
                    kind: file.kind.clone(),
                    size: file.size,
                    path: path.to_string(),
                },
            );
        }
        let params = AdapterCallParams {
            call_id: call_id.to_string(),
            adapter: self.target.clone(),
            capability: call.route.capability.clone(),
            route: AdapterRoute {
                id: call.route.id(),
                model: call.route.model.clone(),
                provider: call.route.provider.clone(),
                contract: object(&call.route.contract),
            },
            base_url: self.client.base.clone(),
            request: object(&call.request),
            files,
            take: call.take.iter().map(|&take| u64::from(take)).collect(),
            key: call.key.clone(),
            attempt: call.attempt,
        };
        let written = match handle {
            None => serde_json::to_value(params),
            Some(Value::Object(handle)) => serde_json::to_value(AdapterCollectParams::new(
                params,
                handle.clone().into_iter().collect(),
            )),
            Some(_) => return Err("the job's handle is not an object".into()),
        };
        written.map_err(|error| {
            format!(
                "the call cannot be written for the adapter host: {}",
                read_reason(&error)
            )
        })
    }

    /// `the adapter <name> answered something FX cannot read: <where>`.
    fn unreadable(&self, place: &str) -> String {
        format!(
            "the adapter {} answered something FX cannot read: {place}",
            self.name
        )
    }

    /// An answered outcome as the answer to check and store, its shape held first (module doc).
    fn answer(&self, call: &CallRequest, wire: AdapterAnswer) -> Result<Answer, Refusal> {
        let cost = cost(wire.cost_usd).map_err(Refusal::Unreadable)?;
        let mut files = IndexMap::with_capacity(wire.files.len());
        for (name, file) in wire.files {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(file.base64.as_bytes())
                .map_err(|_| {
                    Refusal::Unreadable(format!(
                        "files.{name}: its bytes are not base64 with padding"
                    ))
                })?;
            files.insert(
                name,
                StandInFile {
                    kind: file.kind,
                    bytes,
                },
            );
        }
        let mut answer = shape(&call.route.capability, files, wire.data)
            .map_err(|reason| Refusal::Shape { reason, cost })?;
        answer.cost = cost;
        answer.usage = wire.usage.map(|usage| {
            Box::new(Usage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                ..Usage::default()
            })
        });
        Ok(answer)
    }

    /// An `adapter.send` result as the send's outcome (module doc).
    fn sent(&self, call: &CallRequest, value: Value, redactor: &Redactor) -> Sent {
        let lost = |reason: String| Sent::Failed {
            reason: redactor.reason(&reason),
            cost: None,
            retryable: false,
        };
        let result = match serde_json::from_value::<AdapterSendResult>(value) {
            Ok(result) => result,
            Err(error) => return lost(self.unreadable(&read_reason(&error))),
        };
        let reason = |text: String| said(&text, redactor);
        match result {
            AdapterSendResult::Answered(answer) => match self.answer(call, answer) {
                Ok(answer) => Sent::Answered(answer),
                Err(Refusal::Unreadable(place)) => lost(self.unreadable(&place)),
                Err(Refusal::Shape { reason, cost }) => Sent::Failed {
                    reason: redactor.reason(&reason),
                    cost,
                    retryable: true,
                },
            },
            AdapterSendResult::NotReceived {
                reason: text,
                retry_after_s,
            } => match (reason(text), wait(retry_after_s)) {
                (Ok(reason), Ok(retry_after)) => Sent::NotReceived {
                    reason,
                    retry_after,
                },
                (Err(place), _) | (_, Err(place)) => lost(self.unreadable(&place)),
            },
            AdapterSendResult::Refused { reason: text } => match reason(text) {
                Ok(reason) => Sent::Refused { reason },
                Err(place) => lost(self.unreadable(&place)),
            },
            AdapterSendResult::Failed {
                reason: text,
                retryable,
                cost_usd,
            } => match (reason(text), cost(cost_usd)) {
                (Ok(reason), Ok(cost)) => Sent::Failed {
                    reason,
                    cost,
                    retryable,
                },
                (Err(place), _) | (_, Err(place)) => lost(self.unreadable(&place)),
            },
        }
    }

    /// An `adapter.submit` result as the submit's outcome (module doc).
    fn submitted(&self, value: Value, redactor: &Redactor) -> Submitted {
        let lost = |reason: String| Submitted::Uncertain {
            reason: redactor.reason(&reason),
        };
        let result = match serde_json::from_value::<AdapterSubmitResult>(value) {
            Ok(result) => result,
            Err(error) => return lost(self.unreadable(&read_reason(&error))),
        };
        let reason = |text: String| said(&text, redactor);
        match result {
            AdapterSubmitResult::Accepted { handle } => {
                let handle = Value::Object(handle.into_iter().collect());
                let text = grida_fx_core::value::canon(&handle);
                if self.secrets.iter().any(|secret| text.contains(secret)) {
                    return lost(format!(
                        "the adapter {} accepted the job with a handle that holds a key, which \
                         FX cannot keep",
                        self.name
                    ));
                }
                Submitted::Accepted { handle }
            }
            AdapterSubmitResult::Uncertain { reason: text } => match reason(text) {
                Ok(reason) => Submitted::Uncertain { reason },
                Err(place) => lost(self.unreadable(&place)),
            },
            AdapterSubmitResult::NotReceived {
                reason: text,
                retry_after_s,
            } => match (reason(text), wait(retry_after_s)) {
                (Ok(reason), Ok(retry_after)) => Submitted::NotReceived {
                    reason,
                    retry_after,
                },
                (Err(place), _) | (_, Err(place)) => lost(self.unreadable(&place)),
            },
            AdapterSubmitResult::Refused { reason: text } => match reason(text) {
                Ok(reason) => Submitted::Refused { reason },
                Err(place) => lost(self.unreadable(&place)),
            },
            AdapterSubmitResult::Failed {
                reason: text,
                retryable,
                cost_usd,
            } => match (reason(text), cost(cost_usd)) {
                (Ok(reason), Ok(cost)) => Submitted::Failed {
                    reason,
                    cost,
                    retryable,
                },
                (Err(place), _) | (_, Err(place)) => lost(self.unreadable(&place)),
            },
        }
    }

    /// An `adapter.collect` result as the collect's outcome (module doc).
    fn collected(&self, call: &CallRequest, value: Value, redactor: &Redactor) -> Collected {
        let lost = |reason: String| Collected::Unreachable {
            reason: redactor.reason(&reason),
        };
        let result = match serde_json::from_value::<AdapterCollectResult>(value) {
            Ok(result) => result,
            Err(error) => return lost(self.unreadable(&read_reason(&error))),
        };
        let reason = |text: String| said(&text, redactor);
        match result {
            AdapterCollectResult::Answered(answer) => match self.answer(call, answer) {
                Ok(answer) => Collected::Answered(answer),
                Err(Refusal::Unreadable(place)) => lost(self.unreadable(&place)),
                Err(Refusal::Shape { reason, cost }) => Collected::Ended {
                    reason: redactor.reason(&reason),
                    cost,
                },
            },
            AdapterCollectResult::Ended {
                reason: text,
                cost_usd,
            } => match (reason(text), cost(cost_usd)) {
                (Ok(reason), Ok(cost)) => Collected::Ended { reason, cost },
                (Err(place), _) | (_, Err(place)) => lost(self.unreadable(&place)),
            },
            AdapterCollectResult::Unreachable { reason: text } => match reason(text) {
                Ok(reason) => Collected::Unreachable { reason },
                Err(place) => lost(self.unreadable(&place)),
            },
        }
    }
}

impl Runtime for AdapterHost {
    fn shutdown(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if let State::Running(running) = &*state {
                let running = Arc::clone(running);
                if !running.end().await {
                    self.cleanup_verified.store(false, Ordering::SeqCst);
                }
            }
            *state = State::Idle;
            // A host ended apart: one that died, would not serve, or whose start its caller
            // abandoned.
            let endings = std::mem::take(&mut *locked(&self.endings));
            for ending in endings {
                if ending.await.is_err() {
                    self.cleanup_verified.store(false, Ordering::SeqCst);
                }
            }
            self.cleanup_verified.load(Ordering::SeqCst)
        })
    }
}

/// A reason as the host gave it, redacted; `Err` is where it cannot be read: a blank one.
fn said(text: &str, redactor: &Redactor) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("its reason is blank".into());
    }
    Ok(redactor.reason(text))
}

/// A reported cost (`cost_usd`), rounded up to whole micro-dollars; `Err` is where it cannot be
/// read.
fn cost(reported: Option<f64>) -> Result<Option<Usd>, String> {
    match reported {
        None => Ok(None),
        Some(usd) => grida_fx_providers::wire::usd_ceil(&Value::from(usd))
            .map(Some)
            .ok_or_else(|| "cost_usd is not a number of at least 0".into()),
    }
}

/// A wait the provider asked for (`retry_after_s`), at most a day
/// (`grida_fx_providers::adapter::RETRY_AFTER_LIMIT`); `Err` is where it cannot be read.
fn wait(asked: Option<f64>) -> Result<Option<Duration>, String> {
    match asked {
        None => Ok(None),
        Some(seconds) if seconds.is_finite() && seconds >= 0.0 => Ok(Some(
            Duration::try_from_secs_f64(seconds)
                .unwrap_or(grida_fx_providers::adapter::RETRY_AFTER_LIMIT)
                .min(grida_fx_providers::adapter::RETRY_AFTER_LIMIT),
        )),
        Some(_) => Err("retry_after_s is not a number of seconds of at least 0".into()),
    }
}

/// A host started but neither kept nor refused yet: ended apart ([`AdapterHost::end_apart`]) if
/// its caller is dropped first, as a run stopped while the adapter loads drops it.
struct Starting<'a> {
    host: &'a AdapterHost,
    running: Option<Arc<Running>>,
}

impl Starting<'_> {
    fn running(&self) -> &Running {
        self.running.as_deref().expect("a host being started")
    }

    /// The host, no longer ended when this is dropped.
    fn take(mut self) -> Arc<Running> {
        self.running.take().expect("a host being started")
    }
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            drop(self.host.end_apart(running));
        }
    }
}

/// A started host, watched from the moment it starts (module doc).
struct Running {
    connection: Connection,
    /// The process, when the engine started one; the watcher holds it while it waits.
    process: Arc<tokio::sync::Mutex<Option<HostProcess>>>,
    /// Set once the host is gone: its exit status when it was seen to exit.
    gone: Arc<watch::Sender<Option<Option<ExitStatus>>>>,
    /// Fired when the engine ends it: from then on, its going away is expected.
    ending: Cancel,
}

impl Running {
    /// `launched`, watched until it is gone or ended.
    fn watch(launched: Launched) -> Arc<Running> {
        let running = Arc::new(Running {
            connection: launched.connection,
            process: Arc::new(tokio::sync::Mutex::new(launched.process)),
            gone: Arc::new(watch::channel(None).0),
            ending: Cancel::new(),
        });
        let connection = running.connection.clone();
        let process = Arc::clone(&running.process);
        let gone = Arc::clone(&running.gone);
        let ending = running.ending.clone();
        tokio::spawn(async move {
            let seen = async {
                let mut process = process.lock().await;
                let Some(host) = process.as_mut() else {
                    connection.closed().await;
                    return None;
                };
                tokio::select! {
                    biased;
                    () = connection.closed() => {
                        tokio::time::timeout(GRACE, host.exited()).await.ok().flatten()
                    }
                    status = host.exited() => {
                        // Its group is killed by now, which closed the pipes a forked process
                        // held; what it wrote before it exited is still read.
                        if tokio::time::timeout(GRACE, connection.closed()).await.is_err() {
                            connection.abandon();
                        }
                        status
                    }
                }
            };
            tokio::select! {
                biased;
                () = ending.cancelled() => {}
                status = seen => {
                    gone.send_replace(Some(status));
                }
            }
        });
        running
    }

    /// Whether it may take another call.
    fn alive(&self) -> bool {
        self.connection.is_open() && self.gone.borrow().is_none()
    }

    /// The exit status of a host whose connection ended, once the watcher saw it go (at most
    /// twice the grace period).
    async fn status(&self) -> Option<ExitStatus> {
        let mut gone = self.gone.subscribe();
        let seen = tokio::time::timeout(2 * GRACE, gone.wait_for(Option::is_some)).await;
        match seen {
            Ok(Ok(status)) => (*status).flatten(),
            _ => None,
        }
    }

    /// Ends it: `shutdown`, `exit`, then a kill after 5 seconds. Whether its end is verified.
    /// The calls still waiting to learn how it went ([`Running::status`]) learn it then, since
    /// the watcher it stops no longer tells them.
    async fn end(&self) -> bool {
        self.ending.cancel();
        let process = self.process.lock().await.take();
        let (status, verified) = match process {
            Some(process) => process.shutdown_seen().await,
            None => {
                if self.connection.is_open() {
                    let connection = self.connection.clone();
                    let _ = tokio::time::timeout(GRACE, async move {
                        let _ = connection.request(method::SHUTDOWN, None).await;
                        let _ = connection.notify(method::EXIT, None).await;
                    })
                    .await;
                }
                let closed = tokio::time::timeout(GRACE, self.connection.close_input()).await;
                (None, closed.is_ok())
            }
        };
        self.gone.send_if_modified(|gone| {
            let unseen = gone.is_none();
            if unseen {
                *gone = Some(status);
            }
            unseen
        });
        verified
    }
}

// ---------------------------------------------------------------------------------------------
// The adapter

/// One capability of a project adapter, registered by its kind (module doc).
pub struct ProjectAdapter {
    host: Arc<AdapterHost>,
}

impl RequestAdapter for ProjectAdapter {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            if let Err(reason) = self.host.key() {
                return Sent::Refused { reason };
            }
            match self.host.ask(method::ADAPTER_SEND, call, None).await {
                Asked::Result(value, redactor) => self.host.sent(call, value, &redactor),
                Asked::Refused(reason) => Sent::Refused { reason },
                Asked::NotSent(reason) => Sent::not_received(reason),
                Asked::Lost(reason) => Sent::Failed {
                    reason,
                    cost: None,
                    retryable: false,
                },
            }
        })
    }

    fn identity(&self) -> Option<Identity> {
        Some(self.host.identity)
    }

    fn redactor(&self) -> Redactor {
        self.host.client.redactor.clone()
    }
}

impl LongJob for ProjectAdapter {
    fn submit<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Submitted> {
        Box::pin(async move {
            if let Err(reason) = self.host.key() {
                return Submitted::Refused { reason };
            }
            match self.host.ask(method::ADAPTER_SUBMIT, call, None).await {
                Asked::Result(value, redactor) => self.host.submitted(value, &redactor),
                Asked::Refused(reason) => Submitted::Refused { reason },
                Asked::NotSent(reason) => Submitted::not_received(reason),
                Asked::Lost(reason) => Submitted::Uncertain { reason },
            }
        })
    }

    fn collect<'a>(&'a self, call: &'a CallRequest, handle: &'a Value) -> BoxFuture<'a, Collected> {
        Box::pin(async move {
            match self
                .host
                .ask(method::ADAPTER_COLLECT, call, Some(handle))
                .await
            {
                Asked::Result(value, redactor) => self.host.collected(call, value, &redactor),
                Asked::Refused(reason) | Asked::NotSent(reason) | Asked::Lost(reason) => {
                    Collected::Unreachable { reason }
                }
            }
        })
    }

    fn identity(&self) -> Option<Identity> {
        Some(self.host.identity)
    }

    fn redactor(&self) -> Redactor {
        self.host.client.redactor.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Pending calls and `fetch`

/// The calls pending on a host, by `call_id`, and those the engine dropped.
#[derive(Default)]
struct Calls {
    pending: Mutex<HashMap<String, Arc<Pending>>>,
    dropped: Mutex<HashSet<String>>,
}

/// One pending call: its cancellation, and the URLs it downloaded through `fetch`.
#[derive(Default)]
struct Pending {
    cancel: Cancel,
    downloads: Mutex<Vec<String>>,
}

impl Calls {
    /// Opens `call_id`'s entry, closed when the guard drops.
    fn open(self: &Arc<Self>, call_id: &str) -> CallGuard {
        let pending = Arc::new(Pending::default());
        locked(&self.pending).insert(call_id.to_string(), Arc::clone(&pending));
        CallGuard {
            calls: Arc::clone(self),
            id: call_id.to_string(),
            pending,
            done: false,
        }
    }

    /// The pending call `call_id`; `Err(true)` when the engine dropped it.
    fn get(&self, call_id: &str) -> Result<Arc<Pending>, bool> {
        if let Some(pending) = locked(&self.pending).get(call_id) {
            return Ok(Arc::clone(pending));
        }
        Err(locked(&self.dropped).contains(call_id))
    }
}

/// A pending call's entry: dropped with the call, which cancels what it still runs.
struct CallGuard {
    calls: Arc<Calls>,
    id: String,
    pending: Arc<Pending>,
    /// Whether the host answered; a call dropped before is the engine's cancellation.
    done: bool,
}

impl CallGuard {
    /// `base` with every URL the call downloaded added.
    fn redactor(&self, base: &Redactor) -> Redactor {
        locked(&self.pending.downloads)
            .iter()
            .fold(base.clone(), |redactor, url| redactor.with(url))
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.pending.cancel.cancel();
        locked(&self.calls.pending).remove(&self.id);
        if !self.done {
            locked(&self.calls.dropped).insert(self.id.clone());
        }
    }
}

/// Serves an adapter host's `fetch` requests (module doc).
struct Fetches {
    calls: Arc<Calls>,
    client: Client,
}

impl Incoming for Fetches {
    fn request(
        &self,
        method: String,
        params: Option<Value>,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        let calls = Arc::clone(&self.calls);
        let client = self.client.clone();
        Box::pin(async move {
            if method != method::FETCH {
                return Err(RpcError::new(
                    ErrorCode::MethodNotFound,
                    format!("the engine serves no {method} to an adapter host"),
                ));
            }
            let params: FetchParams = serde_json::from_value(params.unwrap_or(Value::Null))
                .map_err(|error| invalid(&read_reason(&error)))?;
            let body = checked_body(&params)?;
            let pending = calls.get(&params.call_id).map_err(|dropped| {
                if dropped {
                    cancelled()
                } else {
                    invalid(&format!("no adapter call {} is pending", params.call_id))
                }
            })?;
            if pending.cancel.is_cancelled() {
                return Err(cancelled());
            }
            let result = match exchange(&client, &pending, params, body) {
                Err(reason) => failed(FetchPhase::Refused, reason),
                Ok(request) => {
                    let sent = tokio::select! {
                        biased;
                        () = pending.cancel.cancelled() => return Err(cancelled()),
                        sent = client.send(request) => sent,
                    };
                    match sent {
                        Ok(response) => FetchResult::Response {
                            status: response.status,
                            headers: response.headers,
                            base64: base64::engine::general_purpose::STANDARD
                                .encode(&response.body),
                        },
                        Err(error) => {
                            let phase = match (error.kind, error.phase) {
                                (TransportErrorKind::Refused, _) => FetchPhase::Refused,
                                (_, Phase::NotSent) => FetchPhase::NotSent,
                                (_, Phase::AfterSend) => FetchPhase::AfterSend,
                            };
                            failed(phase, error.reason)
                        }
                    }
                }
            };
            serde_json::to_value(result).map_err(|error| {
                RpcError::new(
                    ErrorCode::Internal,
                    format!(
                        "the fetch result cannot be written: {}",
                        read_reason(&error)
                    ),
                )
            })
        })
    }

    fn notify(&self, _method: String, _params: Option<Value>) {}
}

fn invalid(message: &str) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn cancelled() -> RpcError {
    RpcError::new(ErrorCode::Cancelled, "the adapter call was dropped")
}

fn failed(phase: FetchPhase, reason: String) -> FetchResult {
    FetchResult::Failed {
        error: FetchFailure { phase, reason },
    }
}

/// The params serde cannot hold a `fetch` to (spec/protocol.md §6.8), refused with `-32602`;
/// the body to send.
fn checked_body(params: &FetchParams) -> Result<Body, RpcError> {
    if let Some(timeout) = params.timeout_s
        && !(timeout.is_finite() && timeout > 0.0 && timeout <= FETCH_TIMEOUT.as_secs_f64())
    {
        return Err(invalid("timeout_s is above 0 and at most 300"));
    }
    if let Some(cap) = params.max_response_bytes
        && !(1..=FETCH_MAX_BYTES).contains(&cap)
    {
        return Err(invalid("max_response_bytes is from 1 to 67108864"));
    }
    let token = |name: &str| {
        !name.is_empty()
            && name.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b"!#$%&'*+-.^_`|~".contains(&b)
            })
    };
    if let Some(headers) = &params.headers
        && let Some((name, _)) = headers.iter().find(|(name, _)| !token(name))
    {
        return Err(invalid(&format!(
            "the header name {} is not a lowercase token",
            Value::from(name.as_str())
        )));
    }
    match (&params.body, params.method) {
        (None, _) => Ok(Body::Empty),
        (Some(_), FetchMethod::Get) => Err(invalid("a GET has no body")),
        (Some(FetchBody::Json { json }), FetchMethod::Post) => Ok(Body::Json(json.clone())),
        (
            Some(FetchBody::Bytes {
                base64,
                content_type,
            }),
            FetchMethod::Post,
        ) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(base64.as_bytes())
                .map_err(|_| invalid("body.base64 is not base64 with padding"))?;
            Ok(Body::Bytes {
                content_type: content_type.clone(),
                bytes,
            })
        }
    }
}

/// The request a `fetch` asks for, or why the engine refuses it before it leaves (module doc).
fn exchange(
    client: &Client,
    pending: &Pending,
    params: FetchParams,
    body: Body,
) -> Result<HttpRequest, String> {
    let lane = match params.lane {
        FetchLane::Provider => Lane::Provider,
        FetchLane::Upload => Lane::Upload,
        FetchLane::Download => Lane::Download,
    };
    let method = match params.method {
        FetchMethod::Get => Method::Get,
        FetchMethod::Post => Method::Post,
    };
    let url = params.url;
    let mut request = HttpRequest::new(method, url.clone(), lane).body(body);
    if let Some(timeout) = params.timeout_s {
        request = request.timeout(Duration::from_secs_f64(timeout));
    }
    if let Some(cap) = params.max_response_bytes {
        request = request.max_response_bytes(cap);
    }
    let own_header = client
        .declared
        .as_ref()
        .map(|declared| declared.header.as_str());
    for (name, value) in params.headers.unwrap_or_default() {
        if own_header.is_some_and(|own| own.eq_ignore_ascii_case(&name)) {
            return Err(format!(
                "the {name} header is {}'s credential, which the engine attaches when asked",
                client.provider
            ));
        }
        request = request.header(&name, &value);
    }
    if lane == Lane::Download {
        if params.credential == Some(true) {
            return Err("a download never carries a credential".into());
        }
        client.download_url(&url).map_err(|_| {
            format!(
                "the download is not on {}'s download hosts",
                client.provider
            )
        })?;
        locked(&pending.downloads).push(url);
        return Ok(request);
    }
    if params.credential == Some(true) {
        if !grida_fx_providers::wire::same_origin(&url, &client.base) {
            return Err(format!(
                "{}'s credential goes only to the origin of its base URL",
                client.provider
            ));
        }
        if let Some(credential) = client.optional_credential()? {
            request = request.credential(credential);
        }
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn costs_and_waits_are_read_or_refused() {
        assert_eq!(cost(None), Ok(None));
        assert_eq!(cost(Some(0.04)), Ok(Some(Usd(40_000))));
        assert_eq!(cost(Some(1e-7)), Ok(Some(Usd(1))));
        assert!(cost(Some(-0.5)).is_err());
        assert_eq!(wait(Some(2.5)), Ok(Some(Duration::from_millis(2500))));
        assert_eq!(
            wait(Some(1e12)),
            Ok(Some(grida_fx_providers::adapter::RETRY_AFTER_LIMIT))
        );
        assert!(wait(Some(-1.0)).is_err());
        assert_eq!(
            said("  ", &Redactor::default()),
            Err("its reason is blank".into())
        );
    }

    #[test]
    fn fetch_params_serde_cannot_hold_are_refused() {
        let params = |extra: Value| -> FetchParams {
            let mut base = json!({"call_id": "c1", "method": "POST", "url": "https://a.test",
                                  "lane": "provider"});
            for (key, value) in extra.as_object().unwrap() {
                base[key] = value.clone();
            }
            serde_json::from_value(base).unwrap()
        };
        let refused = |extra: Value| checked_body(&params(extra)).unwrap_err().message;
        assert_eq!(
            refused(json!({"method": "GET", "body": {"json": 1}})),
            "a GET has no body"
        );
        assert_eq!(
            refused(json!({"timeout_s": 0})),
            "timeout_s is above 0 and at most 300"
        );
        assert_eq!(
            refused(json!({"timeout_s": 300.5})),
            "timeout_s is above 0 and at most 300"
        );
        assert_eq!(
            refused(json!({"max_response_bytes": 0})),
            "max_response_bytes is from 1 to 67108864"
        );
        assert_eq!(
            refused(json!({"max_response_bytes": 67108865})),
            "max_response_bytes is from 1 to 67108864"
        );
        assert_eq!(
            refused(json!({"headers": [["Accept", "x"]]})),
            "the header name \"Accept\" is not a lowercase token"
        );
        assert_eq!(
            refused(json!({"body": {"base64": "not base64!", "content_type": "image/png"}})),
            "body.base64 is not base64 with padding"
        );
        assert_eq!(
            checked_body(&params(
                json!({"body": {"base64": "AAE=", "content_type": "a/b"}})
            ))
            .unwrap(),
            Body::Bytes {
                content_type: "a/b".into(),
                bytes: vec![0, 1]
            }
        );
        assert_eq!(
            checked_body(&params(json!({"timeout_s": 300, "max_response_bytes": 1}))).unwrap(),
            Body::Empty
        );
    }
}
