//! A node host process (spec/protocol.md §1, §2): `<python> -P -m grida.fx.host` started with
//! `tokio::process`, its working directory at the project root, stdin/stdout piped for the
//! protocol, stderr inherited, `PYTHONSAFEPATH` set to `host::SAFE_PATH_MARK` when the
//! environment leaves it unset or empty, and its own process group (Unix), so ending it ends what
//! it started. Dropping a process that was never ended kills it with its group.
//!
//! [`HostProcess::start`] spawns, opens a [`Connection`] and sends `initialize`; the answer's
//! protocol must be `fx-node-protocol-v1` (a mismatch, or a `protocol_mismatch` error, is the
//! sentence `PythonHost` reports: `the project's grida <sdk> speaks <p> and grida-fx <v>
//! speaks fx-node-protocol-v1: upgrade …`). Failures are sentences naming the interpreter as the
//! user would (`label`), never a private absolute path the user did not give. A host that fails
//! to initialize is ended without `shutdown`: its input is closed, it gets 5 seconds to exit, and
//! it is killed after that.
//!
//! Ending:
//! - [`HostProcess::shutdown`]: `shutdown` → `null`, then the `exit` notification, then its input
//!   is closed; waits 5 seconds for the process and then kills it (its group). A host whose
//!   connection already ended only has its input closed before the wait; one that does not take
//!   `shutdown` and `exit` within 5 seconds is killed at once;
//! - [`HostProcess::kill`]: kills the process group at once (a host that ignored `$/cancel` for 5
//!   seconds, spec/protocol.md §5.3);
//! - [`HostProcess::exit_status`]: the status once the process has exited (for `the node host
//!   exited with status <n>`).
//!
//! The group is killed only while the host itself has not been reaped, so its id cannot have
//! been reused by an unrelated process group.

use super::connection::{Connection, ConnectionError, Incoming};
use super::{GRACE, HOST_ARGS, PYTHON_ADVICE, SAFE_PATH_MARK, exited_with, mismatch, program};
use grida_fx_core::{ENGINE_NAME, ENGINE_VERSION};
use grida_fx_protocol::{
    EngineInfo, ErrorCode, ErrorData, InitializeParams, InitializeResult, PROTOCOL, method,
};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};

/// How to start a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    pub python: PathBuf,
    /// How messages name the interpreter (project-relative when inside the project).
    pub label: String,
    /// Absolute: the working directory and `initialize`'s `project_root`.
    pub project_root: PathBuf,
    pub sources: Vec<String>,
}

/// A started, initialized host.
pub struct HostProcess {
    connection: Connection,
    info: InitializeResult,
    running: Running,
}

impl HostProcess {
    /// Spawns and initializes (module doc). `incoming` serves the host's requests.
    pub async fn start(
        spec: &HostSpec,
        incoming: Arc<dyn Incoming>,
    ) -> Result<HostProcess, String> {
        HostProcess::start_within(spec, incoming, GRACE).await
    }

    /// [`HostProcess::start`], giving a host that fails to initialize `grace` to exit.
    pub(crate) async fn start_within(
        spec: &HostSpec,
        incoming: Arc<dyn Incoming>,
        grace: Duration,
    ) -> Result<HostProcess, String> {
        let shown = &spec.label;
        let Some(project_root) = spec.project_root.to_str() else {
            return Err(
                "the project folder's path is not UTF-8, which the node protocol needs".into(),
            );
        };
        let params = serde_json::to_value(InitializeParams {
            protocol: PROTOCOL.into(),
            engine: EngineInfo {
                name: ENGINE_NAME.into(),
                version: ENGINE_VERSION.into(),
            },
            project_root: project_root.into(),
            sources: spec.sources.clone(),
        })
        .map_err(|e| format!("the initialize request cannot be written: {e}"))?;
        let mut command = Command::new(program(&spec.python));
        command.args(HOST_ARGS).current_dir(&spec.project_root);
        if std::env::var_os("PYTHONSAFEPATH").is_none_or(|value| value.is_empty()) {
            command.env("PYTHONSAFEPATH", SAFE_PATH_MARK);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|e| {
            format!(
                "cannot start the Python node host with {shown}: {}; {PYTHON_ADVICE}",
                grida_fx_core::error::io_reason(&e)
            )
        })?;
        // `process_group(0)`: the host leads a group whose id is its pid.
        let group = if cfg!(unix) { child.id() } else { None };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(format!(
                "cannot start the Python node host with {shown}: its streams are not piped"
            ));
        };
        let connection = Connection::start(stdout, stdin, incoming);
        let mut running = Running {
            connection: connection.clone(),
            child,
            group,
            status: None,
        };
        let failure = match connection.request(method::INITIALIZE, Some(params)).await {
            Ok(value) => match serde_json::from_value::<InitializeResult>(value) {
                Ok(info) if info.protocol == PROTOCOL => {
                    return Ok(HostProcess {
                        connection,
                        info,
                        running,
                    });
                }
                Ok(info) => mismatch(Some(&info.host.sdk_version), &info.protocol),
                Err(e) => format!(
                    "the Python node host ({shown}) answered initialize with something FX cannot read: {e}"
                ),
            },
            Err(ConnectionError::Rpc(error))
                if error.kind() == Some(ErrorCode::ProtocolMismatch) =>
            {
                let host_protocol = error
                    .data
                    .clone()
                    .and_then(|data| serde_json::from_value::<ErrorData>(data).ok())
                    .and_then(|data| data.host_protocol);
                match host_protocol {
                    Some(host_protocol) => mismatch(None, &host_protocol),
                    None => format!(
                        "the project's grida does not speak {PROTOCOL}, which grida-fx {ENGINE_VERSION} speaks: {}; upgrade grida",
                        error.message
                    ),
                }
            }
            Err(ConnectionError::Rpc(error)) => format!(
                "the Python node host ({shown}) refused initialize: {}",
                error.message
            ),
            Err(ConnectionError::Closed) => {
                let status = running.end(false, grace).await;
                return Err(format!(
                    "the Python node host ({shown}) exited{} before it answered initialize; {PYTHON_ADVICE}",
                    exited_with(status)
                ));
            }
            Err(ConnectionError::Protocol(message)) => {
                format!("the Python node host ({shown}) broke the protocol: {message}")
            }
        };
        // spec/protocol.md §2: on a mismatch the engine reports both versions, then ends the host.
        running.end(false, grace).await;
        Err(failure)
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// What `initialize` answered.
    pub fn info(&self) -> &InitializeResult {
        &self.info
    }

    /// `shutdown`, `exit`, wait up to 5 seconds, then kill (module doc).
    pub async fn shutdown(mut self) -> Option<ExitStatus> {
        self.end(true, GRACE).await
    }

    /// Kills the process and its group at once.
    pub async fn kill(mut self) -> Option<ExitStatus> {
        self.kill_now().await
    }

    /// The exit status when the process has exited.
    pub fn exit_status(&mut self) -> Option<ExitStatus> {
        self.running.try_status()
    }

    /// Ends the host: politely (`shutdown`, `exit`) when asked and its connection still works,
    /// else by closing its input; then up to `grace` for it to exit before it is killed.
    pub(crate) async fn end(&mut self, polite: bool, grace: Duration) -> Option<ExitStatus> {
        self.running.end(polite, grace).await
    }

    /// Kills the process and its group, and waits for it.
    pub(crate) async fn kill_now(&mut self) -> Option<ExitStatus> {
        self.running.kill().await
    }

    /// Whether the host may serve another request: its connection works and it is running.
    pub(crate) fn is_healthy(&mut self) -> bool {
        self.connection.is_open() && self.exit_status().is_none()
    }

    /// Whether the host has been ended (killed, or seen to exit).
    pub(crate) fn has_ended(&self) -> bool {
        self.running.status.is_some()
    }
}

/// The process behind a host, and how to end it.
struct Running {
    connection: Connection,
    child: Child,
    /// The process group to kill with the host (Unix).
    group: Option<u32>,
    /// Set once the process has been reaped.
    status: Option<ExitStatus>,
}

impl Running {
    fn try_status(&mut self) -> Option<ExitStatus> {
        if self.status.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.status = Some(status);
        }
        self.status
    }

    /// Waits up to `limit` for the process to exit.
    async fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        if self.status.is_none()
            && let Ok(Ok(status)) = tokio::time::timeout(limit, self.child.wait()).await
        {
            self.status = Some(status);
        }
        self.status
    }

    /// See [`HostProcess::end`].
    async fn end(&mut self, polite: bool, grace: Duration) -> Option<ExitStatus> {
        let mut asked = true;
        if polite && self.status.is_none() && self.connection.is_open() {
            let connection = self.connection.clone();
            asked = tokio::time::timeout(grace, async move {
                let _ = connection.request(method::SHUTDOWN, None).await;
                let _ = connection.notify(method::EXIT, None).await;
            })
            .await
            .is_ok();
        }
        if asked
            && tokio::time::timeout(grace, self.connection.close_input())
                .await
                .is_ok()
            && let Some(status) = self.wait_exit(grace).await
        {
            return Some(status);
        }
        self.kill().await
    }

    /// Kills the group and the process, waits for it, then closes its input (a write blocked on
    /// the host ends with it).
    async fn kill(&mut self) -> Option<ExitStatus> {
        if self.try_status().is_none() {
            #[cfg(unix)]
            if let Some(group) = self.group {
                kill_group(group).await;
            }
            let _ = self.child.start_kill();
            if let Ok(status) = self.child.wait().await {
                self.status = Some(status);
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), self.connection.close_input()).await;
        self.status
    }
}

impl Drop for Running {
    /// A host dropped while it runs is killed with its group (`kill_on_drop` covers the host
    /// itself and reaps it in the background).
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.try_status().is_none()
            && let Some(group) = self.group
        {
            let _ = std::process::Command::new("kill")
                .args(["-s", "KILL", "--", &format!("-{group}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Sends `SIGKILL` to a process group (no unsafe code: the system's `kill` program).
#[cfg(unix)]
async fn kill_group(group: u32) {
    let _ = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{group}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host::connection::NoIncoming;
    use std::time::Instant;

    /// A host that answers `initialize`, starts a grandchild, and then ignores everything.
    const STUBBORN: &str = r#"
import json, os, subprocess, sys
child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
with open("grandchild.txt", "w") as f:
    f.write(str(child.pid))
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
while read() is not None:
    pass
import time
time.sleep(60)
"#;

    fn have_python3() -> bool {
        let found = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !found {
            eprintln!("skipped: python3 is not on PATH");
        }
        found
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Waits up to 3 seconds for a process another parent reaps to go away.
    async fn gone(pid: &str) -> bool {
        for _ in 0..300 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    fn stubborn() -> (tempfile::TempDir, HostSpec) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("stubborn.py"), STUBBORN).unwrap();
        let python = dir.path().join("python-stubborn");
        std::fs::write(
            &python,
            "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/stubborn.py\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = HostSpec {
            python,
            label: "python-stubborn".into(),
            project_root: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        (dir, spec)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_that_ignores_shutdown_is_killed_with_what_it_started() {
        if !have_python3() {
            return;
        }
        let (dir, spec) = stubborn();
        let mut host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        assert_eq!(host.info().host.sdk_version, "0");
        assert!(host.is_healthy());
        let grandchild = std::fs::read_to_string(dir.path().join("grandchild.txt")).unwrap();
        assert!(alive(&grandchild));
        let start = Instant::now();
        // It never answers `shutdown`: killed once the grace has passed.
        let status = host.end(true, Duration::from_millis(300)).await;
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert_eq!(status.and_then(|s| s.code()), None, "killed by a signal");
        assert!(host.has_ended());
        assert!(!host.is_healthy());
        assert!(
            gone(&grandchild).await,
            "the grandchild {grandchild} survived"
        );
        host.connection().closed().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn killing_a_host_kills_its_group() {
        if !have_python3() {
            return;
        }
        let (dir, spec) = stubborn();
        let host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        let grandchild = std::fs::read_to_string(dir.path().join("grandchild.txt")).unwrap();
        let connection = host.connection().clone();
        let start = Instant::now();
        let status = host.kill().await;
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(status.is_some());
        connection.closed().await;
        assert!(
            gone(&grandchild).await,
            "the grandchild {grandchild} survived"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_a_running_host_kills_its_group() {
        if !have_python3() {
            return;
        }
        let (dir, spec) = stubborn();
        let host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        let grandchild = std::fs::read_to_string(dir.path().join("grandchild.txt")).unwrap();
        let connection = host.connection().clone();
        drop(host);
        connection.closed().await;
        assert!(
            gone(&grandchild).await,
            "the grandchild {grandchild} survived"
        );
    }

    #[tokio::test]
    async fn a_missing_interpreter_is_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let spec = HostSpec {
            python: PathBuf::from("no-such-python-for-fx"),
            label: "no-such-python-for-fx".into(),
            project_root: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure,
            "cannot start the Python node host with no-such-python-for-fx: no such file; set GRIDA_FX_PYTHON to a Python that has the grida package"
        );
    }
}
