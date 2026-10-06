//! The node host pool against real processes (spec/protocol.md §1 "One job per host", §5.3, §5.6,
//! §6).
//!
//! The hosts are a fake: a stdlib-only Python program written to a temporary folder and started
//! through a small shell script named as the interpreter, so the engine still runs
//! `<python> -P -m grida.fx.host`. What a `run` does is chosen by its `mode` param. The tests
//! need `python3` on `PATH` and skip without it.
#![cfg(unix)]

use grida_fx_protocol::{ErrorCode, RpcError, RunBody, RunInstance, RunParams};
use grida_fx_providers::BoxFuture;
use grida_fx_runtime::engine::Cancel;
use grida_fx_runtime::host::connection::Connection;
use grida_fx_runtime::host::pool::{HostPool, RunReply, RunRequests};
use grida_fx_runtime::host::process::HostSpec;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The fake host. Each process logs what it receives to `host-<pid>.log` in the project root.
const FAKE_HOST: &str = r#"
import json
import os
import subprocess
import sys
import time

PROTOCOL_IN = sys.stdin.buffer
PROTOCOL_OUT = sys.stdout.buffer
PID = os.getpid()
cancelled = set()
next_id = [0]


def log(event):
    with open("host-%d.log" % PID, "a") as f:
        f.write(event + "\n")


def read():
    length = None
    while True:
        line = PROTOCOL_IN.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(PROTOCOL_IN.read(length).decode("utf-8"))


def write(message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    PROTOCOL_OUT.write(b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body)
    PROTOCOL_OUT.flush()


def answer(request, result):
    write({"jsonrpc": "2.0", "id": request["id"], "result": result})


def fail(request, code, message):
    write({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}})


def serve(message):
    """Serves an engine message that arrives while the host waits for something else."""
    method = message.get("method")
    log(method or "response")
    if method == "tool.invoke":
        answer(message, {"content": "looked at " + message["params"]["name"]})
    elif method == "$/cancel":
        cancelled.add(message["params"]["id"])
    elif method is not None and "id" in message:
        fail(message, -32601, "no " + method)


def request(method, params):
    """Asks the engine and waits for its answer, serving the engine meanwhile."""
    next_id[0] += 1
    mine = next_id[0]
    write({"jsonrpc": "2.0", "id": mine, "method": method, "params": params})
    while True:
        message = read()
        if message is None:
            sys.exit(5)
        if "method" not in message and message.get("id") == mine:
            return message
        serve(message)


def run(message):
    params = message["params"]
    run_id = params["run_id"]
    mode = params["params"]["mode"]
    if mode == "echo":
        answer(message, {"outputs": {}, "facts": {"pid": PID, "run_id": run_id}})
    elif mode in ("fact", "other-run"):
        named = run_id if mode == "fact" else "elsewhere"
        reply = request("fact", {"run_id": named, "name": "seen", "value": 1})
        write({"jsonrpc": "2.0", "method": "progress", "params": {"run_id": named, "text": "half"}})
        answer(message, {"outputs": {}, "facts": {"pid": PID, "reply": reply}})
    elif mode == "sleep":
        time.sleep(params["params"]["seconds"])
        answer(message, {"outputs": {}, "facts": {"pid": PID}})
    elif mode == "cancel":
        while message["id"] not in cancelled:
            other = read()
            if other is None:
                sys.exit(5)
            serve(other)
        fail(message, -32002, "the run was cancelled")
    elif mode == "ignore-cancel":
        child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
        with open("grandchild-%d.txt" % PID, "w") as f:
            f.write(str(child.pid))
        while True:
            other = read()
            if other is None:
                time.sleep(60)
            serve(other)
    elif mode == "exit":
        sys.exit(7)
    elif mode == "fork-exit":
        child = os.fork()
        if child == 0:
            time.sleep(30)
            os._exit(0)
        with open("forked-%d.txt" % PID, "w") as f:
            f.write(str(child))
        os._exit(3)
    elif mode == "leave-child":
        child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
        with open("left-%d.txt" % PID, "w") as f:
            f.write(str(child.pid))
        answer(message, {"outputs": {}, "facts": {"pid": PID}})
    elif mode == "fail":
        fail(message, -32000, "the body failed on purpose")
    elif mode == "garbage":
        PROTOCOL_OUT.write(b"Content-Length: 9\r\n\r\nnot json!")
        PROTOCOL_OUT.flush()


def main():
    with open("host-%d.pid" % PID, "w") as f:
        f.write(str(PID))
    shut_down = False
    while True:
        message = read()
        if message is None:
            log("eof")
            sys.exit(0)
        method = message.get("method")
        if method == "initialize":
            log(method)
            answer(message, {"protocol": message["params"]["protocol"],
                             "host": {"language": "python", "version": "3", "sdk_version": "0.0.0-fake"}})
        elif method == "run":
            log(method)
            run(message)
        elif method == "shutdown":
            log(method)
            shut_down = True
            answer(message, None)
        elif method == "exit":
            log(method)
            log("status %d" % (0 if shut_down else 1))
            sys.exit(0 if shut_down else 1)
        else:
            serve(message)


main()
"#;

fn have_python3() -> bool {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
    }
    found
}

/// A project folder and a fake interpreter running the fake host.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    spec: HostSpec,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acme");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        let host = dir.path().join("fake_host.py");
        std::fs::write(&host, FAKE_HOST).unwrap();
        let python = dir.path().join("python-fake");
        std::fs::write(
            &python,
            format!("#!/bin/sh\nexec python3 '{}' \"$@\"\n", host.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = HostSpec {
            python,
            label: "python-fake".into(),
            project_root: root.clone(),
            sources: Vec::new(),
        };
        Fixture {
            _dir: dir,
            root,
            spec,
        }
    }

    fn pool(&self, max: usize) -> Arc<HostPool> {
        Arc::new(HostPool::new(self.spec.clone(), max))
    }

    /// How many hosts have started.
    fn started(&self) -> usize {
        std::fs::read_dir(&self.root)
            .unwrap()
            .filter(|entry| {
                let name = entry.as_ref().unwrap().file_name();
                let name = name.to_string_lossy();
                name.starts_with("host-") && name.ends_with(".pid")
            })
            .count()
    }

    /// What the host `pid` received.
    fn log(&self, pid: i64) -> Vec<String> {
        std::fs::read_to_string(self.root.join(format!("host-{pid}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn params(run_id: &str, run: Value) -> RunParams {
    let Value::Object(run) = run else {
        panic!("params are an object");
    };
    RunParams {
        run_id: run_id.into(),
        instance: RunInstance {
            id: "draw#1".into(),
            path: "draw".into(),
            step: "draw".into(),
            key: None,
            take: Vec::new(),
        },
        type_: "a1b2".into(),
        body: RunBody::Builtin {
            builtin: "fx/echo@1".into(),
        },
        params: run.into_iter().collect(),
        param_files: IndexMap::new(),
        inputs: IndexMap::new(),
        work_dir: "/work/acme/w".into(),
        resources: IndexMap::new(),
        tools: IndexMap::new(),
        calls: IndexMap::new(),
        timeout_s: None,
    }
}

/// Serves one run: answers `fact` with what the host's `tool.invoke` returned, asking it while
/// the host's request is pending; records requests and notifications.
struct Requests {
    host: Connection,
    run_id: String,
    requests: Mutex<Vec<(String, Value)>>,
    notes: Mutex<Vec<(String, Value)>>,
}

impl Requests {
    fn new(host: &Connection, run_id: &str) -> Arc<Requests> {
        Arc::new(Requests {
            host: host.clone(),
            run_id: run_id.into(),
            requests: Mutex::new(Vec::new()),
            notes: Mutex::new(Vec::new()),
        })
    }
}

impl RunRequests for Requests {
    fn request(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push((method, params));
            let invoke = json!({
                "run_id": self.run_id, "agent_id": "a1", "call_id": null,
                "name": "look", "arguments": {}
            });
            let tool = self
                .host
                .request("tool.invoke", Some(invoke))
                .await
                .map_err(|e| RpcError::new(ErrorCode::Internal, e.to_string()))?;
            Ok(json!({"tool": tool}))
        })
    }

    fn notify(&self, method: String, params: Value) {
        self.notes.lock().unwrap().push((method, params));
    }

    fn stop(&self) {}
}

/// Runs one `run` on a lease with fresh requests.
async fn run(
    lease: &mut grida_fx_runtime::host::pool::HostLease,
    run_id: &str,
    mode: Value,
    cancel: &Cancel,
    timeout: Option<Duration>,
) -> RunReply {
    let requests = Requests::new(lease.host().connection(), run_id);
    lease
        .run(&params(run_id, mode), requests, cancel, timeout)
        .await
}

fn pid_of(reply: &RunReply) -> i64 {
    match reply {
        RunReply::Result(result) => result["facts"]["pid"].as_i64().unwrap(),
        other => panic!("{other:?}"),
    }
}

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Waits up to 3 seconds for a process to go away.
async fn gone(pid: &str) -> bool {
    for _ in 0..300 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_go_both_ways_during_a_run() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    assert_eq!(lease.host().info().host.sdk_version, "0.0.0-fake");
    let requests = Requests::new(lease.host().connection(), "inv-1");
    let reply = lease
        .run(
            &params("inv-1", json!({"mode": "fact"})),
            requests.clone(),
            &Cancel::new(),
            None,
        )
        .await;
    let pid = pid_of(&reply);
    let RunReply::Result(result) = reply else {
        unreachable!()
    };
    // The host asked `fact`; the engine asked `tool.invoke` back before answering it.
    assert_eq!(
        result["facts"]["reply"],
        json!({"jsonrpc": "2.0", "id": 1, "result": {"tool": {"content": "looked at look"}}})
    );
    assert_eq!(
        *requests.requests.lock().unwrap(),
        [(
            "fact".to_string(),
            json!({"run_id": "inv-1", "name": "seen", "value": 1})
        )]
    );
    assert_eq!(
        *requests.notes.lock().unwrap(),
        [(
            "progress".to_string(),
            json!({"run_id": "inv-1", "text": "half"})
        )]
    );
    assert_eq!(fixture.log(pid), ["initialize", "run", "tool.invoke"]);

    // The host stays leased to this lease for the next run.
    let again = run(
        &mut lease,
        "inv-2",
        json!({"mode": "echo"}),
        &Cancel::new(),
        None,
    )
    .await;
    assert_eq!(pid_of(&again), pid);
    drop(lease);
    pool.shutdown().await;
    assert_eq!(
        fixture.log(pid),
        [
            "initialize",
            "run",
            "tool.invoke",
            "run",
            "shutdown",
            "exit",
            "status 0"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_router_refuses_another_run() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let requests = Requests::new(lease.host().connection(), "inv-1");
    let reply = lease
        .run(
            &params("inv-1", json!({"mode": "other-run"})),
            requests.clone(),
            &Cancel::new(),
            None,
        )
        .await;
    let RunReply::Result(result) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(
        result["facts"]["reply"]["error"],
        json!({"code": -32602, "message": "no run elsewhere is pending on this host"})
    );
    // Neither the request nor the notification reached this run.
    assert!(requests.requests.lock().unwrap().is_empty());
    assert!(requests.notes.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_run_is_cancelled_and_its_host_kept() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let cancel = Cancel::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stopper.cancel();
    });
    let start = Instant::now();
    // The host answers `cancelled` after `$/cancel`: the answer is discarded.
    let reply = run(
        &mut lease,
        "inv-1",
        json!({"mode": "cancel"}),
        &cancel,
        None,
    )
    .await;
    assert_eq!(reply, RunReply::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(3));
    let pid = pid_of(
        &run(
            &mut lease,
            "inv-2",
            json!({"mode": "echo"}),
            &Cancel::new(),
            None,
        )
        .await,
    );
    assert_eq!(fixture.log(pid), ["initialize", "run", "$/cancel", "run"]);

    // A run past its timeout ends the same way, and the host goes back to the pool.
    let reply = run(
        &mut lease,
        "inv-3",
        json!({"mode": "cancel"}),
        &Cancel::new(),
        Some(Duration::from_millis(100)),
    )
    .await;
    assert_eq!(reply, RunReply::TimedOut);
    drop(lease);
    let mut lease = pool.lease().await.unwrap();
    let reused = run(
        &mut lease,
        "inv-4",
        json!({"mode": "echo"}),
        &Cancel::new(),
        None,
    )
    .await;
    assert_eq!(pid_of(&reused), pid);
    assert_eq!(fixture.started(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_ignores_cancel_is_killed_after_five_seconds() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let connection = lease.host().connection().clone();
    let start = Instant::now();
    let reply = run(
        &mut lease,
        "inv-1",
        json!({"mode": "ignore-cancel"}),
        &Cancel::new(),
        Some(Duration::from_millis(100)),
    )
    .await;
    let took = start.elapsed();
    assert_eq!(reply, RunReply::TimedOut);
    assert!(took >= Duration::from_secs(5), "waited only {took:?}");
    assert!(took < Duration::from_secs(15), "waited {took:?}");
    connection.closed().await;
    assert!(!connection.is_open());
    // The host and what it started are gone.
    let pids: Vec<String> = std::fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("host-") && name.ends_with(".pid"))
        .collect();
    let pid = pids[0].trim_start_matches("host-").trim_end_matches(".pid");
    let grandchild =
        std::fs::read_to_string(fixture.root.join(format!("grandchild-{pid}.txt"))).unwrap();
    assert!(gone(pid).await, "the host {pid} survived");
    assert!(
        gone(&grandchild).await,
        "the grandchild {grandchild} survived"
    );
    // It is not reused.
    drop(lease);
    let mut lease = pool.lease().await.unwrap();
    let next = run(
        &mut lease,
        "inv-2",
        json!({"mode": "echo"}),
        &Cancel::new(),
        None,
    )
    .await;
    assert_ne!(pid_of(&next).to_string(), pid);
    assert_eq!(fixture.started(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_exits_during_a_run() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let reply = run(
        &mut lease,
        "inv-1",
        json!({"mode": "exit"}),
        &Cancel::new(),
        None,
    )
    .await;
    let RunReply::Exited(status) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(status.and_then(|status| status.code()), Some(7));
    // A later run on the same lease finds the host gone.
    assert!(matches!(
        run(
            &mut lease,
            "inv-2",
            json!({"mode": "echo"}),
            &Cancel::new(),
            None
        )
        .await,
        RunReply::Exited(_)
    ));
    drop(lease);
    let mut lease = pool.lease().await.unwrap();
    pid_of(
        &run(
            &mut lease,
            "inv-3",
            json!({"mode": "echo"}),
            &Cancel::new(),
            None,
        )
        .await,
    );
    assert_eq!(fixture.started(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_whose_fork_keeps_its_pipes_is_seen_to_exit() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let start = Instant::now();
    // No timeout: only the process's exit can end the run.
    let reply = run(
        &mut lease,
        "inv-1",
        json!({"mode": "fork-exit"}),
        &Cancel::new(),
        None,
    )
    .await;
    let took = start.elapsed();
    let RunReply::Exited(status) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(status.and_then(|status| status.code()), Some(3));
    assert!(took < Duration::from_secs(5), "{took:?}");
    // The fork went with the host's group.
    let forked = std::fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("forked-")
        })
        .unwrap();
    let forked = std::fs::read_to_string(forked).unwrap();
    assert!(gone(&forked).await, "the forked child {forked} survived");

    // With a timeout the exit is still what is reported.
    drop(lease);
    let mut lease = pool.lease().await.unwrap();
    let reply = run(
        &mut lease,
        "inv-2",
        json!({"mode": "fork-exit"}),
        &Cancel::new(),
        Some(Duration::from_secs(20)),
    )
    .await;
    assert!(
        matches!(&reply, RunReply::Exited(Some(status)) if status.code() == Some(3)),
        "{reply:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_body_leaves_behind_ends_with_its_host() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let pid = pid_of(
        &run(
            &mut lease,
            "inv-1",
            json!({"mode": "leave-child"}),
            &Cancel::new(),
            None,
        )
        .await,
    );
    let left = std::fs::read_to_string(fixture.root.join(format!("left-{pid}.txt"))).unwrap();
    drop(lease);
    // Kept while its host serves the pool.
    assert!(alive(&left));
    pool.shutdown().await;
    assert_eq!(
        fixture.log(pid),
        ["initialize", "run", "shutdown", "exit", "status 0"]
    );
    assert!(gone(&left).await, "{left} outlived its host");
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_come_back_as_the_host_gave_them() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let reply = run(
        &mut lease,
        "inv-1",
        json!({"mode": "fail"}),
        &Cancel::new(),
        None,
    )
    .await;
    assert_eq!(
        reply,
        RunReply::Error(RpcError::new(
            ErrorCode::NodeFailure,
            "the body failed on purpose"
        ))
    );
    drop(lease);
    // The host is kept.
    let mut lease = pool.lease().await.unwrap();
    pid_of(
        &run(
            &mut lease,
            "inv-2",
            json!({"mode": "echo"}),
            &Cancel::new(),
            None,
        )
        .await,
    );
    assert_eq!(fixture.started(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_breaks_the_protocol_is_not_reused() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let reply = run(
        &mut lease,
        "inv-1",
        json!({"mode": "garbage"}),
        &Cancel::new(),
        None,
    )
    .await;
    let RunReply::Error(error) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(error.kind(), Some(ErrorCode::Internal));
    assert!(
        error.message.starts_with(
            "the node host broke the protocol: the node host sent a message that is not I-JSON"
        ),
        "{}",
        error.message
    );
    drop(lease);
    let mut lease = pool.lease().await.unwrap();
    pid_of(
        &run(
            &mut lease,
            "inv-2",
            json!({"mode": "echo"}),
            &Cancel::new(),
            None,
        )
        .await,
    );
    assert_eq!(fixture.started(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_hosts_run_at_once_and_a_third_lease_waits() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(2);
    let mut first = pool.lease().await.unwrap();
    let mut second = pool.lease().await.unwrap();
    let start = Instant::now();
    let sleep = json!({"mode": "sleep", "seconds": 1.0});
    let cancel = Cancel::new();
    let (a, b) = tokio::join!(
        run(&mut first, "inv-1", sleep.clone(), &cancel, None),
        run(&mut second, "inv-2", sleep.clone(), &cancel, None),
    );
    let took = start.elapsed();
    assert!(took < Duration::from_millis(1900), "{took:?}");
    let first_pid = pid_of(&a);
    assert_ne!(first_pid, pid_of(&b));
    assert_eq!(fixture.started(), 2);

    // Both hosts are leased: a third lease waits for one to come back.
    let waiting = tokio::time::timeout(Duration::from_millis(300), pool.lease()).await;
    assert!(waiting.is_err());
    let third = tokio::spawn({
        let pool = Arc::clone(&pool);
        async move { pool.lease().await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!third.is_finished());
    drop(first);
    let mut third = third.await.unwrap().unwrap();
    let reused = run(&mut third, "inv-3", json!({"mode": "echo"}), &cancel, None).await;
    assert_eq!(pid_of(&reused), first_pid);
    assert_eq!(fixture.started(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_abandoned_midway_does_not_return_its_host() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(1);
    let mut lease = pool.lease().await.unwrap();
    let connection = lease.host().connection().clone();
    let abandoned = tokio::time::timeout(
        Duration::from_millis(300),
        run(
            &mut lease,
            "inv-1",
            json!({"mode": "sleep", "seconds": 30.0}),
            &Cancel::new(),
            None,
        ),
    )
    .await;
    assert!(abandoned.is_err());
    drop(lease);
    // The busy host is killed rather than handed to the next lease.
    connection.closed().await;
    let mut lease = pool.lease().await.unwrap();
    pid_of(
        &run(
            &mut lease,
            "inv-2",
            json!({"mode": "echo"}),
            &Cancel::new(),
            None,
        )
        .await,
    );
    assert_eq!(fixture.started(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_ends_idle_hosts_politely() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let pool = fixture.pool(2);
    let mut first = pool.lease().await.unwrap();
    let mut second = pool.lease().await.unwrap();
    let cancel = Cancel::new();
    let a = pid_of(&run(&mut first, "inv-1", json!({"mode": "echo"}), &cancel, None).await);
    let b = pid_of(&run(&mut second, "inv-2", json!({"mode": "echo"}), &cancel, None).await);
    drop(first);
    drop(second);
    pool.shutdown().await;
    for pid in [a, b] {
        assert_eq!(
            fixture.log(pid),
            ["initialize", "run", "shutdown", "exit", "status 0"]
        );
    }
    // The pool still leases: a new host.
    let mut lease = pool.lease().await.unwrap();
    pid_of(&run(&mut lease, "inv-3", json!({"mode": "echo"}), &cancel, None).await);
    assert_eq!(fixture.started(), 3);
}

#[tokio::test]
async fn a_pool_that_cannot_start_hosts_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HostPool::new(
        HostSpec {
            python: PathBuf::from("no-such-python-for-fx"),
            label: "no-such-python-for-fx".into(),
            project_root: dir.path().to_path_buf(),
            sources: Vec::new(),
        },
        2,
    ));
    let sentence = "cannot start the Python node host with no-such-python-for-fx: no such file; set GRIDA_FX_PYTHON to a Python that has the grida package";
    assert_eq!(pool.lease().await.err().unwrap(), sentence);
    assert_eq!(pool.lease().await.err().unwrap(), sentence);
}
