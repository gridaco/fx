//! A node host process (spec/protocol.md §1, §2), of either language ([`HostSpec::language`]).
//! When its spec asks for it (`HostSpec::check_version`), the program is first probed
//! ([`locate::probe`]) and one that cannot host nodes is refused with [`locate::refusal`]'s
//! sentence (a Python before 3.11, a Node before 24 or without `module.registerHooks`), before
//! anything is spawned that could print a usage text. Then the host is started with
//! `tokio::process`: `<python> -P -m grida.fx.host` with `PYTHONSAFEPATH` set to
//! `host::SAFE_PATH_MARK` when the environment leaves it unset or empty, or `<node>
//! <host-entry.js>`; its working directory at the project root, stdin/stdout piped for the
//! protocol, and its own process group (Unix), so ending it ends what it started. Dropping a
//! process that was never ended kills it with its group.
//!
//! A Python host inherits the engine's stderr. A TypeScript host's stderr is a pipe of its own
//! (Unix), which a thread ([`StderrCopy`]) copies to the engine's stderr as spec/protocol.md §1
//! "Output streams" states, with why an inherited stderr would not do: whole lines under the
//! stderr lock the engine's own lines take too ([`crate::stderr`]), at most 512 bytes of them per
//! write, a line in parts only past [`LONGEST_LINE`].
//!
//! Ending a TypeScript host waits for its copy by that section's rule: bound by what the host
//! left, not by time, unless the copy has waited [`DRAIN`] in all for its pipe, or
//! [`DRAIN_IN_ALL`] has passed since it read more than the host left. [`end_every_host`], which
//! the command calls before it exits, waits for every copy left by the same rule;
//! [`end_every_host_now`], which an interrupted command calls, for [`EXIT_DRAIN`] at most.
//!
//! [`HostProcess::start`] spawns, opens a [`Connection`] and sends `initialize`
//! ([`HostProcess::start_in`]: in another working directory, for a stand-in host) with the
//! declared source packages the host's language can name ([`Language::accepts_source`]); the
//! answer's protocol must be `fx-node-protocol-v1` (a mismatch, or a `protocol_mismatch` error,
//! is the sentence the planning host reports: `the project's grida <sdk> speaks <p> and grida-fx
//! <v> speaks fx-node-protocol-v1: upgrade …`, `@grida/fx` for TypeScript). Failures are
//! sentences naming the host of their language (`the Python node host`, `the TypeScript node
//! host`) and its program as the user would (`label`), never a private absolute path the user
//! did not give. A host that fails to initialize is ended without `shutdown`: its input is
//! closed, it gets 5 seconds to exit, and it is killed after that.
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
//! Every host this process started and has not ended is listed process-wide, so a command can
//! kill them all before it exits ([`end_every_host`]; [`end_every_host_now`] when interrupted);
//! after either call no host starts.

use super::connection::{Connection, ConnectionError, Incoming};
use super::{GRACE, HOST_ARGS, SAFE_PATH_MARK, advice, exit_text, locate, mismatch, program};
use grida_fx_core::host::Language;
use grida_fx_core::{ENGINE_NAME, ENGINE_VERSION};
use grida_fx_protocol::{
    EngineInfo, ErrorCode, ErrorData, InitializeParams, InitializeResult, PROTOCOL, method,
};
use serde_json::Value;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// How to start a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// The language of the modules the host runs.
    pub language: Language,
    /// The interpreter (Python) or Node (TypeScript).
    pub program: PathBuf,
    /// The program's arguments: `-P -m grida.fx.host` (Python), the absolute path of the host
    /// program `dist/host-entry.js` (TypeScript).
    pub args: Vec<OsString>,
    /// How messages name the program (project-relative when inside the project).
    pub label: String,
    /// Absolute: the working directory and `initialize`'s `project_root`.
    pub project_root: PathBuf,
    /// The project's declared source packages; `initialize` carries those the language can name
    /// ([`Language::accepts_source`]).
    pub sources: Vec<String>,
    /// Check the program before the first start and refuse one that cannot host nodes
    /// ([`locate::probe`], [`locate::refusal`], spec/protocol.md §1). Set for a program FX
    /// located; a caller that names a program of its own that ignores its arguments (a test's
    /// fake host) leaves it unset ([`HostSpec::unchecked`]).
    pub check_version: bool,
}

impl HostSpec {
    /// The Python host of `project_root`: `<python> -P -m grida.fx.host`, checked before it
    /// starts.
    pub fn python(
        python: impl Into<PathBuf>,
        label: impl Into<String>,
        project_root: impl Into<PathBuf>,
        sources: Vec<String>,
    ) -> HostSpec {
        HostSpec {
            language: Language::Python,
            program: python.into(),
            args: HOST_ARGS.iter().map(OsString::from).collect(),
            label: label.into(),
            project_root: project_root.into(),
            sources,
            check_version: true,
        }
    }

    /// The TypeScript host of `project_root`: `<node> <entry>`, `entry` the absolute path of a
    /// `@grida/fx`'s `dist/host-entry.js` ([`locate::node_host_entry`]), checked before it starts.
    pub fn typescript(
        node: impl Into<PathBuf>,
        entry: impl Into<PathBuf>,
        label: impl Into<String>,
        project_root: impl Into<PathBuf>,
        sources: Vec<String>,
    ) -> HostSpec {
        HostSpec {
            language: Language::TypeScript,
            program: node.into(),
            args: vec![entry.into().into_os_string()],
            label: label.into(),
            project_root: project_root.into(),
            sources,
            check_version: true,
        }
    }

    /// The host of `language` for the project at `root`, with what [`locate`] finds from it, else
    /// from the `planning` project: the interpreter (Python), or Node and the host program of a
    /// `@grida/fx` (TypeScript). The program is named relative to either project when it lies in
    /// one ([`super::label_from`]). `Err` is why there is no host program; `env` reads an
    /// environment variable.
    pub fn locate(
        language: Language,
        root: &Path,
        planning: Option<&Path>,
        sources: Vec<String>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<HostSpec, String> {
        Ok(match language {
            Language::Python => {
                let python = locate::python_interpreter(root, planning, env);
                let label = super::label_from(&python, root, planning);
                HostSpec::python(python, label, root, sources)
            }
            Language::TypeScript => {
                let entry = locate::node_host_entry(root, planning, env)?;
                let node = locate::node_runtime(env);
                let label = super::label_from(&node, root, planning);
                HostSpec::typescript(node, entry, label, root, sources)
            }
        })
    }

    /// The same host, started without checking its program first (a program of the caller's
    /// own, such as a test's fake host, that ignores its arguments).
    pub fn unchecked(mut self) -> HostSpec {
        self.check_version = false;
        self
    }

    /// The folder of the `@grida/fx` whose host program a TypeScript host runs (its argument,
    /// [`HostSpec::typescript`]), when it lies outside `project_root` ([`locate::host_package`]);
    /// `None` for a Python host.
    pub fn host_package(&self, project_root: &Path) -> Option<PathBuf> {
        match self.language {
            Language::TypeScript => {
                locate::host_package(Path::new(self.args.first()?), project_root)
            }
            Language::Python => None,
        }
    }

    /// [`HostSpec::host_package`] of its own project, labelled `@grida/fx` for
    /// [`crate::engine::scrub_paths`] ([`crate::engine::root_labels`]): Node's messages may name
    /// its files. Empty for a Python host, or a host program inside the project.
    pub fn package_labels(&self) -> Vec<(String, String)> {
        self.host_package(&self.project_root)
            .map(|package| crate::engine::root_labels(&package, Language::TypeScript.sdk()))
            .unwrap_or_default()
    }
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
        let title = spec.language.title();
        let sdk = spec.language.sdk();
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
            sources: spec
                .sources
                .iter()
                .filter(|name| spec.language.accepts_source(name))
                .cloned()
                .collect(),
        })
        .map_err(|e| {
            format!(
                "the initialize request cannot be written: {}",
                super::read_reason(&e)
            )
        })?;
        if spec.check_version {
            let probed = match cancel {
                Some(cancel) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => {
                        return Err("the run was stopped while its node host initialized".into());
                    }
                    probed = locate::probe(spec.language, &spec.program) => probed,
                },
                None => locate::probe(spec.language, &spec.program).await,
            };
            if let Some(refusal) = locate::refusal(spec.language, &probed, shown) {
                return Err(refusal);
            }
        }
        let mut command = Command::new(program(&spec.program));
        command.args(&spec.args).current_dir(cwd);
        if spec.language == Language::Python
            && std::env::var_os("PYTHONSAFEPATH").is_none_or(|value| value.is_empty())
        {
            command.env("PYTHONSAFEPATH", SAFE_PATH_MARK);
        }
        // A TypeScript host gets a stderr pipe of its own, copied to the engine's (module doc).
        let forward_stderr = cfg!(unix) && spec.language == Language::TypeScript;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if forward_stderr {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|e| {
            format!(
                "cannot start the {title} node host with {shown}: {}; {}",
                grida_fx_core::error::io_reason(&e),
                advice(spec.language)
            )
        })?;
        let forwarded = child.stderr.take().and_then(forward);
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
                format!(
                    "cannot start the {title} node host with {shown}: its streams are not piped"
                )
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
            forwarded,
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
                Ok(info) => mismatch(spec.language, Some(&info.host.sdk_version), &info.protocol),
                Err(e) => format!(
                    "the {title} node host ({shown}) answered initialize with something FX cannot read: {}",
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
                    Some(host_protocol) => mismatch(spec.language, None, &host_protocol),
                    None => format!(
                        "the project's {sdk} does not speak {PROTOCOL}, which grida-fx {ENGINE_VERSION} speaks: {}; upgrade {sdk}",
                        error.message
                    ),
                }
            }
            Err(ConnectionError::Rpc(error)) => format!(
                "the {title} node host ({shown}) refused initialize: {}",
                error.message
            ),
            Err(ConnectionError::Closed) => {
                let status = running.end(false, grace).await;
                return Err(format!(
                    "the {title} node host ({shown}) {} before it answered initialize; {}",
                    exit_text(status),
                    advice(spec.language)
                ));
            }
            Err(ConnectionError::Protocol(message)) => {
                format!("the {title} node host ({shown}) broke the protocol: {message}")
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
    /// The copy of a TypeScript host's stderr (module doc), until it has been waited for.
    forwarded: Option<Arc<StderrCopy>>,
}

impl Running {
    /// Waits for the copy of the host's stderr to end ([`StderrCopy::wait`]), once.
    async fn drained(&mut self) {
        if let Some(copy) = self.forwarded.take() {
            copy.limit();
            let _ = tokio::task::spawn_blocking(move || copy.wait()).await;
        }
    }

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

    /// See [`HostProcess::end`]; then what the host wrote to its stderr is copied (module doc).
    async fn end(&mut self, polite: bool, grace: Duration) -> Option<ExitStatus> {
        let status = self.stop(polite, grace).await;
        self.drained().await;
        status
    }

    /// Ends the process: politely when asked, else by closing its input; killed after `grace`.
    async fn stop(&mut self, polite: bool, grace: Duration) -> Option<ExitStatus> {
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
    /// the host ends with it); then what the host wrote to its stderr is copied (module doc).
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
        self.drained().await;
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

/// Lists a new host's group; `false` once [`end_every_host`] or [`end_every_host_now`] was
/// called.
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
/// host start after it; then waits for the copies of TypeScript hosts' stderr that have not ended,
/// by the rule a host's ending waits by (module doc). For a command that is about to exit, its
/// work done.
pub fn end_every_host() {
    kill_every_host();
    COPIES.wait_for_all(false);
}

/// [`end_every_host`] for a command that must end now (an interruption): the copies left are
/// waited for [`EXIT_DRAIN`] at most from the call, whatever they do (module doc).
pub fn end_every_host_now() {
    kill_every_host();
    COPIES.wait_for_all(true);
}

/// Kills the process group of every host this process started and has not ended, and of every
/// Codex conversation, and lets no host start after it.
fn kill_every_host() {
    let live = {
        let mut groups = groups();
        groups.closed = true;
        std::mem::take(&mut groups.live)
    };
    kill_groups_now(&live);
    // Codex conversations (spec/providers.md §9.6) own process groups too.
    grida_fx_providers::codex::end_every_codex();
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

/// How long the copy of a TypeScript host's stderr may wait for its pipe, in all, while it is
/// waited for ([`StderrCopy::wait`]): a process that left the host's group may hold the pipe
/// open for good.
const DRAIN: Duration = Duration::from_secs(1);

/// How long the copy of a TypeScript host's stderr is waited for at most, whatever it does, once
/// it has read more than its host left ([`StderrCopy::limit`]): a process that left the host's
/// group is printing, perhaps faster than the engine's stderr is read (so the copy never waits for
/// its pipe), or into a stderr nobody reads.
const DRAIN_IN_ALL: Duration = Duration::from_secs(10);

/// How long [`end_every_host_now`] waits for the copies left, at most, from its call, whatever
/// they do: the command is interrupted.
const EXIT_DRAIN: Duration = Duration::from_secs(2);

/// The most the copy reads from its pipe at once.
const READ_SIZE: usize = 16 * 1024;

/// What the copy's pipe is taken to hold when that cannot be measured: Linux's largest pipe a
/// process may set without privilege (`/proc/sys/fs/pipe-max-size`).
const PIPE_MOST: u64 = 1024 * 1024;

/// How much of a line the copy holds back before it writes the part it has (module doc).
#[cfg(unix)]
const LONGEST_LINE: usize = 64 * 1024;

/// The limits a copy is waited for by: [`DRAIN`], [`DRAIN_IN_ALL`] and [`EXIT_DRAIN`] (tests use
/// shorter ones).
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// [`DRAIN`].
    idle: Duration,
    /// [`DRAIN_IN_ALL`].
    flood: Duration,
    /// [`EXIT_DRAIN`].
    exit: Duration,
}

impl Limits {
    const OF_COMMAND: Limits = Limits {
        idle: DRAIN,
        flood: DRAIN_IN_ALL,
        exit: EXIT_DRAIN,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::OF_COMMAND
    }
}

/// The copy of a TypeScript host's stderr (module doc), as its thread reports it to whoever waits
/// for it to end.
#[derive(Default)]
struct StderrCopy {
    state: Mutex<Copying>,
    changed: Condvar,
    limits: Limits,
    /// The reading end of the copy's pipe, to measure what it holds ([`StderrCopy::limit`]).
    #[cfg(unix)]
    pipe: Option<std::os::fd::OwnedFd>,
}

#[derive(Default)]
struct Copying {
    ended: bool,
    /// Since when the copy has been waiting for its pipe, while it waits.
    waiting_since: Option<Instant>,
    /// How long the copy waited for its pipe before that.
    waited: Duration,
    /// How many bytes the copy has read from its pipe.
    read: u64,
    /// Once the copy is waited for: when waiting for it gives up ([`StderrCopy::limit`]).
    limit: Option<Limit>,
}

/// When waiting for a copy gives up, whichever comes first.
#[derive(Debug, Clone, Copy)]
struct Limit {
    /// The copy has waited this long for its pipe, in all.
    waited: Duration,
    /// What the copy has read, in all, once it has read what its host left: more is text written
    /// after the host ended.
    left: u64,
    /// Once the copy has read more than `left`: this instant has come, whatever the copy does.
    by: Option<Instant>,
}

impl Copying {
    /// How long the copy has waited for its pipe by `now`.
    fn waited(&self, now: Instant) -> Duration {
        self.waited
            + self
                .waiting_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// The copy has read `read` bytes more; once that is more than its host left, waiting for it
    /// gives up `flood` from now at the latest.
    #[cfg(unix)]
    fn took(&mut self, read: usize, flood: Duration) {
        self.read += read as u64;
        let total = self.read;
        if let Some(limit) = &mut self.limit
            && limit.by.is_none()
            && total > limit.left
        {
            limit.by = Some(Instant::now() + flood);
        }
    }
}

impl StderrCopy {
    fn state(&self) -> MutexGuard<'_, Copying> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the state and wakes whoever waits.
    #[cfg(unix)]
    fn update(&self, change: impl FnOnce(&mut Copying)) {
        change(&mut self.state());
        self.changed.notify_all();
    }

    /// How many bytes the copy's pipe holds now ([`PIPE_MOST`] when that cannot be measured).
    fn pending(&self) -> u64 {
        #[cfg(unix)]
        if let Some(pipe) = &self.pipe
            && let Ok(held) = rustix::io::ioctl_fionread(pipe)
        {
            return held;
        }
        PIPE_MOST
    }

    /// Starts the copy's limits, once, the host having ended and its group been killed: waiting
    /// for it gives up when the copy has waited one more [`DRAIN`] for its pipe than it has by
    /// now; and, once it has read more than what its host left (what its pipe holds now and one
    /// read the copy may be taking from it), [`DRAIN_IN_ALL`] after that. Every later wait keeps
    /// the same limits.
    fn limit(&self) -> Limit {
        let mut copying = self.state();
        if let Some(limit) = copying.limit {
            return limit;
        }
        let waited = copying.waited(Instant::now()) + self.limits.idle;
        let left = copying.read + self.pending() + READ_SIZE as u64;
        *copying.limit.insert(Limit {
            waited,
            left,
            by: None,
        })
    }

    /// Waits for the copy to end: for as long as it has text to write, however slow the reader,
    /// until it has waited for its pipe up to its [`StderrCopy::limit`], and, once it has read
    /// more than its host left, until that limit's instant has come. Whether the copy ended.
    fn wait(&self) -> bool {
        self.wait_until(None)
    }

    /// [`StderrCopy::wait`], giving up at `deadline` too when there is one.
    fn wait_until(&self, deadline: Option<Instant>) -> bool {
        let limit = self.limit();
        let mut copying = self.state();
        loop {
            if copying.ended {
                return true;
            }
            let now = Instant::now();
            let idle = limit.waited.saturating_sub(copying.waited(now));
            let by = copying
                .limit
                .and_then(|limit| limit.by)
                .into_iter()
                .chain(deadline)
                .min();
            let left = by.map(|by| by.saturating_duration_since(now));
            if idle.is_zero() || left.is_some_and(|left| left.is_zero()) {
                return false;
            }
            // Waiting for its pipe, the time it may wait runs out unless text arrives; writing,
            // only an instant counts, however slow the reader.
            let timeout = match (copying.waiting_since, left) {
                (Some(_), Some(left)) => Some(idle.min(left)),
                (Some(_), None) => Some(idle),
                (None, left) => left,
            };
            copying = match timeout {
                Some(timeout) => {
                    self.changed
                        .wait_timeout(copying, timeout)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .changed
                    .wait(copying)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

/// Copies of hosts' stderr that have not ended, and the limits they are waited for by.
struct StderrCopies {
    listed: Mutex<Vec<Arc<StderrCopy>>>,
    limits: Limits,
}

/// The copies of this process's hosts' stderr that have not ended.
static COPIES: StderrCopies = StderrCopies::with(Limits::OF_COMMAND);

impl StderrCopies {
    const fn with(limits: Limits) -> StderrCopies {
        StderrCopies {
            listed: Mutex::new(Vec::new()),
            limits,
        }
    }

    fn listed(&self) -> MutexGuard<'_, Vec<Arc<StderrCopy>>> {
        self.listed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for every copy listed ([`StderrCopy::wait`]); `now`: [`EXIT_DRAIN`] at most from the
    /// call, whatever they do. Their limits start together, so the copies waiting for their pipes
    /// cost at most [`DRAIN`] together, and nothing for a copy that already waited its [`DRAIN`]
    /// out.
    fn wait_for_all(&self, now: bool) {
        let deadline = now.then(|| Instant::now() + self.limits.exit);
        let copies: Vec<Arc<StderrCopy>> = self.listed().iter().map(Arc::clone).collect();
        for copy in &copies {
            copy.limit();
        }
        for copy in copies {
            copy.wait_until(deadline);
        }
    }
}

/// Starts the copy of a host's stderr to this process's ([`copy_lines`], [`to_stderr`]). The pipe
/// is read blocking (tokio gives it back so), whatever the host does to its end.
#[cfg(unix)]
fn forward(stderr: tokio::process::ChildStderr) -> Option<Arc<StderrCopy>> {
    let pipe = std::fs::File::from(stderr.into_owned_fd().ok()?);
    spawn_copy(pipe, to_stderr, &COPIES)
}

#[cfg(not(unix))]
fn forward(_stderr: tokio::process::ChildStderr) -> Option<Arc<StderrCopy>> {
    None
}

/// Copies `pipe` with `write` ([`copy_lines`]) on a thread of its own, the copy listed in
/// `copies`, and waited for by their limits, until it ends; `None` when no thread starts.
#[cfg(unix)]
fn spawn_copy(
    pipe: impl std::io::Read + std::os::fd::AsFd + Send + 'static,
    write: impl FnMut(&[u8]) -> bool + Send + 'static,
    copies: &'static StderrCopies,
) -> Option<Arc<StderrCopy>> {
    let copy = Arc::new(StderrCopy {
        limits: copies.limits,
        pipe: pipe.as_fd().try_clone_to_owned().ok(),
        ..StderrCopy::default()
    });
    copies.listed().push(Arc::clone(&copy));
    let ending = Ending(Arc::clone(&copy), copies);
    // A thread that does not start drops `ending`, which ends the copy.
    std::thread::Builder::new()
        .name("grida-fx host stderr".into())
        .spawn(move || copy_lines(pipe, &ending.0, write))
        .ok()
        .map(|_| copy)
}

/// Ends its copy when dropped, however the copy's thread stops: marks it ended and takes it off
/// its list.
#[cfg(unix)]
struct Ending(Arc<StderrCopy>, &'static StderrCopies);

#[cfg(unix)]
impl Drop for Ending {
    fn drop(&mut self) {
        self.1
            .listed()
            .retain(|listed| !Arc::ptr_eq(listed, &self.0));
        self.0.update(|copying| {
            copying.ended = true;
            copying.waiting_since = None;
        });
    }
}

/// Copies `pipe` to `write` until every holder has closed it, in whole lines (module doc), and
/// tells `copy` while it waits for the pipe and what it read. Once `write` fails nothing more is
/// written, but the pipe is still read, so whatever writes to it never blocks.
#[cfg(unix)]
fn copy_lines(
    mut pipe: impl std::io::Read,
    copy: &StderrCopy,
    mut write: impl FnMut(&[u8]) -> bool,
) {
    let mut buffer = vec![0u8; READ_SIZE];
    let mut held = Vec::new();
    let mut writing = true;
    loop {
        copy.update(|copying| copying.waiting_since = Some(Instant::now()));
        let read = pipe.read(&mut buffer);
        copy.update(|copying| {
            if let Some(since) = copying.waiting_since.take() {
                copying.waited += since.elapsed();
            }
            if let Ok(read) = &read {
                copying.took(*read, copy.limits.flood);
            }
        });
        match read {
            Ok(0) => break,
            Ok(read) => held.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        let whole = match held.iter().rposition(|&byte| byte == b'\n') {
            Some(end) => end + 1,
            None if held.len() >= LONGEST_LINE => held.len(),
            None => continue,
        };
        writing = writing && write(&held[..whole]);
        held.drain(..whole);
    }
    if writing && !held.is_empty() {
        write(&held);
    }
}

/// Writes `text` to this process's stderr under its lock ([`crate::stderr`]); whether it all was
/// written.
#[cfg(unix)]
fn to_stderr(text: &[u8]) -> bool {
    crate::stderr::write_whole(&mut std::io::stderr().lock(), text)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host::connection::NoIncoming;
    use std::io::Write;

    /// A pipe that reads `parts` one read at a time (each cut to the reader's buffer), then ends.
    struct Parts(std::collections::VecDeque<Vec<u8>>);

    impl Parts {
        fn of(parts: &[&[u8]]) -> Parts {
            Parts(parts.iter().map(|part| part.to_vec()).collect())
        }
    }

    impl std::io::Read for Parts {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let Some(mut part) = self.0.pop_front() else {
                return Ok(0);
            };
            let read = part.len().min(buffer.len());
            buffer[..read].copy_from_slice(&part[..read]);
            if read < part.len() {
                self.0.push_front(part.split_off(read));
            }
            Ok(read)
        }
    }

    /// What `pipe` comes to, write by write.
    fn copied(pipe: Parts) -> Vec<Vec<u8>> {
        let mut writes = Vec::new();
        copy_lines(pipe, &StderrCopy::default(), |text| {
            writes.push(text.to_vec());
            true
        });
        writes
    }

    #[test]
    fn a_copy_writes_whole_lines() {
        let writes = copied(Parts::of(&[
            b"alpha\nbe",
            b"ta\ngam",
            b"ma",
            b"\n",
            b"delta\nunfinished",
        ]));
        let writes: Vec<&[u8]> = writes.iter().map(Vec::as_slice).collect();
        assert_eq!(
            writes,
            [
                &b"alpha\n"[..],
                b"beta\n",
                b"gamma\n",
                b"delta\n",
                b"unfinished"
            ]
        );
    }

    #[test]
    fn a_copy_writes_a_line_longer_than_it_holds_in_parts() {
        let mut text = vec![b'x'; LONGEST_LINE + 6 * 1024];
        text.extend_from_slice(b"\nend\n");
        let writes = copied(Parts::of(&[&text]));
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].len(), LONGEST_LINE);
        assert_eq!(writes.concat(), text);
    }

    #[test]
    fn a_copy_that_cannot_write_still_reads_its_pipe_to_the_end() {
        let mut pipe = Parts::of(&[b"one\n", b"two\n", b"three\n"]);
        let mut calls = 0;
        copy_lines(&mut pipe, &StderrCopy::default(), |_| {
            calls += 1;
            false
        });
        assert_eq!(calls, 1);
        assert!(pipe.0.is_empty());
    }

    /// Limits short enough for tests, with wide margins between them: [`DRAIN`] half a second,
    /// [`DRAIN_IN_ALL`] two seconds, [`EXIT_DRAIN`] a second and a half.
    const QUICK: Limits = Limits {
        idle: Duration::from_millis(500),
        flood: Duration::from_secs(2),
        exit: Duration::from_millis(1500),
    };

    /// The pipe's writing end, and its copy with `write` on a thread listed in `copies`.
    fn piped(
        write: impl FnMut(&[u8]) -> bool + Send + 'static,
        copies: &'static StderrCopies,
    ) -> (std::io::PipeWriter, Arc<StderrCopy>) {
        let (reader, writer) = std::io::pipe().unwrap();
        (writer, spawn_copy(reader, write, copies).unwrap())
    }

    /// What a reader took.
    type Taken = Arc<Mutex<Vec<u8>>>;

    /// A reader that takes nothing for `first`, then everything; and what it took.
    fn reader_starting_after(first: Duration) -> (impl FnMut(&[u8]) -> bool + Send, Taken) {
        let taken = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&taken);
        let mut starting = true;
        let write = move |text: &[u8]| {
            if std::mem::take(&mut starting) {
                std::thread::sleep(first);
            }
            into.lock().unwrap().extend_from_slice(text);
            true
        };
        (write, taken)
    }

    /// A reader that takes nothing for three of [`QUICK`]'s idle limits, then everything.
    fn slow_reader() -> (impl FnMut(&[u8]) -> bool + Send, Taken) {
        reader_starting_after(QUICK.idle * 3)
    }

    /// `count` numbered lines.
    fn numbered(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|number| format!("line {number:04}\n").into_bytes())
            .collect()
    }

    /// Waits up to 3 seconds for `copy` to end.
    fn ends(copy: &StderrCopy) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if copy.state().ended {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn a_copy_is_waited_for_while_it_writes_to_a_slow_reader() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        let (write, taken) = slow_reader();
        let (mut writer, copy) = piped(write, &LISTED);
        let text = numbered(200);
        writer.write_all(&text).unwrap();
        // The host has ended, its text still in the pipe.
        drop(writer);
        let start = Instant::now();
        assert!(copy.wait());
        let took = start.elapsed();
        assert!(took > QUICK.idle, "{took:?}");
        assert_eq!(*taken.lock().unwrap(), text);
        assert!(LISTED.listed().is_empty());
    }

    /// A reader that starts reading only long after the host ended (`2>&1 | less`, its first
    /// screen read at leisure) still gets everything the host left: waiting for its copy is bound
    /// by what the host left, not by the time it takes.
    #[test]
    fn a_copy_is_waited_for_however_late_its_reader_starts() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        let (write, taken) = reader_starting_after(QUICK.flood + QUICK.idle * 2);
        let (mut writer, copy) = piped(write, &LISTED);
        // Less than one read takes, so the pipe is written whole before the host ends.
        let text = numbered(1200);
        writer.write_all(&text).unwrap();
        drop(writer);
        let start = Instant::now();
        assert!(copy.wait());
        let took = start.elapsed();
        assert!(took > QUICK.flood, "{took:?}");
        assert_eq!(*taken.lock().unwrap(), text);
        // And so does the command's own wait before it exits.
        static AT_EXIT: StderrCopies = StderrCopies::with(QUICK);
        let (write, taken) = reader_starting_after(QUICK.flood + QUICK.idle * 2);
        let (mut writer, copy) = piped(write, &AT_EXIT);
        writer.write_all(&text).unwrap();
        drop(writer);
        AT_EXIT.wait_for_all(false);
        assert!(copy.state().ended);
        assert_eq!(*taken.lock().unwrap(), text);
    }

    #[test]
    fn a_copy_whose_pipe_is_held_idle_is_given_up_after_its_second() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        let taken = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&taken);
        let (mut writer, copy) = piped(
            move |text| {
                into.lock().unwrap().extend_from_slice(text);
                true
            },
            &LISTED,
        );
        writer.write_all(b"before\n").unwrap();
        // `writer` stays open: a process that left the host's group holds the pipe.
        let start = Instant::now();
        assert!(!copy.wait());
        let took = start.elapsed();
        assert!(
            took >= QUICK.idle * 9 / 10 && took < QUICK.idle * 4,
            "{took:?}"
        );
        assert_eq!(*taken.lock().unwrap(), b"before\n");
        // Waiting again, as the command does before it exits, keeps the time already spent.
        let start = Instant::now();
        LISTED.wait_for_all(false);
        let took = start.elapsed();
        assert!(took < QUICK.idle / 2, "{took:?}");
        // Once the holder closes the pipe, the copy ends and leaves the list.
        drop(writer);
        assert!(ends(&copy));
        assert!(LISTED.listed().is_empty());
    }

    #[test]
    fn a_copy_whose_pipe_is_held_by_a_process_printing_now_and_then_is_given_up_too() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        let (mut writer, copy) = piped(|_| true, &LISTED);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let printing = std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                let _ = writer.write_all(b"tick\n");
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let start = Instant::now();
        assert!(!copy.wait());
        let took = start.elapsed();
        stop.store(true, Ordering::SeqCst);
        printing.join().unwrap();
        assert!(
            took >= QUICK.idle * 9 / 10 && took < QUICK.idle * 4,
            "{took:?}"
        );
        assert!(ends(&copy));
    }

    #[test]
    fn the_copies_left_are_waited_for_together() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        // A host that was never waited for, its text still on the way to a slow reader...
        let (write, taken) = slow_reader();
        let (mut writer, slow) = piped(write, &LISTED);
        let text = numbered(100);
        writer.write_all(&text).unwrap();
        drop(writer);
        // ...and one whose pipe a process that left its group holds.
        let (held, idle) = piped(|_| true, &LISTED);
        let start = Instant::now();
        LISTED.wait_for_all(false);
        let took = start.elapsed();
        assert!(
            took > QUICK.idle && took < QUICK.idle * 3 + Duration::from_secs(2),
            "{took:?}"
        );
        assert!(slow.state().ended);
        assert_eq!(*taken.lock().unwrap(), text);
        assert!(!idle.state().ended);
        drop(held);
        assert!(ends(&idle));
        assert!(LISTED.listed().is_empty());
    }

    /// `wait` run on a thread of its own: how it went and how long it took, once it has returned.
    fn waiting<T: Send + 'static>(
        wait: impl FnOnce() -> T + Send + 'static,
    ) -> std::sync::mpsc::Receiver<(T, Duration)> {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let start = Instant::now();
            let went = wait();
            let _ = done.send((went, start.elapsed()));
        });
        finished
    }

    /// A command interrupted while a copy is blocked on a reader that does not read still ends:
    /// waiting for the copies left gives up [`EXIT_DRAIN`] after it starts, whatever they do.
    #[test]
    fn the_copies_left_are_given_up_at_the_exit_whatever_they_do() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        // A reader that takes nothing until it is let go.
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (mut writer, blocked) = piped(
            move |_| {
                let _ = released.recv();
                true
            },
            &LISTED,
        );
        writer.write_all(b"stuck\n").unwrap();
        drop(writer);
        let finished = waiting(|| LISTED.wait_for_all(true));
        let waited = finished.recv_timeout(QUICK.exit * 4);
        let ended = blocked.state().ended;
        drop(release);
        let ((), took) = waited.expect("waiting for the copies never gave up");
        assert!(
            took >= QUICK.exit * 9 / 10 && took < QUICK.exit * 3,
            "{took:?}"
        );
        assert!(!ended);
        // Let go, the copy writes what it holds and ends.
        assert!(ends(&blocked));
        assert!(LISTED.listed().is_empty());
    }

    /// A process that left the host's group and prints faster than the reader takes never lets
    /// the copy wait for its pipe: once the copy has read more than the host left, waiting for it
    /// gives up [`DRAIN_IN_ALL`] later.
    #[test]
    fn a_copy_that_always_has_text_to_write_is_given_up_in_the_end() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        let (mut writer, copy) = piped(
            |_| {
                std::thread::sleep(Duration::from_millis(20));
                true
            },
            &LISTED,
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        // About a megabyte a second, more than the reader takes (16 KiB every 20 ms at most).
        let printing = std::thread::spawn(move || {
            let line = format!("{}\n", "f".repeat(1023));
            while !stopped.load(Ordering::SeqCst) {
                let _ = writer.write_all(line.as_bytes());
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let waited = Arc::clone(&copy);
        let finished = waiting(move || waited.wait());
        let waited = finished.recv_timeout(QUICK.flood * 4);
        stop.store(true, Ordering::SeqCst);
        printing.join().unwrap();
        let (ended, took) = waited.expect("waiting for the copy never gave up");
        assert!(!ended);
        assert!(
            took >= QUICK.flood * 9 / 10 && took < QUICK.flood * 2,
            "{took:?}"
        );
        assert!(ends(&copy));
    }

    /// What the host left, measured when the copy's limits start: what its pipe holds then.
    #[test]
    fn what_the_host_left_is_measured_when_waiting_starts() {
        static LISTED: StderrCopies = StderrCopies::with(QUICK);
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (mut writer, copy) = piped(
            move |_| {
                let _ = released.recv();
                true
            },
            &LISTED,
        );
        // The copy takes one read, then is held by its reader; the rest stays in the pipe.
        writer.write_all(b"first\n").unwrap();
        while copy.state().read == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        let rest = numbered(500);
        writer.write_all(&rest).unwrap();
        drop(writer);
        let limit = copy.limit();
        assert_eq!(
            limit.left,
            (b"first\n".len() + rest.len() + READ_SIZE) as u64
        );
        assert!(limit.by.is_none());
        drop(release);
        assert!(ends(&copy));
        // All it read was what the host left: no clock was started.
        assert!(copy.state().limit.is_some_and(|limit| limit.by.is_none()));
    }

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

        if std::env::var("GRIDA_FX_GROUP_KILL_CHILD").as_deref() != Ok("1") {
            let dir = tempfile::tempdir().unwrap();
            let stub = dir.path().join("kill");
            grida_fx_providers::testing::write_program(
                &stub,
                "#!/bin/sh\nprintf '%s' \"$*\" >> \"$GRIDA_FX_GROUP_KILL_LOG\"\nexit 1\n",
            );
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
        grida_fx_providers::testing::write_program(
            &python,
            &format!("#!/bin/sh\nexec python3 \"$(dirname \"$0\")/{name}.py\"\n"),
        );
        let spec =
            HostSpec::python(python, format!("python-{name}"), dir.path(), Vec::new()).unchecked();
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
        let spec = HostSpec::python(
            "no-such-python-for-fx",
            "no-such-python-for-fx",
            dir.path(),
            Vec::new(),
        )
        .unchecked();
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure,
            "cannot start the Python node host with no-such-python-for-fx: no such file; set GRIDA_FX_PYTHON to a Python that has the grida package"
        );
    }

    #[tokio::test]
    async fn a_python_before_3_11_is_refused_before_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python-old");
        // Asked with `-c`, it answers as Python 3.9.6; started as a host, it leaves a mark.
        grida_fx_providers::testing::write_program(
            &python,
            "#!/bin/sh\nif [ \"$1\" = -c ]; then echo 'fx-python 3.9.6 1'; exit 0; fi\ntouch \
             \"$(dirname \"$0\")/started.txt\"\nexit 2\n",
        );
        let spec = HostSpec::python(python, "python-old", dir.path(), Vec::new());
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure,
            "python-old is Python 3.9.6; FX's node host needs Python 3.11 or later with grida: \
             run uv sync or pip install grida in the project, or set GRIDA_FX_PYTHON"
        );
        assert!(!dir.path().join("started.txt").exists());
    }

    #[tokio::test]
    async fn a_missing_node_is_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let spec = HostSpec::typescript(
            "no-such-node-for-fx",
            dir.path().join("host-entry.js"),
            "no-such-node-for-fx",
            dir.path(),
            Vec::new(),
        )
        .unchecked();
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure,
            "cannot start the TypeScript node host with no-such-node-for-fx: no such file; set \
             GRIDA_FX_NODE to Node 24 or later"
        );
    }

    /// A Node at `<dir>/node-<name>` that answers `-e` with `answer` and otherwise leaves
    /// `started.txt` behind.
    fn node_answering(dir: &Path, name: &str, answer: &str) -> HostSpec {
        let node = dir.join(format!("node-{name}"));
        grida_fx_providers::testing::write_program(
            &node,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = -e ]; then echo '{answer}'; exit 0; fi\ntouch \
                 \"$(dirname \"$0\")/started.txt\"\nexit 2\n"
            ),
        );
        HostSpec::typescript(
            node,
            dir.join("host-entry.js"),
            format!("node-{name}"),
            dir,
            Vec::new(),
        )
    }

    #[tokio::test]
    async fn a_node_that_cannot_host_is_refused_before_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        for (name, answer, refusal) in [
            (
                "old",
                "fx-node 22.12.0 1",
                "node-old is Node 22.12.0; FX's TypeScript node host needs Node 24 or later: \
                 install it, or set GRIDA_FX_NODE",
            ),
            (
                "bun",
                "fx-node 24.3.0 0",
                "node-bun is not Node.js 24 or later with module.registerHooks; FX's TypeScript \
                 node host needs it: install Node 24 or later, or set GRIDA_FX_NODE",
            ),
        ] {
            let spec = node_answering(dir.path(), name, answer);
            let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
                .await
                .err()
                .unwrap();
            assert_eq!(failure, refusal);
            assert!(
                !dir.path().join("started.txt").exists(),
                "{name} was started"
            );
        }
        // A Node that answers 24 with its hooks is started (and here fails as a host would).
        let spec = node_answering(dir.path(), "new", "fx-node 24.14.0 1");
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure,
            "the TypeScript node host (node-new) exited with status 2 before it answered \
             initialize; set GRIDA_FX_NODE to Node 24 or later"
        );
        assert!(dir.path().join("started.txt").exists());
    }

    /// A host that writes its argv, its working directory, whether `PYTHONSAFEPATH` is set, and
    /// the `initialize` params to `seen.json`, then answers as a TypeScript host of `@grida/fx`
    /// `sdk` speaking `protocol`, and takes `shutdown` and `exit`.
    const RECORDING: &str = r#"
import sys
protocol, sdk = sys.argv[1], sys.argv[2]
message = read()
with open(os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), "seen.json"), "w") as f:
    json.dump({"argv": sys.argv[3:], "cwd": os.getcwd(),
               "safe_path": os.environ.get("PYTHONSAFEPATH"), "params": message["params"]}, f)
write({"jsonrpc": "2.0", "id": message["id"], "result": {
    "protocol": message["params"]["protocol"] if protocol == "same" else protocol,
    "host": {"language": "typescript", "version": "24.14.0", "sdk_version": sdk}}})
"#;

    /// A TypeScript host spec whose Node is a shell running [`RECORDING`] with `protocol` and
    /// `sdk`, passing on the arguments the engine gives it.
    fn recording(dir: &Path, protocol: &str, sdk: &str) -> HostSpec {
        std::fs::write(
            dir.join("recording.py"),
            format!("{FRAMES}{RECORDING}{POLITE_LOOP}"),
        )
        .unwrap();
        let node = dir.join("node-recording");
        grida_fx_providers::testing::write_program(
            &node,
            &format!(
                "#!/bin/sh\nexec python3 \"$(dirname \"$0\")/recording.py\" {protocol} {sdk} \"$@\"\n"
            ),
        );
        let project = dir.join("acme");
        std::fs::create_dir_all(&project).unwrap();
        HostSpec::typescript(
            node,
            dir.join("sdk/dist/host-entry.js"),
            "node-recording",
            project,
            vec![
                "acme_lib".into(),
                "@acme/shapes".into(),
                "AcmePy".into(),
                "acme-ts".into(),
            ],
        )
        .unchecked()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_typescript_host_is_node_running_its_host_program() {
        if !have_python3() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let spec = recording(dir.path(), "same", "0.6.0");
        let host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        assert_eq!(host.info().host.language, "typescript");
        assert_eq!(host.info().host.sdk_version, "0.6.0");
        assert_eq!(host.shutdown().await.and_then(|s| s.code()), Some(0));
        let seen: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("seen.json")).unwrap()).unwrap();
        // `<node> <host-entry.js>`: no Python options, no PYTHONSAFEPATH.
        assert_eq!(
            seen["argv"],
            serde_json::json!([dir.path().join("sdk/dist/host-entry.js")])
        );
        if std::env::var_os("PYTHONSAFEPATH").is_none() {
            assert_eq!(seen["safe_path"], Value::Null);
        }
        assert_eq!(
            std::fs::canonicalize(seen["cwd"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(&spec.project_root).unwrap()
        );
        // Only the source packages an npm name can name.
        assert_eq!(
            seen["params"]["sources"],
            serde_json::json!(["acme_lib", "@acme/shapes", "acme-ts"])
        );
        assert_eq!(
            seen["params"]["project_root"],
            spec.project_root.to_str().unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_typescript_host_of_another_protocol_names_its_sdk() {
        if !have_python3() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let spec = recording(dir.path(), "fx-node-protocol-v2", "0.9.0");
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert_eq!(
            failure,
            format!(
                "the project's @grida/fx 0.9.0 speaks fx-node-protocol-v2 and grida-fx \
                 {ENGINE_VERSION} speaks fx-node-protocol-v1: upgrade grida-fx"
            )
        );
        let spec = recording(dir.path(), "fx-node-protocol-v0", "0.5.0");
        let failure = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .err()
            .unwrap();
        assert!(
            failure.starts_with("the project's @grida/fx 0.5.0 speaks fx-node-protocol-v0 and")
                && failure.ends_with(": upgrade @grida/fx"),
            "{failure}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_python_host_gets_the_python_source_packages_and_its_safe_path() {
        if !have_python3() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let typescript = recording(dir.path(), "same", "0");
        let spec = HostSpec::python(
            typescript.program.clone(),
            "python-recording",
            typescript.project_root.clone(),
            typescript.sources.clone(),
        )
        .unchecked();
        let host = HostProcess::start(&spec, Arc::new(NoIncoming))
            .await
            .unwrap();
        host.shutdown().await;
        let seen: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("seen.json")).unwrap()).unwrap();
        assert_eq!(
            seen["argv"],
            serde_json::json!(["-P", "-m", "grida.fx.host"])
        );
        assert_eq!(
            seen["params"]["sources"],
            serde_json::json!(["acme_lib", "AcmePy"])
        );
        if std::env::var_os("PYTHONSAFEPATH").is_none() {
            assert_eq!(seen["safe_path"], SAFE_PATH_MARK);
        }
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
