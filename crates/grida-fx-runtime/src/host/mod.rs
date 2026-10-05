//! Node hosts (protocol.md §1, §2): the Python host process and its session.
//!
//! [`PythonHost`] implements `grida_fx_core::host::NodeHost`. It starts the host lazily, on the
//! first `describe` or `build`, so planning a project without node modules never needs Python:
//! `<python> -P -m grida.fx.host` with its working directory at the project root, stdin/stdout
//! piped for the protocol and stderr inherited (free-form log text). `-P` (Python 3.11 and later)
//! keeps the working directory off `sys.path`, so a project module named like a standard library
//! module or like `grida` cannot replace the host's own imports before `initialize` puts the root
//! on `sys.path`. The engine also sets `PYTHONSAFEPATH` to [`SAFE_PATH_MARK`] when its own
//! environment leaves it unset, for an interpreter wrapper that drops `-P`; the host removes that
//! value before it loads user code, so programs user code starts see the user's environment.
//!
//! The engine sends `initialize`
//! (`{protocol, engine: {name: "grida-fx", version}, project_root, sources}`), checks the
//! answer's protocol (a mismatch, or a `protocol_mismatch` error, is
//! `the project's grida <sdk_version> speaks <host protocol> and grida-fx <version> speaks
//! fx-node-protocol-v1: upgrade grida` or `…: upgrade grida-fx`), then serves requests one at a
//! time. A host that cannot start is `HostFailure::Unavailable` with a sentence naming the
//! interpreter and `GRIDA_FX_PYTHON`. Dropping the host sends `shutdown` then the `exit`
//! notification and waits up to 5 seconds before killing it.
//!
//! The interpreter is the one given to [`PythonHost::with_python`], else
//! [`locate::python_interpreter`] over the process environment. It is named in messages as the
//! user would write it: a path inside the project relative to the project root. A host that
//! failed to start, or broke the protocol, stays failed for its project: later calls give the
//! same sentence without starting it again, until `open_project` names another project. A host
//! request during `describe` or `build` is answered `-32601`.
//!
//! Tests: a fake host script (a small Python program written to a temp folder, run with
//! `python3`) answering initialize/describe/build/shutdown; a host that exits early; a protocol
//! mismatch. Skip the process tests when `python3` is not on PATH.

pub mod locate;
pub mod session;

use grida_fx_core::host::{HostFailure, NodeHost};
use grida_fx_core::{ENGINE_NAME, ENGINE_VERSION};
use grida_fx_protocol::{
    BuildParams, BuildResult, DescribeParams, DescribeResult, EngineInfo, ErrorCode, ErrorData,
    InitializeParams, InitializeResult, PROTOCOL, method,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use session::{NoRequests, Session, SessionError};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The interpreter's arguments: the host module, with the safe-path option (protocol.md §1).
const HOST_ARGS: [&str; 3] = ["-P", "-m", "grida.fx.host"];

/// How long the engine waits for a host to answer `shutdown`, and then to exit after `exit`.
const GRACE: Duration = Duration::from_secs(5);

/// The `PYTHONSAFEPATH` value the engine sets for a host when its own environment has none; the
/// host removes a `PYTHONSAFEPATH` of exactly this value before it loads user code.
pub const SAFE_PATH_MARK: &str = "grida-fx";

/// What to tell a person whose Python cannot host nodes.
const PYTHON_ADVICE: &str = "set GRIDA_FX_PYTHON to a Python that has the grida package";

/// The Python node host of one project, started on first use.
pub struct PythonHost {
    /// Set by `open_project`; a call before it is `Unavailable`.
    project_root: Option<PathBuf>,
    sources: Vec<String>,
    /// The interpreter, when the caller chose it; else [`locate::python_interpreter`].
    python: Option<PathBuf>,
    child: Option<Child>,
    session: Option<Session>,
    /// What `initialize` answered.
    pub info: Option<InitializeResult>,
    /// Why the host of the open project cannot serve, once it failed to start or broke.
    failure: Option<String>,
}

impl PythonHost {
    /// A host with no project yet (see `NodeHost::open_project`).
    pub fn new() -> PythonHost {
        PythonHost {
            project_root: None,
            sources: Vec::new(),
            python: None,
            child: None,
            session: None,
            info: None,
            failure: None,
        }
    }

    /// Uses this interpreter instead of locating one.
    pub fn with_python(mut self, python: PathBuf) -> PythonHost {
        self.python = Some(python);
        self
    }

    /// The interpreter the host runs, or would run, for the open project: the one given to
    /// [`PythonHost::with_python`], else the one [`locate::python_interpreter`] finds. `None`
    /// when neither an interpreter nor a project is set.
    pub fn interpreter(&self) -> Option<PathBuf> {
        if let Some(python) = &self.python {
            return Some(python.clone());
        }
        let root = self.project_root.as_ref()?;
        Some(locate::python_interpreter(root, &|name| {
            std::env::var(name).ok()
        }))
    }

    /// The interpreter as messages name it: relative to the project root when it lies inside the
    /// project (`.venv/bin/python`), else as given (`python3`, a `GRIDA_FX_PYTHON` value).
    pub fn interpreter_label(&self) -> Option<String> {
        let python = self.interpreter()?;
        Some(match &self.project_root {
            Some(root) => label(&python, root),
            None => python.display().to_string(),
        })
    }

    /// Starts the host when it is not running and returns what `initialize` answered (for
    /// `doctor`'s Python host line).
    pub fn host_info(&mut self) -> Result<InitializeResult, HostFailure> {
        self.start()?;
        self.info.clone().ok_or_else(|| {
            HostFailure::Unavailable("the node host has not answered initialize".into())
        })
    }

    /// Starts the process and initializes the session, once.
    fn start(&mut self) -> Result<&mut Session, HostFailure> {
        if self.session.is_none() {
            if let Some(failure) = &self.failure {
                return Err(HostFailure::Unavailable(failure.clone()));
            }
            let Some(root) = self.project_root.clone() else {
                return Err(HostFailure::Unavailable(
                    "the node host has no project: open one before describe or build".into(),
                ));
            };
            match self.launch(&root) {
                Ok((child, session, info)) => {
                    self.child = Some(child);
                    self.session = Some(session);
                    self.info = Some(info);
                }
                Err(message) => {
                    self.failure = Some(message.clone());
                    return Err(HostFailure::Unavailable(message));
                }
            }
        }
        Ok(self.session.as_mut().expect("a started host has a session"))
    }

    /// Spawns `<python> -P -m grida.fx.host` and initializes it. Errors are sentences.
    fn launch(&self, root: &Path) -> Result<(Child, Session, InitializeResult), String> {
        let python = self
            .interpreter()
            .unwrap_or_else(|| PathBuf::from("python3"));
        let shown = label(&python, root);
        let Some(project_root) = root.to_str() else {
            return Err(
                "the project folder's path is not UTF-8, which the node protocol needs".into(),
            );
        };
        let mut command = Command::new(program(&python));
        command.args(HOST_ARGS).current_dir(root);
        if std::env::var_os("PYTHONSAFEPATH").is_none_or(|value| value.is_empty()) {
            command.env("PYTHONSAFEPATH", SAFE_PATH_MARK);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot start the Python node host with {shown}: {}; {PYTHON_ADVICE}",
                    grida_fx_core::error::io_reason(&e)
                )
            })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            close(&mut child, None, false);
            return Err(format!(
                "cannot start the Python node host with {shown}: its streams are not piped"
            ));
        };
        let mut session = Session::new(Box::new(BufReader::new(stdout)), Box::new(stdin));
        let params = InitializeParams {
            protocol: PROTOCOL.into(),
            engine: EngineInfo {
                name: ENGINE_NAME.into(),
                version: ENGINE_VERSION.into(),
            },
            project_root: project_root.into(),
            sources: self.sources.clone(),
        };
        let params = serde_json::to_value(&params).map_err(|e| e.to_string())?;
        let answer = session.request(method::INITIALIZE, Some(params), &mut NoRequests);
        let failure = match answer {
            Ok(value) => match serde_json::from_value::<InitializeResult>(value) {
                Ok(info) if info.protocol == PROTOCOL => return Ok((child, session, info)),
                Ok(info) => mismatch(Some(&info.host.sdk_version), &info.protocol),
                Err(e) => format!(
                    "the Python node host ({shown}) answered initialize with something FX cannot read: {e}"
                ),
            },
            Err(SessionError::Rpc(error)) if error.kind() == Some(ErrorCode::ProtocolMismatch) => {
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
            Err(SessionError::Rpc(error)) => format!(
                "the Python node host ({shown}) refused initialize: {}",
                error.message
            ),
            Err(SessionError::Closed) => {
                let status = close(&mut child, Some(session), false);
                return Err(format!(
                    "the Python node host ({shown}) exited{} before it answered initialize; {PYTHON_ADVICE}",
                    exited_with(status)
                ));
            }
            Err(error) => format!("the Python node host ({shown}) broke the protocol: {error}"),
        };
        // protocol.md §2: on a mismatch the engine reports both versions, then ends the host.
        close(&mut child, Some(session), false);
        Err(failure)
    }

    /// Ends the session: `shutdown`, `exit`, wait (kill after 5 seconds). Idempotent.
    pub fn shutdown(&mut self) -> Result<(), HostFailure> {
        let session = self.session.take();
        if let Some(mut child) = self.child.take() {
            let polite = session.as_ref().is_some_and(Session::is_open);
            close(&mut child, session, polite);
        }
        Ok(())
    }

    /// Sends one request of the engine's and reads its result.
    fn call<P, R>(&mut self, method: &str, params: &P) -> Result<R, HostFailure>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params).map_err(|e| {
            HostFailure::Unavailable(format!("the {method} request cannot be written: {e}"))
        })?;
        let answer = self.start()?.request(method, Some(params), &mut NoRequests);
        match answer {
            Ok(value) => serde_json::from_value(value).map_err(|e| {
                self.broke(format!(
                    "the Python node host answered {method} with a result FX cannot read: {e}"
                ))
            }),
            Err(SessionError::Rpc(error)) => Err(HostFailure::Rpc(error)),
            Err(SessionError::Closed) => {
                let status = self.end_process();
                let message = format!(
                    "the node host exited{} while answering {method}",
                    exited_with(status)
                );
                self.failure = Some(message.clone());
                Err(HostFailure::Unavailable(message))
            }
            Err(SessionError::Protocol(message)) => Err(self.broke(format!(
                "the node host broke the protocol while answering {method}: {message}"
            ))),
            Err(SessionError::Io(e)) => Err(self.broke(format!(
                "cannot talk to the node host while it answers {method}: {e}"
            ))),
        }
    }

    /// Ends a host that cannot go on and keeps the reason for later calls.
    fn broke(&mut self, message: String) -> HostFailure {
        self.end_process();
        self.failure = Some(message.clone());
        HostFailure::Unavailable(message)
    }

    /// Ends the process, politely when the session still works; its exit status when known.
    fn end_process(&mut self) -> Option<ExitStatus> {
        let session = self.session.take();
        let mut child = self.child.take()?;
        let polite = session.as_ref().is_some_and(Session::is_open);
        close(&mut child, session, polite)
    }
}

impl Default for PythonHost {
    fn default() -> Self {
        PythonHost::new()
    }
}

impl NodeHost for PythonHost {
    fn open_project(&mut self, root: &Path, sources: &[String]) -> Result<(), HostFailure> {
        let root = std::path::absolute(root).map_err(|e| {
            HostFailure::Unavailable(format!(
                "the project folder cannot be named: {}",
                grida_fx_core::error::io_reason(&e)
            ))
        })?;
        if self.project_root.as_ref() == Some(&root) && self.sources == sources {
            return Ok(());
        }
        self.shutdown()?;
        self.project_root = Some(root);
        self.sources = sources.to_vec();
        self.info = None;
        self.failure = None;
        Ok(())
    }

    fn describe(&mut self, params: &DescribeParams) -> Result<DescribeResult, HostFailure> {
        self.call(method::DESCRIBE, params)
    }

    fn build(&mut self, params: &BuildParams) -> Result<BuildResult, HostFailure> {
        self.call(method::BUILD, params)
    }
}

impl Drop for PythonHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// The program to spawn: a relative path with folders is made absolute against the engine's
/// working directory, since the host starts in the project root; a bare name is left for the
/// `PATH` lookup. Symbolic links are kept (a virtual environment's `python` is one).
fn program(python: &Path) -> PathBuf {
    if python.is_relative() && python.components().count() > 1 {
        if let Ok(absolute) = std::path::absolute(python) {
            return absolute;
        }
    }
    python.to_path_buf()
}

/// A path as messages name it: POSIX and relative to `root` inside it, else as given.
fn label(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(inside) if !inside.as_os_str().is_empty() => inside
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => path.display().to_string(),
    }
}

/// The sentence for a host that speaks another protocol (protocol.md §2 "Version mismatch").
fn mismatch(sdk_version: Option<&str>, host_protocol: &str) -> String {
    let grida = match sdk_version {
        Some(version) if !version.is_empty() => format!("grida {version}"),
        _ => "grida".to_string(),
    };
    format!(
        "the project's {grida} speaks {host_protocol} and grida-fx {ENGINE_VERSION} speaks {PROTOCOL}: upgrade {}",
        upgrade(host_protocol)
    )
}

/// Which side to upgrade: grida-fx when the host speaks a newer version of this protocol,
/// grida otherwise.
fn upgrade(host_protocol: &str) -> &'static str {
    let version = |protocol: &str| -> Option<u64> {
        protocol
            .strip_prefix("fx-node-protocol-v")?
            .parse::<u64>()
            .ok()
    };
    match (version(PROTOCOL), version(host_protocol)) {
        (Some(ours), Some(theirs)) if theirs > ours => "grida-fx",
        _ => "grida",
    }
}

/// ` with status <n>`, or nothing when the status is not known.
fn exited_with(status: Option<ExitStatus>) -> String {
    match status.and_then(|s| s.code()) {
        Some(code) => format!(" with status {code}"),
        None => String::new(),
    }
}

/// Ends a host process. Politely: `shutdown` and `exit` are sent from a helper thread, so a
/// host that never answers cannot hang the engine; the engine waits up to [`GRACE`] for that,
/// then up to [`GRACE`] for the process to exit. Otherwise the session is dropped, which closes
/// the host's input, and the process gets [`GRACE`] to exit. A host still running after that is
/// killed. Returns the exit status when it is known.
fn close(child: &mut Child, session: Option<Session>, polite: bool) -> Option<ExitStatus> {
    let mut sent = true;
    match session {
        Some(mut session) if polite => {
            let (done, finished) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("grida-fx-host-shutdown".into())
                .spawn(move || {
                    let _ = session.request(method::SHUTDOWN, None, &mut NoRequests);
                    let _ = session.notify(method::EXIT, None);
                    drop(session);
                    let _ = done.send(());
                });
            sent = spawned.is_ok() && finished.recv_timeout(GRACE).is_ok();
        }
        other => drop(other),
    }
    if sent {
        if let Some(status) = wait_for(child, GRACE) {
            return Some(status);
        }
    }
    let _ = child.kill();
    child.wait().ok()
}

/// Waits for a process to exit, up to `limit`.
fn wait_for(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if start.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatch_sentences() {
        assert_eq!(
            mismatch(Some("0.1.0a1"), "fx-node-protocol-v0"),
            format!(
                "the project's grida 0.1.0a1 speaks fx-node-protocol-v0 and grida-fx {ENGINE_VERSION} speaks fx-node-protocol-v1: upgrade grida"
            )
        );
        assert_eq!(
            mismatch(Some("0.3.0"), "fx-node-protocol-v2"),
            format!(
                "the project's grida 0.3.0 speaks fx-node-protocol-v2 and grida-fx {ENGINE_VERSION} speaks fx-node-protocol-v1: upgrade grida-fx"
            )
        );
        assert!(
            mismatch(None, "something-else")
                .starts_with("the project's grida speaks something-else and")
        );
        assert!(mismatch(None, "something-else").ends_with(": upgrade grida"));
        assert!(mismatch(Some(""), "fx-node-protocol-v12").ends_with(": upgrade grida-fx"));
    }

    #[test]
    fn labels() {
        let root = Path::new("/work/acme");
        assert_eq!(
            label(Path::new("/work/acme/.venv/bin/python"), root),
            ".venv/bin/python"
        );
        assert_eq!(label(Path::new("python3"), root), "python3");
        assert_eq!(
            label(Path::new("/opt/py/bin/python"), root),
            "/opt/py/bin/python"
        );
        assert_eq!(label(Path::new("/work/acme"), root), "/work/acme");
    }

    #[test]
    fn programs() {
        assert_eq!(program(Path::new("python3")), PathBuf::from("python3"));
        assert_eq!(
            program(Path::new("/opt/py/bin/python")),
            PathBuf::from("/opt/py/bin/python")
        );
        let relative = program(Path::new("python/.venv/bin/python"));
        assert!(relative.is_absolute());
        assert!(relative.ends_with("python/.venv/bin/python"));
    }

    #[test]
    fn a_host_without_a_project_is_unavailable() {
        let mut host = PythonHost::new();
        assert_eq!(host.interpreter(), None);
        let failure = host
            .describe(&DescribeParams {
                targets: Vec::new(),
                builtins: false,
            })
            .unwrap_err();
        assert!(
            matches!(&failure, HostFailure::Unavailable(m) if m.contains("no project")),
            "{failure:?}"
        );
        host.shutdown().unwrap();
        host.shutdown().unwrap();
    }

    #[test]
    fn opening_a_project_names_the_interpreter() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = PythonHost::new().with_python(dir.path().join(".venv/bin/python"));
        host.open_project(dir.path(), &[]).unwrap();
        assert_eq!(
            host.interpreter_label().as_deref(),
            Some(".venv/bin/python")
        );
        // The same project again keeps the host; nothing was started.
        host.open_project(dir.path(), &[]).unwrap();
        assert!(host.child.is_none());
    }

    #[test]
    fn a_missing_interpreter_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = PythonHost::new().with_python(PathBuf::from("no-such-python-for-fx"));
        host.open_project(dir.path(), &[]).unwrap();
        let params = DescribeParams {
            targets: Vec::new(),
            builtins: false,
        };
        let failure = host.describe(&params).unwrap_err();
        assert_eq!(
            failure,
            HostFailure::Unavailable(
                "cannot start the Python node host with no-such-python-for-fx: no such file; set GRIDA_FX_PYTHON to a Python that has the grida package"
                    .into()
            )
        );
        // The failure is kept for the project.
        assert_eq!(host.describe(&params).unwrap_err(), failure);
        assert!(host.info.is_none());
    }
}
