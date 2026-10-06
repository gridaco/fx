//! Node hosts (spec/protocol.md §1, §2): host processes, their connections, the pool that runs
//! bodies, and the planning host.
//!
//! - [`connection`]: the asynchronous JSON-RPC connection, with requests in both directions;
//! - [`process`]: one started, initialized host process ([`process::HostProcess`]);
//! - [`pool`]: the hosts that run bodies, one job each;
//! - [`locate`]: which interpreter hosts a project's nodes.
//!
//! [`PythonHost`] implements `grida_fx_core::host::NodeHost` for planning (`describe`, `build`).
//! It starts the host lazily, on the first `describe` or `build`, so planning a project without
//! node modules never needs Python: `<python> -P -m grida.fx.host` with its working directory at
//! the project root, stdin/stdout piped for the protocol and stderr inherited (free-form log
//! text). `-P` (Python 3.11 and later) keeps the working directory off `sys.path`, so a project
//! module named like a standard library module or like `grida` cannot replace the host's own
//! imports before `initialize` puts the root on `sys.path`. The engine also sets
//! `PYTHONSAFEPATH` to [`SAFE_PATH_MARK`] when its own environment leaves it unset, for an
//! interpreter wrapper that drops `-P`; the host removes that value before it loads user code,
//! so programs user code starts see the user's environment.
//!
//! The engine sends `initialize`
//! (`{protocol, engine: {name: "grida-fx", version}, project_root, sources}`), checks the
//! answer's protocol (a mismatch, or a `protocol_mismatch` error, is
//! `the project's grida <sdk_version> speaks <host protocol> and grida-fx <version> speaks
//! fx-node-protocol-v1: upgrade grida` or `…: upgrade grida-fx`), then sends one request at a
//! time. A host that cannot start is `HostFailure::Unavailable` with a sentence naming the
//! interpreter and `GRIDA_FX_PYTHON`. Dropping the host sends `shutdown` then the `exit`
//! notification and waits up to 5 seconds before killing it.
//!
//! The interpreter is the one given to [`PythonHost::with_python`], else
//! [`locate::python_interpreter`] over the process environment, with the planning project given
//! to [`PythonHost::with_planning_root`] as its fallback. It is named in messages as the
//! user would write it ([`label`]): a path inside the project relative to the project root. A
//! host that failed to start, or broke the protocol, stays failed for its project: later calls
//! give the same sentence without starting it again, until `open_project` names another
//! project. A host request during `describe` or `build` is answered `-32601`.
//!
//! A host that exits while answering is seen to exit by its process, not by the end of its output
//! (a process it forked may hold the pipes): `the node host exited with status <n> while
//! answering describe`, or `… was killed by signal 11 (SIGSEGV) …` ([`exit_text`]).
//!
//! `PythonHost` is synchronous: it runs its [`process::HostProcess`] on the runtime given to
//! [`PythonHost::with_handle`], else on a current-thread runtime of its own built on first use,
//! and blocks on each request. It must not be called from a thread that runs asynchronous tasks
//! (a tokio worker, or inside `block_on`): when it is, it blocks on a helper thread instead of
//! panicking, but the thread it was called from stays blocked meanwhile.

pub mod connection;
pub mod locate;
pub mod pool;
pub mod process;

use connection::{Connection, ConnectionError, NoIncoming};
use grida_fx_core::ENGINE_VERSION;
use grida_fx_core::host::{HostFailure, NodeHost};
use grida_fx_protocol::{
    BuildParams, BuildResult, DescribeParams, DescribeResult, InitializeResult, PROTOCOL, method,
};
pub use process::end_every_host;
use process::{HostProcess, HostSpec};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

/// The interpreter's arguments: the host module, with the safe-path option (spec/protocol.md §1).
const HOST_ARGS: [&str; 3] = ["-P", "-m", "grida.fx.host"];

/// How long the engine waits for a host to answer `shutdown` or `$/cancel`, and then to exit.
pub(crate) const GRACE: Duration = Duration::from_secs(5);

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
    /// The planning project's root, when the caller knows it: its `.venv` is the interpreter's
    /// fallback after the open project's (spec/protocol.md §1).
    planning_root: Option<PathBuf>,
    process: Option<HostProcess>,
    /// What `initialize` answered.
    pub info: Option<InitializeResult>,
    /// Why the host of the open project cannot serve, once it failed to start or broke.
    failure: Option<String>,
    /// The engine's runtime, when the command has one.
    handle: Option<tokio::runtime::Handle>,
    /// A runtime of the host's own, built on first use when it has no handle. Declared last: it
    /// outlives the process.
    runtime: Option<tokio::runtime::Runtime>,
}

impl PythonHost {
    /// A host with no project yet (see `NodeHost::open_project`).
    pub fn new() -> PythonHost {
        PythonHost {
            project_root: None,
            sources: Vec::new(),
            python: None,
            planning_root: None,
            process: None,
            info: None,
            failure: None,
            handle: None,
            runtime: None,
        }
    }

    /// Uses this interpreter instead of locating one.
    pub fn with_python(mut self, python: PathBuf) -> PythonHost {
        self.python = Some(python);
        self
    }

    /// Falls back to this planning project's `.venv` after the open project's when it locates
    /// the interpreter (spec/protocol.md §1).
    pub fn with_planning_root(mut self, root: PathBuf) -> PythonHost {
        self.planning_root = Some(root);
        self
    }

    /// Runs the host on this runtime (the engine's) instead of one of its own. Give it before
    /// the first request; the runtime must outlive the host, which is ended when dropped.
    pub fn with_handle(mut self, handle: tokio::runtime::Handle) -> PythonHost {
        self.handle = Some(handle);
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
        Some(locate::python_interpreter(
            root,
            self.planning_root.as_deref(),
            &|name| std::env::var(name).ok(),
        ))
    }

    /// The interpreter as messages name it: relative to the project root when it lies inside the
    /// project (`.venv/bin/python`) or inside the planning project (`../../.venv/bin/python`),
    /// else as given (`python3`, a `GRIDA_FX_PYTHON` value).
    pub fn interpreter_label(&self) -> Option<String> {
        let python = self.interpreter()?;
        Some(match &self.project_root {
            Some(root) => label_from(&python, root, self.planning_root.as_deref()),
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

    /// Starts the process and initializes it, once; its connection.
    fn start(&mut self) -> Result<Connection, HostFailure> {
        if let Some(process) = &self.process {
            return Ok(process.connection().clone());
        }
        if let Some(failure) = &self.failure {
            return Err(HostFailure::Unavailable(failure.clone()));
        }
        let Some(root) = self.project_root.clone() else {
            return Err(HostFailure::Unavailable(
                "the node host has no project: open one before describe or build".into(),
            ));
        };
        let python = self
            .interpreter()
            .unwrap_or_else(|| PathBuf::from("python3"));
        let spec = HostSpec {
            label: label(&python, &root),
            python,
            project_root: root,
            sources: self.sources.clone(),
        };
        let started = self.block_on(HostProcess::start(&spec, Arc::new(NoIncoming)));
        match started {
            Ok(Ok(process)) => {
                let connection = process.connection().clone();
                self.info = Some(process.info().clone());
                self.process = Some(process);
                Ok(connection)
            }
            Ok(Err(message)) | Err(message) => {
                self.failure = Some(message.clone());
                Err(HostFailure::Unavailable(message))
            }
        }
    }

    /// Ends the host: `shutdown`, `exit`, wait (kill after 5 seconds). Idempotent.
    pub fn shutdown(&mut self) -> Result<(), HostFailure> {
        self.end_process();
        Ok(())
    }

    /// Sends one request of the engine's and reads its result, or sees the host exit.
    fn call<P, R>(&mut self, method: &str, params: &P) -> Result<R, HostFailure>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params).map_err(|e| {
            HostFailure::Unavailable(format!(
                "the {method} request cannot be written: {}",
                read_reason(&e)
            ))
        })?;
        self.start()?;
        let Some(mut process) = self.process.take() else {
            return Err(HostFailure::Unavailable(
                "the node host is not running".into(),
            ));
        };
        let asked = self.block_on(async move {
            let connection = process.connection().clone();
            let request = connection.request(method, Some(params));
            tokio::pin!(request);
            let answer = process.answer_or_exit(request, GRACE).await;
            (process, answer)
        });
        let answer = match asked {
            Ok((process, answer)) => {
                self.process = Some(process);
                answer
            }
            // The process was dropped with the request, which killed it.
            Err(message) => return Err(self.broke(message)),
        };
        match answer {
            Ok(value) => serde_json::from_value(value).map_err(|e| {
                self.broke(format!(
                    "the Python node host answered {method} with a result FX cannot read: {}",
                    read_reason(&e)
                ))
            }),
            Err(ConnectionError::Rpc(error)) => Err(HostFailure::Rpc(error)),
            Err(ConnectionError::Closed) => {
                let status = self.end_process();
                let message = format!(
                    "the node host {} while answering {method}",
                    exit_text(status)
                );
                self.failure = Some(message.clone());
                Err(HostFailure::Unavailable(message))
            }
            Err(ConnectionError::Protocol(message)) => Err(self.broke(format!(
                "the node host broke the protocol while answering {method}: {message}"
            ))),
        }
    }

    /// Ends a host that cannot go on and keeps the reason for later calls.
    fn broke(&mut self, message: String) -> HostFailure {
        self.end_process();
        self.failure = Some(message.clone());
        HostFailure::Unavailable(message)
    }

    /// Ends the process, politely when its connection still works; its exit status when known.
    fn end_process(&mut self) -> Option<ExitStatus> {
        let process = self.process.take()?;
        // Without a runtime to end it on, dropping the process kills it.
        self.block_on(process.shutdown()).ok().flatten()
    }

    /// Runs `future` to completion on the host's runtime (module doc). Errors are sentences.
    fn block_on<F>(&mut self, future: F) -> Result<F::Output, String>
    where
        F: Future + Send,
        F::Output: Send,
    {
        if self.handle.is_none() && self.runtime.is_none() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    format!(
                        "cannot start the runtime the node host needs: {}",
                        grida_fx_core::error::io_reason(&e)
                    )
                })?;
            self.runtime = Some(runtime);
        }
        let runtime = self.runtime.as_ref();
        let handle = self.handle.as_ref();
        // A runtime of its own is driven by its `block_on`; an engine handle by its workers.
        let run = move || match (runtime, handle) {
            (Some(runtime), _) => Some(runtime.block_on(future)),
            (None, Some(handle)) => Some(handle.block_on(future)),
            (None, None) => None,
        };
        let output = if tokio::runtime::Handle::try_current().is_err() {
            run()
        } else {
            // Blocking inside a runtime would panic: block on a helper thread instead.
            std::thread::scope(|scope| scope.spawn(run).join().ok().flatten())
        };
        output.ok_or_else(|| "the node host's runtime stopped".to_string())
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
        // Dropping a runtime blocks, which is refused inside another runtime.
        if let Some(runtime) = self.runtime.take()
            && tokio::runtime::Handle::try_current().is_ok()
        {
            runtime.shutdown_background();
        }
    }
}

/// The program to spawn: a relative path with folders is made absolute against the engine's
/// working directory, since the host starts in the project root; a bare name is left for the
/// `PATH` lookup. Symbolic links are kept (a virtual environment's `python` is one).
fn program(python: &Path) -> PathBuf {
    if python.is_relative()
        && python.components().count() > 1
        && let Ok(absolute) = std::path::absolute(python)
    {
        return absolute;
    }
    python.to_path_buf()
}

/// A path as messages name it: POSIX and relative to `root` inside it, else as given (the
/// interpreter's `HostSpec::label`).
pub fn label(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(inside) if !inside.as_os_str().is_empty() => inside
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => path.display().to_string(),
    }
}

/// [`label`], except that a path under `planning` (the planning project, when the home is a
/// project nested in it) and outside `root` is named relative to `root` with `..` segments
/// (`../../.venv/bin/python`), never by its absolute path.
pub fn label_from(path: &Path, root: &Path, planning: Option<&Path>) -> String {
    match planning {
        Some(planning)
            if path.strip_prefix(root).is_err()
                && path.strip_prefix(planning).is_ok()
                && root.strip_prefix(planning).is_ok() =>
        {
            crate::engine::relative_label(path, root)
        }
        _ => label(path, root),
    }
}

/// The sentence for a host that speaks another protocol (spec/protocol.md §2 "Version mismatch").
pub(crate) fn mismatch(sdk_version: Option<&str>, host_protocol: &str) -> String {
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

/// How a host ended, for a sentence that starts `the node host`: `exited with status 3`, `was
/// killed by signal 11 (SIGSEGV)`, or `exited` when the status is not known.
pub fn exit_text(status: Option<ExitStatus>) -> String {
    let Some(status) = status else {
        return "exited".into();
    };
    if let Some(code) = status.code() {
        return format!("exited with status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return match signal_name(signal) {
                Some(name) => format!("was killed by signal {signal} ({name})"),
                None => format!("was killed by signal {signal}"),
            };
        }
    }
    "exited".into()
}

/// The name of a signal by its number on this platform, for the signals a process commonly dies
/// of.
#[cfg(unix)]
fn signal_name(signal: i32) -> Option<&'static str> {
    let name = match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        _ => return platform_signal_name(signal),
    };
    Some(name)
}

/// The signals whose numbers differ between Linux and the BSDs.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn platform_signal_name(signal: i32) -> Option<&'static str> {
    match signal {
        7 => Some("SIGBUS"),
        10 => Some("SIGUSR1"),
        12 => Some("SIGUSR2"),
        31 => Some("SIGSYS"),
        _ => None,
    }
}

/// The signals whose numbers differ between Linux and the BSDs.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn platform_signal_name(signal: i32) -> Option<&'static str> {
    match signal {
        10 => Some("SIGBUS"),
        12 => Some("SIGSYS"),
        30 => Some("SIGUSR1"),
        31 => Some("SIGUSR2"),
        _ => None,
    }
}

/// Why a value could not be read, as a sentence for a person: what the reader found where it
/// expected what, without the reader's type names, quoting or positions (`it holds a list where
/// an object belongs`, `it has no outputs`, `it has none of the shapes FX reads there`).
pub fn read_reason(error: &serde_json::Error) -> String {
    let text = error.to_string();
    let text = match text.rfind(" at line ") {
        Some(at) if text[at..].contains(" column ") => &text[..at],
        _ => text.as_str(),
    };
    let found_where = |rest: &str| {
        rest.split_once(", expected ").map(|(found, expected)| {
            format!(
                "it holds {} where {} belongs",
                found_text(found),
                expected_text(expected)
            )
        })
    };
    if let Some(sentence) = text.strip_prefix("invalid type: ").and_then(found_where) {
        return sentence;
    }
    if let Some(sentence) = text.strip_prefix("invalid value: ").and_then(found_where) {
        return sentence;
    }
    if let Some(name) = text.strip_prefix("missing field ") {
        return format!("it has no {}", unquote(name));
    }
    if let Some(rest) = text.strip_prefix("unknown field ") {
        let name = rest.split(", ").next().unwrap_or(rest);
        return format!("{} is not one of its fields", unquote(name));
    }
    if let Some(rest) = text.strip_prefix("unknown variant ") {
        let name = rest.split(", expected").next().unwrap_or(rest);
        return format!("{} is not one of the values it takes", unquote(name));
    }
    if let Some(name) = text.strip_prefix("duplicate field ") {
        return format!("it names {} twice", unquote(name));
    }
    if text.starts_with("invalid length ") {
        return "it holds the wrong number of items".into();
    }
    if text.starts_with("data did not match any variant of untagged enum") {
        return "it has none of the shapes FX reads there".into();
    }
    text.replace('`', "")
}

/// What a reader found, in the words of JSON (see [`read_reason`]).
fn found_text(found: &str) -> String {
    let number = |rest: &str| format!("the number {}", unquote(rest));
    match found {
        "sequence" => "a list".into(),
        "map" => "an object".into(),
        "null" | "unit value" | "unit" | "Option value" => "null".into(),
        _ => {
            if let Some(rest) = found.strip_prefix("string ") {
                format!("the text {rest}")
            } else if let Some(rest) = found.strip_prefix("integer ") {
                number(rest)
            } else if let Some(rest) = found.strip_prefix("floating point ") {
                number(rest)
            } else if let Some(rest) = found.strip_prefix("boolean ") {
                unquote(rest)
            } else {
                unquote(found)
            }
        }
    }
}

/// What a reader expected, in the words of JSON (see [`read_reason`]).
fn expected_text(expected: &str) -> String {
    match expected {
        "a map" | "a JSON object" => "an object".into(),
        "a sequence" => "a list".into(),
        "a string" | "a borrowed string" | "a character" | "a char" => "text".into(),
        "a boolean" => "true or false".into(),
        "unit" => "null".into(),
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" => "a whole number".into(),
        "f32" | "f64" => "a number".into(),
        _ if expected.starts_with("struct ") => "an object".into(),
        _ if expected.starts_with("a tuple") => "a list".into(),
        _ if expected.starts_with("enum ") => "one of the values it takes".into(),
        _ => unquote(expected),
    }
}

/// A name without serde's backticks.
fn unquote(text: &str) -> String {
    text.replace('`', "")
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

    #[cfg(unix)]
    #[test]
    fn exits_and_signals_are_sentences() {
        use std::os::unix::process::ExitStatusExt;
        // A wait status: the exit code in the second byte, else the signal in the low bits.
        let exited = |code: i32| Some(ExitStatus::from_raw(code << 8));
        let killed = |signal: i32| Some(ExitStatus::from_raw(signal));
        assert_eq!(exit_text(exited(3)), "exited with status 3");
        assert_eq!(exit_text(exited(0)), "exited with status 0");
        assert_eq!(exit_text(killed(11)), "was killed by signal 11 (SIGSEGV)");
        assert_eq!(exit_text(killed(9)), "was killed by signal 9 (SIGKILL)");
        assert_eq!(exit_text(killed(6)), "was killed by signal 6 (SIGABRT)");
        assert_eq!(exit_text(killed(15)), "was killed by signal 15 (SIGTERM)");
        assert!(exit_text(killed(10)).starts_with("was killed by signal 10 (SIG"));
        assert_eq!(exit_text(killed(64)), "was killed by signal 64");
        assert_eq!(exit_text(None), "exited");
    }

    #[test]
    fn reasons_read_as_sentences() {
        use serde::Deserialize;
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Shape {
            modules: Vec<String>,
            count: u32,
            kind: Kind,
        }
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "lowercase")]
        #[allow(dead_code)]
        enum Kind {
            One,
        }
        #[derive(Debug, Deserialize)]
        #[serde(untagged)]
        #[allow(dead_code)]
        enum Either {
            Text(String),
            Number(u32),
        }
        let reason = |value: serde_json::Value| {
            read_reason(&serde_json::from_value::<Shape>(value).unwrap_err())
        };
        assert_eq!(
            reason(serde_json::json!(5)),
            "it holds the number 5 where an object belongs"
        );
        assert_eq!(
            reason(serde_json::json!([])),
            "it holds the wrong number of items"
        );
        assert_eq!(
            reason(serde_json::json!({"modules": "none", "count": 1, "kind": "one"})),
            "it holds the text \"none\" where a list belongs"
        );
        assert_eq!(
            reason(serde_json::json!({"modules": [], "count": -1, "kind": "one"})),
            "it holds the number -1 where a whole number belongs"
        );
        assert_eq!(
            reason(serde_json::json!({"modules": [], "kind": "one"})),
            "it has no count"
        );
        assert_eq!(
            reason(serde_json::json!({"modules": [], "count": 1, "kind": "one", "x": 1})),
            "x is not one of its fields"
        );
        assert_eq!(
            reason(serde_json::json!({"modules": [], "count": 1, "kind": "two"})),
            "two is not one of the values it takes"
        );
        assert_eq!(
            read_reason(&serde_json::from_value::<Either>(serde_json::json!([1])).unwrap_err()),
            "it has none of the shapes FX reads there"
        );
        assert_eq!(
            read_reason(&serde_json::from_str::<Shape>("{\"modules\": true}").unwrap_err()),
            "it holds true where a list belongs"
        );
        for text in ["[]", "{\"modules\": null}", "{\"modules\": [1.5]}"] {
            let reason = read_reason(&serde_json::from_str::<Shape>(text).unwrap_err());
            assert!(
                !reason.contains('`')
                    && !reason.contains(" at line ")
                    && !reason.contains("struct"),
                "{reason}"
            );
        }
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
        // A home nested in the planning project names the planning project's interpreter with
        // `..` segments; anything else is labelled as before.
        let home = Path::new("/work/acme/library/thing");
        let planning = Some(root);
        assert_eq!(
            label_from(Path::new("/work/acme/.venv/bin/python"), home, planning),
            "../../.venv/bin/python"
        );
        assert_eq!(
            label_from(
                Path::new("/work/acme/library/thing/.venv/bin/python"),
                home,
                planning
            ),
            ".venv/bin/python"
        );
        assert_eq!(
            label_from(Path::new("/opt/py/bin/python"), home, planning),
            "/opt/py/bin/python"
        );
        assert_eq!(label_from(Path::new("python3"), home, None), "python3");
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
        assert!(host.process.is_none());
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
