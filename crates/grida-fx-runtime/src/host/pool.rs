//! A pool of node hosts for running bodies (spec/protocol.md §1 "One job per host").
//!
//! A host handles one `run` at a time; to run nodes in parallel the engine starts more hosts and
//! reuses them. [`HostPool::lease`] hands out an idle host, or starts one while fewer than `max`
//! exist, or waits for one to come back (leases are handed out in the order they were asked
//! for). A [`HostLease`] returns its host to the pool when dropped, unless the host was killed,
//! its connection broke, it exited, or a `run` it was sent was never answered. A host that could
//! not be started (no interpreter, another protocol) is not started again: every later lease
//! gets the same sentence.
//!
//! Each pool host's [`Incoming`] is a router: a host request whose `run_id` is the run the host is
//! running goes to that run's [`RunRequests`]; any other `run_id` is answered `-32602` (`no run
//! <id> is pending on this host`) and such a notification is ignored (spec/protocol.md §6).
//!
//! [`HostLease::run`] sends `run` and waits for its answer:
//! - at `timeout` or when `cancel` fires: the run's requests are stopped first
//!   ([`RunRequests::stop`]: from then on they are answered `cancelled`, so nothing of the run
//!   makes a request past its deadline), then `$/cancel {id}`; a host that has not answered 5
//!   seconds later is killed with its process group ([`RunReply::TimedOut`] /
//!   [`RunReply::Cancelled`]);
//!   an answer that arrives after the cancel is discarded (a host that answered in time stays in
//!   the pool);
//! - a host that exits while the run is pending: [`RunReply::Exited`] with its status, seen by
//!   waiting on the process (a process the body forked may keep the host's output open); its
//!   process group is killed then;
//! - otherwise the result or the error, as the host gave it. A host that breaks the protocol is
//!   killed and its run ends with an `internal` error naming what it sent.
//!
//! The run is unregistered from the router before `run` returns (also when its future is
//! dropped), so later host requests for it are answered `-32602`.
//!
//! [`HostPool::shutdown`] ends every idle host politely (`HostProcess::shutdown`); the pool can
//! still lease hosts afterwards. Dropping the pool kills the hosts it still holds.

use super::GRACE;
use super::connection::{ConnectionError, Incoming};
use super::process::{HostProcess, HostSpec};
use crate::engine::Cancel;
use grida_fx_protocol::{ErrorCode, RpcError, RunParams, method};
use grida_fx_providers::BoxFuture;
use serde_json::Value;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Serves the host requests of one run (implemented by `executor::requests`).
pub trait RunRequests: Send + Sync {
    /// A host request carrying this run's `run_id`.
    fn request(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, RpcError>>;

    /// A host notification carrying this run's `run_id` (`progress`).
    fn notify(&self, method: String, params: Value);

    /// The run is being stopped (its deadline passed, or it was cancelled): every later request
    /// is answered `cancelled`. Called before the host is sent `$/cancel`.
    fn stop(&self);
}

/// How a `run` request ended.
#[derive(Debug, Clone, PartialEq)]
pub enum RunReply {
    /// The host's result (a `run_result`, not yet checked).
    Result(Value),
    /// The host's error (`node_failure`, `node_error`, `load_failed`, an engine error passed on).
    Error(RpcError),
    /// `timeout_s` passed.
    TimedOut,
    /// The run was stopped.
    Cancelled,
    /// The host exited while the run was pending.
    Exited(Option<ExitStatus>),
}

/// The node hosts of one invocation.
pub struct HostPool {
    spec: HostSpec,
    /// One permit per lease: at most `max` hosts exist, since a host is started only for a
    /// lease that found no idle one.
    leases: Arc<Semaphore>,
    state: Mutex<PoolState>,
    /// How long a host gets to answer `$/cancel`, to take `shutdown`, or to exit (5 seconds;
    /// shorter in tests).
    grace: Duration,
}

struct PoolState {
    /// Hosts waiting for a lease, the most recently returned last.
    idle: Vec<Idle>,
    /// Why hosts cannot be started, once one failed to start.
    failure: Option<String>,
}

/// An idle host and the router its connection serves.
struct Idle {
    process: HostProcess,
    router: Arc<Router>,
}

impl HostPool {
    /// A pool of at most `max` hosts (at least 1). Starts nothing.
    pub fn new(spec: HostSpec, max: usize) -> HostPool {
        let max = max.max(1);
        HostPool {
            spec,
            leases: Arc::new(Semaphore::new(max)),
            state: Mutex::new(PoolState {
                idle: Vec::new(),
                failure: None,
            }),
            grace: GRACE,
        }
    }

    /// The same pool with another grace period.
    #[cfg(test)]
    pub(crate) fn with_grace(mut self, grace: Duration) -> HostPool {
        self.grace = grace;
        self
    }

    /// The default size: the machine's available parallelism, at most 8.
    pub fn default_size() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(1)
    }

    pub fn spec(&self) -> &HostSpec {
        &self.spec
    }

    fn state(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An idle host, a new one, or the next one returned (module doc).
    pub async fn lease(self: &Arc<Self>) -> Result<HostLease, String> {
        let permit = Arc::clone(&self.leases)
            .acquire_owned()
            .await
            .map_err(|_| "the node host pool is closed".to_string())?;
        loop {
            let idle = self.state().idle.pop();
            let Some(mut idle) = idle else { break };
            if idle.process.is_healthy() {
                return Ok(HostLease {
                    pool: Arc::clone(self),
                    host: Some(idle.process),
                    router: idle.router,
                    permit: Some(permit),
                    spent: false,
                });
            }
            idle.process.kill_now().await;
        }
        if let Some(failure) = self.state().failure.clone() {
            return Err(failure);
        }
        let router = Arc::new(Router::new());
        let incoming: Arc<dyn Incoming> = router.clone();
        match HostProcess::start_within(&self.spec, incoming, self.grace).await {
            Ok(process) => Ok(HostLease {
                pool: Arc::clone(self),
                host: Some(process),
                router,
                permit: Some(permit),
                spent: false,
            }),
            Err(failure) => {
                self.state().failure = Some(failure.clone());
                Err(failure)
            }
        }
    }

    /// Ends every idle host politely.
    pub async fn shutdown(&self) {
        let idle = std::mem::take(&mut self.state().idle);
        let mut ending = tokio::task::JoinSet::new();
        let grace = self.grace;
        for mut idle in idle {
            ending.spawn(async move { idle.process.end(true, grace).await });
        }
        while ending.join_next().await.is_some() {}
    }
}

/// A host leased for one run.
pub struct HostLease {
    pool: Arc<HostPool>,
    host: Option<HostProcess>,
    router: Arc<Router>,
    /// Released after the host went back to the pool.
    permit: Option<OwnedSemaphorePermit>,
    /// The host may not serve another run: it was killed, or a `run` it was sent is unanswered.
    spent: bool,
}

impl HostLease {
    /// The leased host (for `tool.invoke` and `agent.check` while its run is pending).
    pub fn host(&self) -> &HostProcess {
        self.host
            .as_ref()
            .expect("a lease holds its host until dropped")
    }

    /// Sends `run` and waits (module doc). `requests` serves the run's host requests.
    pub async fn run(
        &mut self,
        params: &RunParams,
        requests: Arc<dyn RunRequests>,
        cancel: &Cancel,
        timeout: Option<Duration>,
    ) -> RunReply {
        if cancel.is_cancelled() {
            return RunReply::Cancelled;
        }
        let value = match serde_json::to_value(params) {
            Ok(value) => value,
            Err(e) => {
                return RunReply::Error(RpcError::new(
                    ErrorCode::Internal,
                    format!("the run request cannot be written: {e}"),
                ));
            }
        };
        let Some(connection) = self.host.as_ref().map(|host| host.connection().clone()) else {
            return RunReply::Error(RpcError::new(
                ErrorCode::Internal,
                "the lease holds no node host",
            ));
        };
        let stopping = Arc::clone(&requests);
        let _registered = Registration::new(&self.router, &params.run_id, requests);
        // Until the host answers, it is busy with this run.
        self.spent = true;
        let grace = self.pool.grace;
        let stop = Cancel::new();
        let request = connection.request_cancellable(method::RUN, Some(value), &stop);
        tokio::pin!(request);
        let deadline = async {
            match timeout {
                Some(timeout) => tokio::time::sleep(timeout).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(deadline);
        let Some(host) = self.host.as_mut() else {
            return RunReply::Error(RpcError::new(
                ErrorCode::Internal,
                "the lease holds no node host",
            ));
        };
        let stopped = tokio::select! {
            biased;
            answer = host.answer_or_exit(request.as_mut(), grace) => {
                return self.answered(answer).await;
            }
            _ = cancel.cancelled() => RunReply::Cancelled,
            _ = &mut deadline => RunReply::TimedOut,
        };
        // spec/protocol.md §5.3: the run's requests end first, then `$/cancel`, and the host has
        // the grace period to answer.
        stopping.stop();
        stop.cancel();
        let Some(host) = self.host.as_mut() else {
            return stopped;
        };
        let answered =
            tokio::time::timeout(grace, host.answer_or_exit(request.as_mut(), grace)).await;
        match answered {
            // An answer after the cancel is discarded; the host is free again unless it exited.
            Ok(Ok(_)) | Ok(Err(ConnectionError::Rpc(_))) => self.spent = host.has_ended(),
            Ok(Err(ConnectionError::Closed)) => {
                host.end(false, grace).await;
            }
            Ok(Err(ConnectionError::Protocol(_))) | Err(_) => {
                host.kill_now().await;
            }
        }
        stopped
    }

    /// The reply for an answer that arrived before any cancel.
    async fn answered(&mut self, answer: Result<Value, ConnectionError>) -> RunReply {
        let grace = self.pool.grace;
        // A host that answered and then exited serves no other run.
        let exited = self.host.as_ref().is_none_or(HostProcess::has_ended);
        match answer {
            Ok(value) => {
                self.spent = exited;
                RunReply::Result(value)
            }
            Err(ConnectionError::Rpc(error)) => {
                self.spent = exited;
                RunReply::Error(error)
            }
            Err(ConnectionError::Closed) => {
                let status = match self.host.as_mut() {
                    Some(host) => host.end(false, grace).await,
                    None => None,
                };
                RunReply::Exited(status)
            }
            Err(ConnectionError::Protocol(message)) => {
                if let Some(host) = self.host.as_mut() {
                    host.kill_now().await;
                }
                RunReply::Error(RpcError::new(
                    ErrorCode::Internal,
                    format!("the node host broke the protocol: {message}"),
                ))
            }
        }
    }
}

impl Drop for HostLease {
    fn drop(&mut self) {
        if let Some(mut host) = self.host.take() {
            if !self.spent && host.is_healthy() {
                self.pool.state().idle.push(Idle {
                    process: host,
                    router: Arc::clone(&self.router),
                });
            } else if !host.has_ended()
                && let Ok(runtime) = tokio::runtime::Handle::try_current()
            {
                runtime.spawn(async move {
                    host.kill_now().await;
                });
            }
            // Otherwise dropping the process kills it with its group.
        }
        drop(self.permit.take());
    }
}

/// Registers a run in a router for as long as it lives.
struct Registration {
    router: Arc<Router>,
    run_id: String,
}

impl Registration {
    fn new(router: &Arc<Router>, run_id: &str, requests: Arc<dyn RunRequests>) -> Registration {
        router.set(run_id.to_string(), requests);
        Registration {
            router: Arc::clone(router),
            run_id: run_id.to_string(),
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.router.clear(&self.run_id);
    }
}

/// The router a pool host's connection serves (module doc).
pub(crate) struct Router {
    /// The run the host is running, with what serves its requests.
    current: Mutex<Option<(String, Arc<dyn RunRequests>)>>,
}

impl Router {
    pub(crate) fn new() -> Router {
        Router {
            current: Mutex::new(None),
        }
    }

    fn current(&self) -> MutexGuard<'_, Option<(String, Arc<dyn RunRequests>)>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Routes requests carrying `run_id` to `requests`, in place of any earlier run.
    pub(crate) fn set(&self, run_id: String, requests: Arc<dyn RunRequests>) {
        *self.current() = Some((run_id, requests));
    }

    /// Stops routing `run_id` (another run registered since is kept).
    pub(crate) fn clear(&self, run_id: &str) {
        let mut current = self.current();
        if current.as_ref().is_some_and(|(id, _)| id == run_id) {
            *current = None;
        }
    }

    /// What serves a message with these params, or the sentence refusing it.
    fn route(&self, method: &str, params: Option<&Value>) -> Result<Arc<dyn RunRequests>, String> {
        let Some(run_id) = params
            .and_then(|params| params.get("run_id"))
            .and_then(Value::as_str)
        else {
            return Err(format!("{method} names no run: its params have no run_id"));
        };
        match &*self.current() {
            Some((current, requests)) if current == run_id => Ok(Arc::clone(requests)),
            _ => Err(format!("no run {run_id} is pending on this host")),
        }
    }
}

impl Incoming for Router {
    fn request(
        &self,
        method: String,
        params: Option<Value>,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        match self.route(&method, params.as_ref()) {
            Ok(requests) => requests.request(method, params.unwrap_or(Value::Null)),
            Err(refusal) => {
                Box::pin(async move { Err(RpcError::new(ErrorCode::InvalidParams, refusal)) })
            }
        }
    }

    fn notify(&self, method: String, params: Option<Value>) {
        if let Ok(requests) = self.route(&method, params.as_ref()) {
            requests.notify(method, params.unwrap_or(Value::Null));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Answers every request with its method and params; records notifications and stops.
    #[derive(Default)]
    struct Echo {
        notes: Mutex<Vec<(String, Value)>>,
        stopped: std::sync::atomic::AtomicBool,
    }

    impl RunRequests for Echo {
        fn request(
            self: Arc<Self>,
            method: String,
            params: Value,
        ) -> BoxFuture<'static, Result<Value, RpcError>> {
            Box::pin(async move { Ok(json!({"method": method, "params": params})) })
        }

        fn notify(&self, method: String, params: Value) {
            self.notes.lock().unwrap().push((method, params));
        }

        fn stop(&self) {
            self.stopped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn the_router_serves_the_current_run_only() {
        let router = Router::new();
        let echo = Arc::new(Echo::default());
        let refused = |result: Result<Value, RpcError>| {
            let error = result.unwrap_err();
            assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
            error.message
        };
        // No run yet.
        assert_eq!(
            refused(
                router
                    .request("fact".into(), Some(json!({"run_id": "r1"})))
                    .await
            ),
            "no run r1 is pending on this host"
        );
        router.set("r1".into(), echo.clone());
        assert_eq!(
            router
                .request("fact".into(), Some(json!({"run_id": "r1", "name": "n"})))
                .await
                .unwrap(),
            json!({"method": "fact", "params": {"run_id": "r1", "name": "n"}})
        );
        assert_eq!(
            refused(
                router
                    .request("fact".into(), Some(json!({"run_id": "r2"})))
                    .await
            ),
            "no run r2 is pending on this host"
        );
        assert_eq!(
            refused(
                router
                    .request("fact".into(), Some(json!({"name": "n"})))
                    .await
            ),
            "fact names no run: its params have no run_id"
        );
        assert_eq!(
            refused(
                router
                    .request("fact".into(), Some(json!({"run_id": 1})))
                    .await
            ),
            "fact names no run: its params have no run_id"
        );
        assert_eq!(
            refused(router.request("fact".into(), None).await),
            "fact names no run: its params have no run_id"
        );

        router.notify(
            "progress".into(),
            Some(json!({"run_id": "r2", "text": "x"})),
        );
        router.notify("progress".into(), Some(json!({"text": "x"})));
        router.notify(
            "progress".into(),
            Some(json!({"run_id": "r1", "text": "half"})),
        );
        assert_eq!(
            *echo.notes.lock().unwrap(),
            [(
                "progress".to_string(),
                json!({"run_id": "r1", "text": "half"})
            )]
        );

        // Clearing another run keeps the current one; clearing it ends its routing.
        router.clear("r2");
        assert!(
            router
                .request("fact".into(), Some(json!({"run_id": "r1"})))
                .await
                .is_ok()
        );
        router.clear("r1");
        assert_eq!(
            refused(
                router
                    .request("fact".into(), Some(json!({"run_id": "r1"})))
                    .await
            ),
            "no run r1 is pending on this host"
        );
    }

    #[test]
    fn a_registration_ends_with_its_scope() {
        let router = Arc::new(Router::new());
        {
            let _registered = Registration::new(&router, "r1", Arc::new(Echo::default()));
            assert!(router.route("fact", Some(&json!({"run_id": "r1"}))).is_ok());
        }
        assert!(
            router
                .route("fact", Some(&json!({"run_id": "r1"})))
                .is_err()
        );
    }

    #[test]
    fn pools_have_at_least_one_host() {
        let spec = HostSpec {
            python: "python3".into(),
            label: "python3".into(),
            project_root: "/work/acme".into(),
            sources: Vec::new(),
        };
        let pool = HostPool::new(spec.clone(), 0);
        assert_eq!(pool.leases.available_permits(), 1);
        assert_eq!(pool.spec(), &spec);
        assert_eq!(pool.grace, GRACE);
        assert!((1..=8).contains(&HostPool::default_size()));
    }

    #[test]
    fn lease_and_run_futures_are_sendable() {
        fn send<T: Send>(_: &T) {}
        let spec = HostSpec {
            python: "python3".into(),
            label: "python3".into(),
            project_root: "/work/acme".into(),
            sources: Vec::new(),
        };
        let pool = Arc::new(HostPool::new(spec, 1));
        send(&pool.lease());
        send(&pool.shutdown());
        fn run_is_send(lease: &mut HostLease, params: &RunParams, cancel: &Cancel) {
            let requests: Arc<dyn RunRequests> = Arc::new(Echo::default());
            send(&lease.run(params, requests, cancel, None));
        }
        let _ = run_is_send;
    }

    /// A host that answers `initialize` and then ignores everything, `$/cancel` included.
    const DEAF: &str = r#"
import json, sys, time
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))
message = read()
body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {
    "protocol": message["params"]["protocol"],
    "host": {"language": "python", "version": "3", "sdk_version": "0"}}}).encode()
sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
sys.stdout.buffer.flush()
with open("ran.txt", "a") as f:
    f.write("started\n")
while read() is not None:
    pass
time.sleep(60)
"#;

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_that_ignores_cancel_is_killed_after_the_grace() {
        use grida_fx_protocol::{RunBody, RunInstance};
        use indexmap::IndexMap;
        use std::time::Instant;
        let found = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !found {
            eprintln!("skipped: python3 is not on PATH");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("deaf.py"), DEAF).unwrap();
        let python = dir.path().join("python-deaf");
        std::fs::write(
            &python,
            "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/deaf.py\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = HostSpec {
            python,
            label: "python-deaf".into(),
            project_root: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        let pool = Arc::new(HostPool::new(spec, 1).with_grace(Duration::from_millis(300)));
        let params = RunParams {
            run_id: "r1".into(),
            instance: RunInstance {
                id: "a".into(),
                path: "a".into(),
                step: "a".into(),
                key: None,
                take: Vec::new(),
            },
            type_: "t".into(),
            body: RunBody::Builtin {
                builtin: "fx/echo@1".into(),
            },
            params: IndexMap::new(),
            param_files: IndexMap::new(),
            inputs: IndexMap::new(),
            work_dir: dir.path().display().to_string(),
            resources: IndexMap::new(),
            tools: IndexMap::new(),
            calls: IndexMap::new(),
            timeout_s: Some(0.2),
        };
        let mut lease = pool.lease().await.unwrap();
        let connection = lease.host().connection().clone();
        let start = Instant::now();
        let requests = Arc::new(Echo::default());
        let reply = lease
            .run(
                &params,
                Arc::clone(&requests) as Arc<dyn RunRequests>,
                &Cancel::new(),
                Some(Duration::from_millis(200)),
            )
            .await;
        let took = start.elapsed();
        assert_eq!(reply, RunReply::TimedOut);
        // The deadline stopped the run's requests.
        assert!(requests.stopped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(took >= Duration::from_millis(500), "{took:?}");
        assert!(took < Duration::from_secs(4), "{took:?}");
        // Killed: its connection closed, and it does not go back to the pool.
        connection.closed().await;
        drop(lease);
        assert!(pool.state().idle.is_empty());
        let _second = pool.lease().await.unwrap();
        let started = std::fs::read_to_string(dir.path().join("ran.txt")).unwrap();
        assert_eq!(started.lines().count(), 2, "a new host was started");

        // A run cancelled by the caller ends the same way.
        let mut lease = _second;
        let cancel = Cancel::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });
        let reply = lease
            .run(&params, Arc::new(Echo::default()), &cancel, None)
            .await;
        assert_eq!(reply, RunReply::Cancelled);
        assert!(!lease.host.as_mut().unwrap().is_healthy());
        // A token cancelled before the run sends nothing.
        assert_eq!(
            lease
                .run(&params, Arc::new(Echo::default()), &cancel, None)
                .await,
            RunReply::Cancelled
        );
    }
}
