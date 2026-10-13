//! Node hosts (spec/protocol.md §1, §2): host processes, their connections, the pools that run
//! bodies, and the planning hosts.
//!
//! - [`connection`]: the asynchronous JSON-RPC connection, with requests in both directions;
//! - [`process`]: one started, initialized host process ([`process::HostProcess`]);
//! - [`pool`]: the hosts that run bodies, one job each, and one pool per language
//!   ([`pool::HostPools`]);
//! - [`locate`]: which program hosts a project's nodes: the Python interpreter, or Node and the
//!   TypeScript host program.
//!
//! A project module is hosted by the host of its language, chosen by its suffix
//! (`grida_fx_core::host::Language`): `.py` by the Python host, `.ts`, `.mts`, `.js` and `.mjs`
//! by the TypeScript host. [`NodeHosts`] implements `grida_fx_core::host::NodeHost` for planning
//! (`describe`, `build`) with one [`LanguageHost`] per language: `describe` sends each host the
//! targets of its language and merges the answers back in order, `build` goes to the host of the
//! builder's language, and built-ins are described by the Python host. Each [`LanguageHost`]
//! starts its host lazily, on the first `describe` or `build` it gets, so planning a project
//! without node modules of a language never needs that language; a language whose host failed
//! leaves the other's working.
//!
//! The Python host is `<python> -P -m grida.fx.host` with its working directory at the project
//! root, stdin/stdout piped for the protocol and stderr inherited (free-form log text). `-P`
//! (Python 3.11 and later) keeps the working directory off `sys.path`, so a project module named
//! like a standard library module or like `grida` cannot replace the host's own imports before
//! `initialize` puts the root on `sys.path`. The engine also sets `PYTHONSAFEPATH` to
//! [`SAFE_PATH_MARK`] when its own environment leaves it unset, for an interpreter wrapper that
//! drops `-P`; the host removes that value before it loads user code, so programs user code
//! starts see the user's environment. The TypeScript host is `<node> <host-entry.js>`, the host
//! program of a `@grida/fx` ([`locate::node_host_entry`]), started the same way except for its
//! stderr: a pipe of its own that the engine copies to its stderr ([`process`]).
//!
//! The engine sends `initialize`
//! (`{protocol, engine: {name: "grida-fx", version}, project_root, sources}`, the sources the
//! host's language can name), checks the answer's protocol (a mismatch, or a `protocol_mismatch`
//! error, is `the project's grida <sdk_version> speaks <host protocol> and grida-fx <version>
//! speaks fx-node-protocol-v1: upgrade grida` or `…: upgrade grida-fx`; `@grida/fx` for the
//! TypeScript host), then sends one request at a time. A host that cannot start is
//! `HostFailure::Unavailable` with a sentence naming its program and the variable that chooses
//! it ([`advice`]: `GRIDA_FX_PYTHON`, `GRIDA_FX_NODE`). A program the host located is checked
//! before it is first started ([`locate::probe`]): one that cannot host nodes is refused with
//! [`locate::refusal`]'s sentence and never started. A TypeScript host with no host program is
//! unavailable with [`locate::node_host_entry`]'s sentence; one whose host program's `@grida/fx`
//! lies outside the project ([`locate::host_package`]) has that folder replaced by `@grida/fx` in
//! the texts of its answers (a failed module's error, a refusal, a failure), since Node's
//! messages may name its files. Dropping the host sends `shutdown`
//! then the `exit` notification and waits up to 5 seconds before killing it.
//!
//! The program is the one given to [`LanguageHost::with_program`], else the one [`locate`]
//! finds over the process environment ([`locate::python_interpreter`], with the planning project
//! given to [`LanguageHost::with_planning_root`] as its fallback; [`locate::node_runtime`]). It is
//! named in messages as the user would write it ([`label_from`]): a path inside the project
//! relative to the project root, one inside the planning project with `..` segments. A host that
//! failed to start, or broke the protocol, stays failed for its project: later calls give the
//! same sentence without starting it again, until `open_project` names another project. A host
//! request during `describe` or `build` is answered `-32601`.
//!
//! A host that exits while answering is seen to exit by its process, not by the end of its output
//! (a process it forked may hold the pipes): `the node host exited with status <n> while
//! answering describe`, or `… was killed by signal 11 (SIGSEGV) …` ([`exit_text`]).
//!
//! `LanguageHost` is synchronous: it runs its [`process::HostProcess`] on the runtime given to
//! [`LanguageHost::with_handle`], else on a current-thread runtime of its own built on first use,
//! and blocks on each request. It must not be called from a thread that runs asynchronous tasks
//! (a tokio worker, or inside `block_on`): when it is, it blocks on a helper thread instead of
//! panicking, but the thread it was called from stays blocked meanwhile.

pub mod connection;
pub mod locate;
pub mod pool;
pub mod process;

use connection::{Connection, ConnectionError, NoIncoming};
use grida_fx_core::ENGINE_VERSION;
use grida_fx_core::host::{HostFailure, Language, NodeHost};
use grida_fx_protocol::{
    BuildParams, BuildResult, DescribeParams, DescribeResult, DescribeTarget, ErrorCode,
    InitializeResult, ModuleDescription, PROTOCOL, RpcError, method,
};
use process::{HostProcess, HostSpec};
pub use process::{end_every_host, end_every_host_now};
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

/// What to tell a person whose program cannot host nodes of `language`.
pub(crate) fn advice(language: Language) -> &'static str {
    match language {
        Language::Python => "set GRIDA_FX_PYTHON to a Python that has the grida package",
        Language::TypeScript => "set GRIDA_FX_NODE to Node 24 or later",
    }
}

/// The node host of one language for one project, started on first use (module doc).
pub struct LanguageHost {
    language: Language,
    /// Set by `open_project`; a call before it is `Unavailable`.
    project_root: Option<PathBuf>,
    sources: Vec<String>,
    /// The program (interpreter or Node), when the caller chose it; else [`locate`]'s.
    program: Option<PathBuf>,
    /// The TypeScript host program, when the caller chose it; else
    /// [`locate::node_host_entry`]'s.
    entry: Option<PathBuf>,
    /// The planning project's root, when the caller knows it: its `.venv` is the interpreter's
    /// fallback after the open project's, and its `@grida/fx` the TypeScript host program's
    /// (spec/protocol.md §1); programs inside it are named from the open project with `..`
    /// segments.
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

impl LanguageHost {
    /// A host of `language` with no project yet (see `NodeHost::open_project`).
    pub fn new(language: Language) -> LanguageHost {
        LanguageHost {
            language,
            project_root: None,
            sources: Vec::new(),
            program: None,
            entry: None,
            planning_root: None,
            process: None,
            info: None,
            failure: None,
            handle: None,
            runtime: None,
        }
    }

    /// The Python host.
    pub fn python() -> LanguageHost {
        LanguageHost::new(Language::Python)
    }

    /// The TypeScript host.
    pub fn typescript() -> LanguageHost {
        LanguageHost::new(Language::TypeScript)
    }

    /// The language of the modules this host runs.
    pub fn language(&self) -> Language {
        self.language
    }

    /// Uses this program (interpreter or Node) instead of locating one; it is not checked before
    /// it starts.
    pub fn with_program(mut self, program: PathBuf) -> LanguageHost {
        self.program = Some(program);
        self
    }

    /// Uses this TypeScript host program instead of locating one.
    pub fn with_entry(mut self, entry: PathBuf) -> LanguageHost {
        self.entry = Some(entry);
        self
    }

    /// Falls back to this planning project's `.venv` (Python) or `@grida/fx` (TypeScript) after
    /// the open project's when it locates the program (spec/protocol.md §1).
    pub fn with_planning_root(mut self, root: PathBuf) -> LanguageHost {
        self.planning_root = Some(root);
        self
    }

    /// Runs the host on this runtime (the engine's) instead of one of its own. Give it before
    /// the first request; the runtime must outlive the host, which is ended when dropped.
    pub fn with_handle(mut self, handle: tokio::runtime::Handle) -> LanguageHost {
        self.handle = Some(handle);
        self
    }

    /// The program the host runs, or would run, for the open project: the one given to
    /// [`LanguageHost::with_program`], else the one [`locate`] finds (the interpreter for the
    /// project, or Node). `None` when neither a program nor a project is set.
    pub fn program(&self) -> Option<PathBuf> {
        if let Some(program) = &self.program {
            return Some(program.clone());
        }
        let root = self.project_root.as_ref()?;
        let env = |name: &str| std::env::var(name).ok();
        Some(match self.language {
            Language::Python => {
                locate::python_interpreter(root, self.planning_root.as_deref(), &env)
            }
            Language::TypeScript => locate::node_runtime(&env),
        })
    }

    /// The program as messages name it: relative to the project root when it lies inside the
    /// project (`.venv/bin/python`) or inside the planning project (`../../.venv/bin/python`),
    /// else as given (`python3`, `node`, a `GRIDA_FX_PYTHON` or `GRIDA_FX_NODE` value).
    pub fn program_label(&self) -> Option<String> {
        let program = self.program()?;
        Some(match &self.project_root {
            Some(root) => label_from(&program, root, self.planning_root.as_deref()),
            None => program.display().to_string(),
        })
    }

    /// The TypeScript host program for the open project: the one given to
    /// [`LanguageHost::with_entry`], else [`locate::node_host_entry`]'s, from the open project
    /// and then the planning project (`Err`: its sentence). `None` for a Python host, or with no
    /// project.
    pub fn host_program(&self) -> Option<Result<PathBuf, String>> {
        if self.language != Language::TypeScript {
            return None;
        }
        if let Some(entry) = &self.entry {
            return Some(Ok(entry.clone()));
        }
        let root = self.project_root.as_ref()?;
        Some(locate::node_host_entry(
            root,
            self.planning_root.as_deref(),
            &|name: &str| std::env::var(name).ok(),
        ))
    }

    /// The folder of the TypeScript host program's `@grida/fx` when it lies outside the open
    /// project ([`locate::host_package`]), labelled `@grida/fx` for
    /// [`crate::engine::scrub_paths`]: Node's messages may name its files, and the host cuts only
    /// the project's own paths. Empty for a Python host, or a host program inside the project.
    fn package_roots(&self) -> Vec<(String, String)> {
        let (Some(Ok(entry)), Some(root)) = (self.host_program(), &self.project_root) else {
            return Vec::new();
        };
        locate::host_package(&entry, root)
            .map(|package| crate::engine::root_labels(&package, Language::TypeScript.sdk()))
            .unwrap_or_default()
    }

    /// What the program of the open project says about itself ([`locate::probe`], asked once
    /// per process), for `doctor`'s host lines. `None` when neither a program nor a project is
    /// set.
    pub fn probe(&mut self) -> Option<locate::Probe> {
        let program = self.program()?;
        let language = self.language;
        self.block_on(async move { locate::probe(language, &program).await })
            .ok()
    }

    /// Starts the host when it is not running and returns what `initialize` answered (for
    /// `doctor`'s host lines).
    pub fn host_info(&mut self) -> Result<InitializeResult, HostFailure> {
        self.start()?;
        self.info.clone().ok_or_else(|| {
            HostFailure::Unavailable("the node host has not answered initialize".into())
        })
    }

    /// How the host of the open project starts; `Err` is why it cannot.
    fn spec(&self) -> Result<HostSpec, String> {
        let Some(root) = self.project_root.clone() else {
            return Err("the node host has no project: open one before describe or build".into());
        };
        let fallback = match self.language {
            Language::Python => "python3",
            Language::TypeScript => "node",
        };
        let program = self.program().unwrap_or_else(|| PathBuf::from(fallback));
        let label = label_from(&program, &root, self.planning_root.as_deref());
        let spec = match self.language {
            Language::Python => HostSpec::python(program, label, root, self.sources.clone()),
            Language::TypeScript => {
                let entry = self
                    .host_program()
                    .unwrap_or_else(|| Err(locate::NO_SDK.into()))?;
                HostSpec::typescript(program, entry, label, root, self.sources.clone())
            }
        };
        // A program the caller named is its own (a test's fake host); one FX located is checked.
        Ok(if self.program.is_some() {
            spec.unchecked()
        } else {
            spec
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
        // Without a project, `spec` says so (and opening one clears the failure).
        let started = match self.spec() {
            Ok(spec) => self.block_on(HostProcess::start(&spec, Arc::new(NoIncoming))),
            Err(message) => Err(message),
        };
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
                    "the {} node host answered {method} with a result FX cannot read: {}",
                    self.language.title(),
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

impl NodeHost for LanguageHost {
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

    /// The host's answer, with a `@grida/fx` folder outside the project cut from its texts
    /// ([`LanguageHost::package_roots`]).
    fn describe(&mut self, params: &DescribeParams) -> Result<DescribeResult, HostFailure> {
        let roots = self.package_roots();
        let mut described = self
            .call::<_, DescribeResult>(method::DESCRIBE, params)
            .map_err(|failure| scrub_failure(failure, &roots))?;
        for module in &mut described.modules {
            if let ModuleDescription::Failed { error, .. } = module {
                *error = crate::engine::scrub_paths(error, &roots);
            }
        }
        Ok(described)
    }

    /// The host's answer, with a `@grida/fx` folder outside the project cut from its texts
    /// ([`LanguageHost::package_roots`]).
    fn build(&mut self, params: &BuildParams) -> Result<BuildResult, HostFailure> {
        let roots = self.package_roots();
        self.call(method::BUILD, params)
            .map_err(|failure| scrub_failure(failure, &roots))
    }
}

/// `failure` with `roots` replaced in its text ([`crate::engine::scrub_paths`]).
fn scrub_failure(failure: HostFailure, roots: &[(String, String)]) -> HostFailure {
    if roots.is_empty() {
        return failure;
    }
    match failure {
        HostFailure::Unavailable(message) => {
            HostFailure::Unavailable(crate::engine::scrub_paths(&message, roots))
        }
        HostFailure::Rpc(mut error) => {
            error.message = crate::engine::scrub_paths(&error.message, roots);
            HostFailure::Rpc(error)
        }
    }
}

impl Drop for LanguageHost {
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

/// The planning hosts of a command: one [`LanguageHost`] per language, chosen by each module's
/// suffix (module doc).
pub struct NodeHosts {
    python: LanguageHost,
    typescript: LanguageHost,
}

impl NodeHosts {
    /// A Python and a TypeScript host, with no project yet.
    pub fn new() -> NodeHosts {
        NodeHosts::of(LanguageHost::python(), LanguageHost::typescript())
    }

    /// Hosts made by the caller (a test's, with programs of its own): a Python host and a
    /// TypeScript host.
    pub fn of(python: LanguageHost, typescript: LanguageHost) -> NodeHosts {
        debug_assert_eq!(python.language, Language::Python);
        debug_assert_eq!(typescript.language, Language::TypeScript);
        NodeHosts { python, typescript }
    }

    /// Both hosts with the planning project's root (see [`LanguageHost::with_planning_root`]).
    pub fn with_planning_root(self, root: PathBuf) -> NodeHosts {
        NodeHosts {
            python: self.python.with_planning_root(root.clone()),
            typescript: self.typescript.with_planning_root(root),
        }
    }

    /// The host of `language` (for `doctor`'s host lines).
    pub fn member(&mut self, language: Language) -> &mut LanguageHost {
        match language {
            Language::Python => &mut self.python,
            Language::TypeScript => &mut self.typescript,
        }
    }

    /// Ends both hosts. Idempotent.
    pub fn shutdown(&mut self) {
        let _ = self.python.shutdown();
        let _ = self.typescript.shutdown();
    }
}

impl Default for NodeHosts {
    fn default() -> Self {
        NodeHosts::new()
    }
}

impl NodeHost for NodeHosts {
    /// Points both hosts at the project; neither starts.
    fn open_project(&mut self, root: &Path, sources: &[String]) -> Result<(), HostFailure> {
        self.python.open_project(root, sources)?;
        self.typescript.open_project(root, sources)
    }

    /// Sends each host the targets of its language (built-ins to the Python host) and merges the
    /// modules back in the order of `params.targets`. A target of no language is a failed module
    /// (`<path>: no node host for …`); a host with no targets and no built-ins is not asked, so
    /// it never starts.
    fn describe(&mut self, params: &DescribeParams) -> Result<DescribeResult, HostFailure> {
        let mut modules: Vec<Option<ModuleDescription>> = vec![None; params.targets.len()];
        let mut asked: Vec<(Language, Vec<usize>)> = Language::ALL
            .iter()
            .map(|&language| (language, Vec::new()))
            .collect();
        for (at, target) in params.targets.iter().enumerate() {
            match Language::of_module(&target.path) {
                Ok(language) => {
                    if let Some((_, indices)) = asked.iter_mut().find(|(l, _)| *l == language) {
                        indices.push(at);
                    }
                }
                Err(refusal) => {
                    modules[at] = Some(ModuleDescription::Failed {
                        path: target.path.clone(),
                        attribute: target.attribute.clone(),
                        error: format!("{}: {refusal}", target.path),
                    });
                }
            }
        }
        let mut builtins = Vec::new();
        for (language, indices) in asked {
            let with_builtins = params.builtins && language == Language::Python;
            if indices.is_empty() && !with_builtins {
                continue;
            }
            let targets: Vec<DescribeTarget> = indices
                .iter()
                .map(|&at| params.targets[at].clone())
                .collect();
            let described = self.member(language).describe(&DescribeParams {
                targets,
                builtins: with_builtins,
            })?;
            if described.modules.len() != indices.len() {
                return Err(HostFailure::Unavailable(format!(
                    "the {} node host described {} of {} modules",
                    language.title(),
                    described.modules.len(),
                    indices.len()
                )));
            }
            for (at, module) in indices.into_iter().zip(described.modules) {
                modules[at] = Some(module);
            }
            if with_builtins {
                builtins = described.builtins;
            }
        }
        Ok(DescribeResult {
            modules: modules.into_iter().flatten().collect(),
            builtins,
        })
    }

    /// Asks the host of the builder file's language.
    fn build(&mut self, params: &BuildParams) -> Result<BuildResult, HostFailure> {
        match Language::of_module(&params.path) {
            Ok(language) => self.member(language).build(params),
            Err(refusal) => Err(HostFailure::Rpc(RpcError::new(
                ErrorCode::LoadFailed,
                format!("{}: {refusal}", params.path),
            ))),
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
    let inside = |path: &Path, root: &Path| match path.strip_prefix(root) {
        Ok(inside) if !inside.as_os_str().is_empty() => Some(
            inside
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        ),
        _ => None,
    };
    // A root reached through a symbolic link (macOS's /var is /private/var) is the same root:
    // compare again with the folders resolved, but not the file, which may itself be a link
    // (a virtual environment's python).
    let resolved = || {
        let folder = std::fs::canonicalize(path.parent()?).ok()?;
        inside(
            &folder.join(path.file_name()?),
            &std::fs::canonicalize(root).ok()?,
        )
    };
    inside(path, root)
        .or_else(resolved)
        .unwrap_or_else(|| path.display().to_string())
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

/// The sentence for a host of `language` that speaks another protocol (spec/protocol.md §2
/// "Version mismatch"): `the project's grida 0.1.0 speaks …: upgrade grida`, `@grida/fx` for
/// TypeScript.
pub(crate) fn mismatch(
    language: Language,
    sdk_version: Option<&str>,
    host_protocol: &str,
) -> String {
    let sdk = language.sdk();
    let named = match sdk_version {
        Some(version) if !version.is_empty() => format!("{sdk} {version}"),
        _ => sdk.to_string(),
    };
    format!(
        "the project's {named} speaks {host_protocol} and grida-fx {ENGINE_VERSION} speaks {PROTOCOL}: upgrade {}",
        upgrade(language, host_protocol)
    )
}

/// Which side to upgrade: grida-fx when the host speaks a newer version of this protocol, the
/// language's SDK otherwise.
fn upgrade(language: Language, host_protocol: &str) -> &'static str {
    let version = |protocol: &str| -> Option<u64> {
        protocol
            .strip_prefix("fx-node-protocol-v")?
            .parse::<u64>()
            .ok()
    };
    match (version(PROTOCOL), version(host_protocol)) {
        (Some(ours), Some(theirs)) if theirs > ours => "grida-fx",
        _ => language.sdk(),
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
        let mismatch = |sdk, protocol| super::mismatch(Language::Python, sdk, protocol);
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
        // The TypeScript host's SDK is @grida/fx.
        assert_eq!(
            super::mismatch(Language::TypeScript, Some("0.6.0"), "fx-node-protocol-v0"),
            format!(
                "the project's @grida/fx 0.6.0 speaks fx-node-protocol-v0 and grida-fx {ENGINE_VERSION} speaks fx-node-protocol-v1: upgrade @grida/fx"
            )
        );
        assert!(
            super::mismatch(Language::TypeScript, None, "fx-node-protocol-v2")
                .starts_with("the project's @grida/fx speaks fx-node-protocol-v2 and")
        );
        assert!(
            super::mismatch(Language::TypeScript, None, "fx-node-protocol-v2")
                .ends_with(": upgrade grida-fx")
        );
    }

    #[test]
    fn advice_names_the_variable_of_each_language() {
        assert_eq!(
            advice(Language::Python),
            "set GRIDA_FX_PYTHON to a Python that has the grida package"
        );
        assert_eq!(
            advice(Language::TypeScript),
            "set GRIDA_FX_NODE to Node 24 or later"
        );
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
        let mut host = LanguageHost::python();
        assert_eq!(host.program(), None);
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
        let mut host = LanguageHost::python().with_program(dir.path().join(".venv/bin/python"));
        host.open_project(dir.path(), &[]).unwrap();
        assert_eq!(host.program_label().as_deref(), Some(".venv/bin/python"));
        assert_eq!(host.host_program(), None);
        // The same project again keeps the host; nothing was started.
        host.open_project(dir.path(), &[]).unwrap();
        assert!(host.process.is_none());
    }

    #[test]
    fn a_missing_interpreter_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = LanguageHost::python().with_program(PathBuf::from("no-such-python-for-fx"));
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

    #[test]
    fn a_typescript_host_without_a_host_program_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        // Node is given, so nothing is probed; no @grida/fx is found from the project.
        let mut host = LanguageHost::typescript().with_program(PathBuf::from("node-for-fx"));
        host.open_project(dir.path(), &[]).unwrap();
        assert_eq!(host.program_label().as_deref(), Some("node-for-fx"));
        let params = DescribeParams {
            targets: Vec::new(),
            builtins: false,
        };
        let expected = HostFailure::Unavailable(locate::NO_SDK.into());
        if std::env::var_os(locate::SDK_NODE_HOST_VAR).is_none() {
            assert_eq!(host.host_program(), Some(Err(locate::NO_SDK.into())));
            assert_eq!(host.describe(&params).unwrap_err(), expected);
            assert!(host.info.is_none());
        }
        // A host program given by the caller is used as it is.
        let mut host = LanguageHost::typescript()
            .with_program(PathBuf::from("no-such-node-for-fx"))
            .with_entry(dir.path().join("host-entry.js"));
        host.open_project(dir.path(), &[]).unwrap();
        assert_eq!(
            host.describe(&params).unwrap_err(),
            HostFailure::Unavailable(
                "cannot start the TypeScript node host with no-such-node-for-fx: no such file; \
                 set GRIDA_FX_NODE to Node 24 or later"
                    .into()
            )
        );
    }

    #[test]
    fn the_planning_hosts_start_nothing_for_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosts = NodeHosts::of(
            LanguageHost::python().with_program(PathBuf::from("no-such-python-for-fx")),
            LanguageHost::typescript().with_program(PathBuf::from("no-such-node-for-fx")),
        );
        hosts.open_project(dir.path(), &[]).unwrap();
        let described = hosts
            .describe(&DescribeParams {
                targets: Vec::new(),
                builtins: false,
            })
            .unwrap();
        assert!(described.modules.is_empty() && described.builtins.is_empty());
        // A target of no language is a failed module, and asks no host either.
        let described = hosts
            .describe(&DescribeParams {
                targets: vec![DescribeTarget {
                    path: "nodes/a.cjs".into(),
                    attribute: Some("x".into()),
                }],
                builtins: false,
            })
            .unwrap();
        assert_eq!(
            described.modules,
            [ModuleDescription::Failed {
                path: "nodes/a.cjs".into(),
                attribute: Some("x".into()),
                error: "nodes/a.cjs: no node host for .cjs modules".into(),
            }]
        );
        assert!(hosts.member(Language::Python).info.is_none());
        assert!(hosts.member(Language::Python).failure.is_none());
        assert!(hosts.member(Language::TypeScript).failure.is_none());
        let refused = hosts
            .build(&BuildParams {
                path: "build.rb".into(),
                function: "build".into(),
                arguments: Default::default(),
                cwd: dir.path().display().to_string(),
            })
            .unwrap_err();
        assert!(
            matches!(&refused, HostFailure::Rpc(error)
                if error.kind() == Some(ErrorCode::LoadFailed)
                    && error.message == "build.rb: no node host for .rb modules yet"),
            "{refused:?}"
        );
    }

    #[test]
    fn one_language_failing_leaves_the_other_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut hosts = NodeHosts::of(
            LanguageHost::python().with_program(PathBuf::from("no-such-python-for-fx")),
            LanguageHost::typescript().with_program(PathBuf::from("no-such-node-for-fx")),
        );
        hosts.open_project(dir.path(), &[]).unwrap();
        let target = |path: &str| DescribeParams {
            targets: vec![DescribeTarget {
                path: path.into(),
                attribute: None,
            }],
            builtins: false,
        };
        let failure = hosts.describe(&target("nodes/a.py")).unwrap_err();
        assert!(
            matches!(&failure, HostFailure::Unavailable(m) if m.starts_with("cannot start the Python node host")),
            "{failure:?}"
        );
        assert!(hosts.member(Language::TypeScript).failure.is_none());
        let failure = hosts.describe(&target("nodes/a.ts")).unwrap_err();
        assert!(
            matches!(&failure, HostFailure::Unavailable(m) if m.contains("TypeScript")),
            "{failure:?}"
        );
        // Built-ins are the Python host's.
        let failure = hosts
            .describe(&DescribeParams {
                targets: Vec::new(),
                builtins: true,
            })
            .unwrap_err();
        assert!(
            matches!(&failure, HostFailure::Unavailable(m) if m.starts_with("cannot start the Python node host")),
            "{failure:?}"
        );
    }
}
