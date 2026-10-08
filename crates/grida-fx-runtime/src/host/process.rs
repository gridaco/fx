//! A node host process (spec/protocol.md §1, §2): `<python> -P -m grida.fx.host` started with
//! `tokio::process`, its working directory at the project root, stdin/stdout piped for the
//! protocol, stderr inherited, `PYTHONSAFEPATH` set to `host::SAFE_PATH_MARK` when the
//! environment leaves it unset or empty, and its own process group (Unix), so ending it ends what
//! it started. Dropping a process that was never ended kills it with its group.
//!
//! [`HostProcess::start`] spawns, opens a [`Connection`] and sends `initialize`
//! ([`HostProcess::start_in`]: in another working directory, for a stand-in host); the answer's
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
//! However a host ends, its process group is killed once the host has exited, so a process a body
//! started and left behind does not outlive the host (nor hold the command's output streams
//! open). A host that is killed has its group killed before it is reaped; a host that exits on
//! its own is reaped first and its group killed right after: while the group still has a member
//! its id cannot be reused (POSIX keeps a process group's id out of use until the group is
//! empty), and an empty group is gone, so the kill reaches nothing.
//!
//! The host's exit is seen by waiting on the process, not by the end of its output: a process
//! that forked from the host keeps the protocol pipes open after the host has gone.
//! [`HostProcess::answer_or_exit`] waits for an answer or the exit, whichever comes first; after
//! an exit (and the group kill, which closes the pipes such a process held) it reads what the
//! host wrote before it exited, and ends the connection when nothing more arrives within the
//! grace period (a process that left the group still holds the pipes).
//!
//! Every host this process started and has not ended is listed process-wide, so an interrupted
//! command can kill them all with [`end_every_host`] before it exits; after that call no host
//! starts.

use super::connection::{Connection, ConnectionError, Incoming};
use super::{GRACE, HOST_ARGS, PYTHON_ADVICE, SAFE_PATH_MARK, exit_text, mismatch, program};
use grida_fx_core::{ENGINE_NAME, ENGINE_VERSION};
use grida_fx_protocol::{
    EngineInfo, ErrorCode, ErrorData, InitializeParams, InitializeResult, PROTOCOL, method,
};
use serde_json::Value;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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

    /// [`HostProcess::start`] with the working directory `cwd` instead of the project root: a
    /// stand-in host (spec/protocol.md §1, §5.7) starts in the engine's working directory, with
    /// `spec.project_root` as `initialize`'s project. It is enlisted like any host, so
    /// [`end_every_host`] ends it too.
    pub async fn start_in(
        spec: &HostSpec,
        cwd: &Path,
        incoming: Arc<dyn Incoming>,
    ) -> Result<HostProcess, String> {
        HostProcess::start_at(spec, cwd, incoming, GRACE, None, None).await
    }

    /// [`HostProcess::start`], giving a host that fails to initialize `grace` to exit.
    pub(crate) async fn start_within(
        spec: &HostSpec,
        incoming: Arc<dyn Incoming>,
        grace: Duration,
    ) -> Result<HostProcess, String> {
        HostProcess::start_at(spec, &spec.project_root, incoming, grace, None, None).await
    }

    /// Pool startup retains process ownership while cancellation interrupts initialization.
    pub(crate) async fn start_cancellable(
        spec: &HostSpec,
        incoming: Arc<dyn Incoming>,
        grace: Duration,
        cancel: &crate::engine::Cancel,
        cleanup_verified: Arc<AtomicBool>,
    ) -> Result<HostProcess, String> {
        HostProcess::start_at(
            spec,
            &spec.project_root,
            incoming,
            grace,
            Some(cancel),
            Some(cleanup_verified),
        )
        .await
    }

    /// Spawns in `cwd` and initializes (module doc).
    async fn start_at(
        spec: &HostSpec,
        cwd: &Path,
        incoming: Arc<dyn Incoming>,
        grace: Duration,
        cancel: Option<&crate::engine::Cancel>,
        cleanup_verified: Option<Arc<AtomicBool>>,
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
        .map_err(|e| {
            format!(
                "the initialize request cannot be written: {}",
                super::read_reason(&e)
            )
        })?;
        let mut command = Command::new(program(&spec.python));
        command.args(HOST_ARGS).current_dir(cwd);
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
        let enlisted = group.is_none_or(enlist);
        let streams = (child.stdin.take(), child.stdout.take());
        let (true, (Some(stdin), Some(stdout))) = (enlisted, streams) else {
            if let Some(group) = group {
                forget(group);
                if !kill_groups_now(&[group])
                    && let Some(verified) = &cleanup_verified
                {
                    verified.store(false, Ordering::SeqCst);
                }
            }
            let _ = child.start_kill();
            if child.wait().await.is_err()
                && let Some(verified) = &cleanup_verified
            {
                verified.store(false, Ordering::SeqCst);
            }
            return Err(if enlisted {
                format!("cannot start the Python node host with {shown}: its streams are not piped")
            } else {
                "the command is stopping, so no node host starts".to_string()
            });
        };
        let connection = Connection::start(stdout, stdin, incoming);
        let mut running = Running {
            connection: connection.clone(),
            child,
            group,
            status: None,
            cleanup_verified,
            group_delivery_verified: true,
        };
        let asking = connection.clone();
        let request = asking.request(method::INITIALIZE, Some(params));
        tokio::pin!(request);
        let cancelled = async {
            match cancel {
                Some(cancel) => cancel.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let answer = tokio::select! {
            biased;
            () = cancelled => {
                running.kill().await;
                return Err("the run was stopped while its node host initialized".into());
            }
            answer = running.answer_or_exit(request, grace) => answer,
        };
        let failure = match answer {
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
                    "the Python node host ({shown}) answered initialize with something FX cannot read: {}",
                    super::read_reason(&e)
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
                    "the Python node host ({shown}) {} before it answered initialize; {PYTHON_ADVICE}",
                    exit_text(status)
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

    /// Shutdown evidence for an owner that must certify local cleanup.
    pub(crate) async fn shutdown_verified(mut self) -> bool {
        self.end(true, GRACE).await.is_some() && self.running.group_delivery_verified
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

    /// The answer to `request` (a request on this host's connection), or, when the host exits
    /// first, what it answered before it exited: the answer it wrote, else
    /// [`ConnectionError::Closed`] once its output ends or `grace` passes with no answer (the
    /// connection is then ended). See the module doc.
    pub(crate) async fn answer_or_exit<F>(
        &mut self,
        request: Pin<&mut F>,
        grace: Duration,
    ) -> Result<Value, ConnectionError>
    where
        F: Future<Output = Result<Value, ConnectionError>>,
    {
        self.running.answer_or_exit(request, grace).await
    }

    /// Resolves once the process has exited, with its status; its group has been killed by
    /// then (module doc). Cancel-safe.
    pub(crate) async fn exited(&mut self) -> Option<ExitStatus> {
        self.running.exited().await
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
    /// The process group to kill with the host (Unix), until it has been killed.
    group: Option<u32>,
    /// Set once the process has been reaped.
    status: Option<ExitStatus>,
    /// Pool receipt eligibility: a dropped or failed reap leaves cleanup unverified.
    cleanup_verified: Option<Arc<AtomicBool>>,
    group_delivery_verified: bool,
}

impl Running {
    /// Records the status of a reaped host and kills what is left of its group (module doc).
    fn reaped(&mut self, status: ExitStatus) {
        self.status = Some(status);
        self.end_group();
    }

    /// Kills the host's process group, once, and takes it off the process-wide list.
    fn end_group(&mut self) {
        if let Some(group) = self.group.take() {
            forget(group);
            if !kill_groups_now(&[group]) {
                self.group_delivery_verified = false;
                if let Some(verified) = &self.cleanup_verified {
                    verified.store(false, Ordering::SeqCst);
                }
            }
        }
    }

    fn try_status(&mut self) -> Option<ExitStatus> {
        if self.status.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.reaped(status);
        }
        self.status
    }

    /// Waits up to `limit` for the process to exit.
    async fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        if self.status.is_none()
            && let Ok(Ok(status)) = tokio::time::timeout(limit, self.child.wait()).await
        {
            self.reaped(status);
        }
        self.status
    }

    /// Resolves once the process has exited, with its status; its group has been killed by then.
    /// Cancel-safe. A process that cannot be waited for never resolves here.
    async fn exited(&mut self) -> Option<ExitStatus> {
        if self.status.is_none() {
            match self.child.wait().await {
                Ok(status) => self.reaped(status),
                Err(_) => std::future::pending::<()>().await,
            }
        }
        self.status
    }

    /// See [`HostProcess::answer_or_exit`].
    async fn answer_or_exit<F>(
        &mut self,
        mut request: Pin<&mut F>,
        grace: Duration,
    ) -> Result<Value, ConnectionError>
    where
        F: Future<Output = Result<Value, ConnectionError>>,
    {
        tokio::select! {
            biased;
            answer = &mut request => return answer,
            _ = self.exited() => {}
        }
        // Its group was killed with it, which closed the pipes a forked process held; what the
        // host wrote before it exited is still read.
        match tokio::time::timeout(grace, &mut request).await {
            Ok(answer) => answer,
            Err(_) => {
                self.connection.abandon();
                request.await
            }
        }
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
            // Before reaping: the group's id is the unreaped host's own.
            self.end_group();
            let _ = self.child.start_kill();
            if let Ok(status) = self.child.wait().await {
                self.reaped(status);
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), self.connection.close_input()).await;
        self.status
    }
}

impl Drop for Running {
    /// A host dropped while it runs is killed with its group (`kill_on_drop` covers the host
    /// itself and reaps it in the background); a host that exited has what is left of its group
    /// killed.
    fn drop(&mut self) {
        self.try_status();
        self.end_group();
        if self.status.is_none()
            && let Some(verified) = &self.cleanup_verified
        {
            verified.store(false, Ordering::SeqCst);
        }
    }
}

/// The process groups of the hosts this process started and has not ended, and whether hosts may
/// still start (module doc).
struct Groups {
    live: Vec<u32>,
    closed: bool,
}

static GROUPS: Mutex<Groups> = Mutex::new(Groups {
    live: Vec::new(),
    closed: false,
});

fn groups() -> MutexGuard<'static, Groups> {
    GROUPS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lists a new host's group; `false` once [`end_every_host`] was called.
fn enlist(group: u32) -> bool {
    let mut groups = groups();
    if groups.closed {
        return false;
    }
    groups.live.push(group);
    true
}

/// Takes a group off the list.
fn forget(group: u32) {
    groups().live.retain(|&listed| listed != group);
}

/// Kills the process group of every host this process started and has not ended, and lets no
/// host start after it: for a command that is about to exit because it was interrupted.
pub fn end_every_host() {
    let live = {
        let mut groups = groups();
        groups.closed = true;
        std::mem::take(&mut groups.live)
    };
    kill_groups_now(&live);
}

/// Sends `SIGKILL` directly to every owned group. Only successful delivery or the kernel's
/// `ESRCH` (the group is already absent) verifies cleanup; other errors remain unverified.
#[cfg(unix)]
fn kill_groups_now(groups: &[u32]) -> bool {
    kill_groups_with(groups, rustix::process::kill_process_group)
}

#[cfg(unix)]
fn kill_groups_with(
    groups: &[u32],
    mut kill: impl FnMut(rustix::process::Pid, rustix::process::Signal) -> rustix::io::Result<()>,
) -> bool {
    use rustix::io::Errno;
    use rustix::process::{Pid, Signal};

    groups.iter().fold(true, |verified, &group| {
        // Zero names the caller's group and one would map to kill(-1), so neither can be an
        // owned host group. Reject them and out-of-range IDs before invoking the syscall.
        let pid = i32::try_from(group)
            .ok()
            .filter(|&raw| raw > 1)
            .and_then(Pid::from_raw);
        let result = pid
            .ok_or(Errno::INVAL)
            .and_then(|pid| kill(pid, Signal::KILL));
        // Do not short-circuit after failure: every other owned group still needs cleanup.
        verified & matches!(result, Ok(()) | Err(Errno::SRCH))
    })
}

/// Process groups are a Unix notion: `kill_on_drop` ends a host elsewhere.
#[cfg(not(unix))]
fn kill_groups_now(_groups: &[u32]) -> bool {
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host::connection::NoIncoming;
    use std::time::Instant;

    #[test]
    fn group_cleanup_accepts_only_delivery_or_kernel_confirmed_absence() {
        use rustix::io::Errno;

        assert!(kill_groups_with(&[42], |_, _| Ok(())));
        assert!(kill_groups_with(&[42], |_, _| Err(Errno::SRCH)));
        for error in [Errno::PERM, Errno::INVAL, Errno::IO, Errno::INTR] {
            assert!(!kill_groups_with(&[42], |_, _| Err(error)), "{error:?}");
        }
        let mut attempted = Vec::new();
        assert!(!kill_groups_with(&[42, 43], |pid, _| {
            attempted.push(pid.as_raw_pid());
            if pid.as_raw_pid() == 42 {
                Err(Errno::PERM)
            } else {
                Ok(())
            }
        }));
        assert_eq!(attempted, [42, 43]);
        assert!(!kill_groups_with(&[0, 1, u32::MAX], |_, _| panic!(
            "an invalid group must never reach kill"
        )));
        assert!(kill_groups_now(&[]));
    }

    #[tokio::test]
    async fn an_already_reaped_empty_group_is_verified_by_esrch() {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let group = child.id().unwrap();
        assert!(child.wait().await.unwrap().success());
        let pid = rustix::process::Pid::from_raw(i32::try_from(group).unwrap()).unwrap();
        assert_eq!(
            rustix::process::test_kill_process_group(pid),
            Err(rustix::io::Errno::SRCH)
        );
        assert!(kill_groups_now(&[group]));
    }

    /// Runs with a child-only PATH override, so concurrent tests keep their own environment.
    #[tokio::test]
    async fn path_shadowed_kill_cannot_fake_verified_group_cleanup() {
        use rustix::process::{Pid, Signal, kill_process, test_kill_process};
        use std::os::unix::fs::PermissionsExt;

        if std::env::var("GRIDA_FX_GROUP_KILL_CHILD").as_deref() != Ok("1") {
            let dir = tempfile::tempdir().unwrap();
            let stub = dir.path().join("kill");
            std::fs::write(
                &stub,
                "#!/bin/sh\nprintf '%s' \"$*\" >> \"$GRIDA_FX_GROUP_KILL_LOG\"\nexit 1\n",
            )
            .unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
            let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "host::process::tests::path_shadowed_kill_cannot_fake_verified_group_cleanup",
                    "--nocapture",
                ])
                .env("PATH", path)
                .env("GRIDA_FX_GROUP_KILL_CHILD", "1")
                .env("GRIDA_FX_GROUP_KILL_LOG", dir.path().join("invoked"))
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !dir.path().join("invoked").exists(),
                "group cleanup must not resolve kill through PATH"
            );
            return;
        }
        assert!(
            have_python3(),
            "this process-group regression needs python3"
        );
        let (dir, spec) = fake(
            "polite",
            &format!("{FRAMES}{POLITE}{INITIALIZED}{POLITE_LOOP}"),
        );
        let host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        let grandchild = std::fs::read_to_string(dir.path().join("grandchild.txt")).unwrap();
        let pid = Pid::from_raw(grandchild.trim().parse().unwrap()).unwrap();
        assert_eq!(test_kill_process(pid), Ok(()));
        let verified = host.shutdown_verified().await;
        let ended = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if test_kill_process(pid) == Err(rustix::io::Errno::SRCH) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        if !ended {
            // A failing regression must still clean up the real child it started.
            let _ = kill_process(pid, Signal::KILL);
        }
        assert!(verified, "the actual kernel delivery should verify cleanup");
        assert!(
            ended,
            "the grandchild {grandchild} survived reported verified cleanup"
        );
    }

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
        fake("stubborn", STUBBORN)
    }

    /// A folder holding `script` and an interpreter named `python-<name>` that runs it.
    fn fake(name: &str, script: &str) -> (tempfile::TempDir, HostSpec) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(format!("{name}.py")), script).unwrap();
        let python = dir.path().join(format!("python-{name}"));
        std::fs::write(
            &python,
            format!("#!/bin/sh\nexec python3 \"$(dirname \"$0\")/{name}.py\"\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let spec = HostSpec {
            python,
            label: format!("python-{name}"),
            project_root: dir.path().to_path_buf(),
            sources: Vec::new(),
        };
        (dir, spec)
    }

    /// The reading and writing half of a fake host: `read()` and `write(message)`.
    const FRAMES: &str = r#"
import json, os, subprocess, sys, time
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
def write(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
"#;

    /// Answers `initialize`.
    const INITIALIZED: &str = r#"
message = read()
write({"jsonrpc": "2.0", "id": message["id"], "result": {
    "protocol": message["params"]["protocol"],
    "host": {"language": "python", "version": "3", "sdk_version": "0"}}})
"#;

    /// A host that, asked anything, forks a child that keeps the protocol pipes and sleeps, then
    /// exits with status 3.
    const FORKER: &str = r#"
read()
child = os.fork()
if child == 0:
    time.sleep(30)
    os._exit(0)
with open("forked.txt", "w") as f:
    f.write(str(child))
os._exit(3)
"#;

    /// A host that starts a grandchild before it answers `initialize`, then takes `shutdown` and
    /// `exit` as asked.
    const POLITE: &str = r#"
child = subprocess.Popen(["sleep", "60"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
with open("grandchild.txt", "w") as f:
    f.write(str(child.pid))
"#;

    const POLITE_LOOP: &str = r#"
while True:
    message = read()
    if message is None or message.get("method") == "exit":
        sys.exit(0)
    if message.get("method") == "shutdown":
        write({"jsonrpc": "2.0", "id": message["id"], "result": None})
"#;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_whose_fork_holds_its_pipes_is_seen_to_exit() {
        if !have_python3() {
            return;
        }
        let (dir, spec) = fake("forker", &format!("{FRAMES}{INITIALIZED}{FORKER}"));
        let mut host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        let connection = host.connection().clone();
        let request = connection.request("describe", None);
        tokio::pin!(request);
        let start = Instant::now();
        let answer = host.answer_or_exit(request, Duration::from_secs(20)).await;
        let took = start.elapsed();
        assert_eq!(answer, Err(ConnectionError::Closed));
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert_eq!(host.exit_status().and_then(|s| s.code()), Some(3));
        assert!(host.has_ended());
        // Its group was killed with it, the fork included, which closed the pipes.
        let forked = std::fs::read_to_string(dir.path().join("forked.txt")).unwrap();
        assert!(gone(&forked).await, "the forked child {forked} survived");
        connection.closed().await;
        assert_eq!(host.shutdown().await.and_then(|s| s.code()), Some(3));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_ended_politely_takes_what_it_started_with_it() {
        if !have_python3() {
            return;
        }
        let (dir, spec) = fake(
            "polite",
            &format!("{FRAMES}{POLITE}{INITIALIZED}{POLITE_LOOP}"),
        );
        let host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        let grandchild = std::fs::read_to_string(dir.path().join("grandchild.txt")).unwrap();
        assert!(alive(&grandchild));
        let start = Instant::now();
        let status = host.shutdown().await;
        assert!(start.elapsed() < Duration::from_secs(4));
        assert_eq!(
            status.and_then(|s| s.code()),
            Some(0),
            "it exited on its own"
        );
        assert!(
            gone(&grandchild).await,
            "the grandchild {grandchild} outlived its host"
        );
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

    /// Every process the engine starts gets a standard input of its own, so none inherits the
    /// stand-in's socket on the engine's (spec/protocol.md §5.7): each `Command` the engine's
    /// sources build outside their tests sets `stdin` before it starts the process.
    #[test]
    fn every_process_the_engine_starts_sets_its_standard_input() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut checked = 0;
        for source in [
            "grida-fx-runtime/src",
            "grida-fx/src",
            "grida-fx-providers/src",
        ] {
            let mut folders = vec![crates.join(source)];
            while let Some(folder) = folders.pop() {
                for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        folders.push(path);
                        continue;
                    }
                    if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                        continue;
                    }
                    let text = std::fs::read_to_string(&path).unwrap();
                    // The file's own tests start their programs as they like.
                    let code = text
                        .find("#[cfg(test)]")
                        .into_iter()
                        .chain(text.find("#[cfg(all(test"))
                        .min()
                        .map_or(text.as_str(), |at| &text[..at]);
                    for (at, _) in code.match_indices("Command::new(") {
                        let rest = &code[at..];
                        let end = [".spawn()", ".status()", ".output()"]
                            .iter()
                            .filter_map(|start| rest.find(start))
                            .min()
                            .or_else(|| rest.find("let mut child"))
                            .unwrap_or(rest.len());
                        assert!(
                            rest[..end].contains(".stdin("),
                            "{}: a Command that does not set stdin",
                            path.display()
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked >= 3, "found {checked} commands");
    }
}
