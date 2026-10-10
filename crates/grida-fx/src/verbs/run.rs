//! `grida-fx run <target> [inputs] [--routes f]… [--arg n=v]… [--live] [--max-usd N]
//! [--yes-up-to N] [--deliver OUTPUT=PATH]… [--name NAME | --resume NAME | --run FOLDER]
//! [--stand-in FILE.py#FUNCTION | -]`
//! (docs/guide/05-running.md; spec/store.md §8; spec/protocol.md §5.7).
//!
//! 0. A stand-in, before anything else (usage errors, exit 2): `--stand-in cannot be used with
//!    --live` (or `with --yes-up-to`); a source that is neither `<file>.py#<function>` (a Python
//!    identifier after the last `#`) nor `-` is `--stand-in takes <file>.py#<function>, or -`; a
//!    file that is not there is `no stand-in file <file>`; `-` with a standard input that is not
//!    a Unix-domain socket is `--stand-in - needs the stand-in's socket on standard input`
//!    ([`StandInSource::parse`]).
//! 1. Arguments, before anything is read: `--max-usd` and `--yes-up-to` through `Usd::parse` (a
//!    negative, non-finite or over-precise amount is a usage error, `<option> <text>: <reason>`,
//!    exit 2); `--deliver` pairs split at the first `=` (`--deliver <pair>: write OUTPUT=PATH`
//!    when either side is empty); workflow input flags as the planning verbs take them.
//! 2. The planner; then every `--deliver` name is checked against the workflow's declared outputs
//!    (`--deliver <pair>: name one of <sorted names, ", ">`, or `…: the workflow declares no
//!    outputs`, exit 2) before anything is planned or run. A stand-in's answerer starts next
//!    ([`start_stand_in`]), before planning: for a file, a stand-in host in the working
//!    directory (`initialize` with the planning project, then `stand_in.load` with the file's
//!    absolute path, which nothing records), failing with `the stand-in <file>#<function> could
//!    not be loaded: <reason>`; for `-`, `initialize` over the socket, failing with `the stand-in
//!    on standard input could not be started: <reason>` (both exit 2). Then the engine
//!    ([`crate::engine::engine_for`]), which with `--live` builds every provider's adapters from
//!    the keys in the process environment or the planning project's `.env` (spec/providers.md
//!    §3): a `.env` or base-URL refusal is a usage error naming the variable or the line (exit 2),
//!    and building prints nothing. A missing key refuses only the calls that need it, when they
//!    are made (`<VARIABLE> is not set`, $0). A stand-in run's engine has the stand-in store
//!    (`<cache>/stand-in`) as its store and asks the stand-in.
//! 3. Plan through the engine ([`crate::engine::plan`]: `at: plan` steps run, `cached` is the
//!    store's). A plan with problems prints the plan (`plan::render::render`; a stand-in run's
//!    with `plan::render::render_stand_in`, which never warns that the ceiling stops the run)
//!    and exits 1 without creating a folder.
//! 4. The folder: `--name` atomically claims a source-scoped name; `--resume` requires its
//!    recorded run to exist, with the current plan and mode checked by the runner. `--run` is
//!    relative to the working directory, named as typed, and retains create/resume behavior; else
//!    `folder::new_folder` under the planning project's runs folder, named relative to the
//!    working directory: it makes the folder, so invocations starting at once never share one
//!    (one that cannot be made is an error, exit 2). The refusals that need no folder
//!    (`runner::refused`) come first, so a refused run makes none, and a run refused after the
//!    folder was made, before it wrote anything there, removes it again. The plan text is
//!    printed, then `runner::run` with the takes file relative to the planning project
//!    (`plan.json` records it for `reroll` and `pick`).
//! 5. `RunError::Refused` prints `refused: <message>` on stdout, exit 1; `RunError::Fatal` is an
//!    error (exit 2). A cancelled run (Ctrl-C) exits 130 and prints nothing more.
//! 6. The summary, each label padded to 10 columns: `run       <folder as named>`, `stand-in
//!    <source as typed>` for a stand-in run, `result    ok
//!    | incomplete | failed   spent $<charged, 2 places>`, `failed    <id>: <error or "no reason
//!    recorded">` per failure in order, `stopped   <message>`. An absolute path of the project,
//!    the home or the store left in an error is shown relative to the project.
//! 7. `--deliver`, after the run, request by request. First every request is matched with the
//!    output's files (one file; else each file of a list, keyed collection or object, labelled
//!    by its key, else by its position from 0, as spec/store.md §8 "One file or several" labels
//!    `outputs/`): a one-file output given `{key}` (`--deliver <pair>: <name> is one file, so no
//!    {key}`) or several files without it (`--deliver <pair>: name each element with {key}`) is
//!    a usage error (exit 2) before any file is delivered. Then, in order: each file goes to
//!    `<pattern>` with every `{key}` replaced by the label as a path (`folder::keyed_path`:
//!    spec/store.md §8 "Keys as paths"), relative to the working directory; the store's copy is
//!    checked against its digest, copied under a temporary name beside the target and renamed
//!    over it, and printed `delivered <path as named>`; a target that already holds the same
//!    bytes is left alone and not printed. An output with no file prints `missing   <name>: the
//!    run did not produce it`.
//! 8. Exit 0 when the run is ok and every requested output was delivered, else 1. The stand-in's
//!    answerer is ended with the engine ([`crate::engine::shutdown`]).

use crate::cli::RunArgs;
use crate::print::{labelled, print_line, shown_path};
use grida_fx_core::project::{PlanRequest, Planner, make_planner};
use grida_fx_core::val::{FileValue, Val};
use grida_fx_core::{Error, ErrorKind};
use grida_fx_runtime::engine::{Engine, scrub_paths};
use grida_fx_runtime::folder::{keyed_path, new_folder};
use grida_fx_runtime::host::locate::python_interpreter;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::run_index::RunIndex;
use grida_fx_runtime::runner::{self, RunError, RunOptions, RunOutcome};
use grida_fx_runtime::stand_in::{ConnectionAnswerer, StandIn};
use grida_fx_runtime::store::Store;
use indexmap::IndexMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The exit status of a run a person interrupted.
pub const INTERRUPTED: u8 = 130;

/// What `{key}` stands for in a `--deliver` path.
const KEY: &str = "{key}";

/// Runs `grida-fx run`.
pub fn run(args: &RunArgs) -> Result<u8, Error> {
    for name in args.name.iter().chain(args.resume.iter()) {
        super::run_catalog::validate_name(name)?;
    }
    let cwd = super::planning::working_directory()?;
    let stand_in = args
        .stand_in
        .as_deref()
        .map(|text| StandInSource::parse(text, args, &cwd))
        .transpose()?;
    let max_usd = args
        .max_usd
        .as_deref()
        .map(|text| super::planning::amount("--max-usd", text))
        .transpose()?;
    let yes_up_to = args
        .yes_up_to
        .as_deref()
        .map(|text| super::planning::amount("--yes-up-to", text))
        .transpose()?;
    let deliveries = args
        .deliver
        .iter()
        .map(|pair| Delivery::parse(pair))
        .collect::<Result<Vec<_>, _>>()?;
    let request = PlanRequest {
        target: args.target.clone(),
        cwd: cwd.clone(),
        input_files: args.inputs.clone(),
        rest: args.rest.clone(),
        arguments: crate::args::parse_arguments(&args.arg)?,
        routes: args.routes.clone(),
        max_usd,
        builtin_routes: crate::engine::builtin_routes()?,
    };
    let mut host = crate::print::planning_host(&request.target, &request.cwd);
    let mut planner = make_planner(&request, &mut host)?;
    let declared: Vec<&str> = planner
        .workflow
        .workflow
        .outputs
        .keys()
        .map(String::as_str)
        .collect();
    for delivery in &deliveries {
        delivery.check_name(&declared)?;
    }
    let runtime = crate::engine::runtime()?;
    // Paths the engine keeps out of what it records besides its own roots: the working
    // directory, and a stand-in file's folder (its traceback names it).
    let mut private = vec![cwd.clone()];
    if let Some(StandInSource::File { path, .. }) = &stand_in
        && let Some(folder) = std::path::absolute(path)
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        private.push(folder);
    }
    let stand_in = stand_in
        .map(|source| start_stand_in(&runtime, source, &planner, &cwd))
        .transpose()?;
    let engine = match crate::engine::engine_for(
        &runtime,
        &planner,
        args.live,
        stand_in.clone(),
        &private,
    ) {
        Ok(engine) => engine,
        Err(error) => {
            if let Some(stand_in) = &stand_in {
                runtime.block_on(stand_in.shutdown());
            }
            return Err(error);
        }
    };
    let ran = plan_and_run(
        args,
        &cwd,
        &engine,
        &mut planner,
        &mut host,
        yes_up_to,
        &deliveries,
    );
    crate::engine::shutdown(&runtime, &engine);
    ran
}

/// Steps 3–8 of the module doc.
fn plan_and_run(
    args: &RunArgs,
    cwd: &Path,
    engine: &Arc<Engine>,
    planner: &mut Planner,
    host: &mut grida_fx_runtime::host::PythonHost,
    yes_up_to: Option<grida_fx_core::money::Usd>,
    deliveries: &[Delivery],
) -> Result<u8, Error> {
    let plan = crate::engine::plan(engine, planner, host)?;
    let text = if engine.stand_in_run() {
        grida_fx_core::plan::render::render_stand_in(&plan, planner)
    } else {
        grida_fx_core::plan::render::render(&plan, planner)
    };
    if !plan.ok() {
        print_line(&text);
        return Ok(1);
    }
    if let Some(message) = runner::refused(&plan, engine.live) {
        // Before any folder is made.
        print_line(&text);
        print_line(&format!("refused: {message}"));
        return Ok(1);
    }
    let (folder, label, made) = if let Some(name) = &args.name {
        let folder = super::run_catalog::create_named(
            &planner.project.runs_dir(),
            &planner.workflow.workflow.id,
            &planner.workflow.source,
            name,
        )?;
        let label = shown_path(&folder, cwd);
        (folder, label, true)
    } else if let Some(name) = &args.resume {
        let folder = super::run_catalog::resume_named(
            &planner.project.runs_dir(),
            &planner.workflow.workflow.id,
            &planner.workflow.source,
            name,
        )?;
        let label = shown_path(&folder, cwd);
        (folder, label, false)
    } else {
        match &args.run {
            Some(typed) => (cwd.join(typed), typed.clone(), false),
            None => {
                let runs = planner.project.runs_dir();
                let folder = new_folder(&runs, &planner.workflow.workflow.id).map_err(|error| {
                    Error::io(
                        &shown_path(&runs.join(&planner.workflow.workflow.id), cwd),
                        &error,
                    )
                })?;
                let label = shown_path(&folder, cwd);
                (folder, label, true)
            }
        }
    };
    print_line(&text);
    let options = RunOptions {
        folder: folder.clone(),
        label: label.clone(),
        name: args.name.clone(),
        yes_up_to,
        takes_file: super::takes_file(planner),
        existing: args.resume.is_some(),
        index: Some(RunIndex {
            project_root: planner.project.root.clone(),
        }),
    };
    let mut viewer = None;
    let project = planner.project.clone();
    let ran = runner::run_with_control_ready(
        Arc::clone(engine),
        planner,
        host,
        plan,
        options,
        |ready| {
            crate::interrupt::runner_started();
            print_line(&format!("run       {label}"));
            eprintln!("invocation {}", ready.invocation_id);
            eprintln!(
                "{}",
                if ready.control_available {
                    "control   available"
                } else {
                    "control   unavailable"
                }
            );
            let root = &ready.folder;
            // Observation is independent: a bind failure must not fail or hold up execution.
            if !args.no_view && (args.standalone || !project.has_file) {
                match RunViewer::start(engine, root, args.open) {
                    Ok(host) => viewer = Some(host),
                    Err(_) => crate::print::print_error(&Error::usage(
                        "standalone viewer unavailable; the workflow will continue. Reopen it with inspect RUN --standalone after completion.",
                    )),
                }
            } else if !args.no_view {
                super::view::report_service(
                    super::service::register_run(&project, root, args.open),
                    args.open,
                );
            }
        },
    );
    // The run command's lifetime owns the server, including failure and cancellation.
    drop(viewer);
    if made && ran.is_err() {
        // Refused before anything was written: the new folder goes again (only while empty).
        let _ = std::fs::remove_dir(&folder);
    }
    let outcome = match ran {
        Ok(outcome) => outcome,
        Err(RunError::Refused(message)) => {
            print_line(&format!("refused: {message}"));
            return Ok(1);
        }
        Err(RunError::Fatal(error)) => return Err(error),
    };
    if outcome.cancelled {
        return Ok(INTERRUPTED);
    }
    let scrub = Scrub::new(planner, &engine.store);
    // The run folder was already flushed at readiness. Keep one stable summary header;
    // dynamic invocation/control diagnostics belong to stderr.
    for line in summary(&label, args.stand_in.as_deref(), &outcome, &scrub)
        .into_iter()
        .skip(1)
    {
        print_line(&line);
    }
    let delivered = deliver(deliveries, &outcome.outputs, &engine.store, cwd)?;
    Ok(u8::from(!(outcome.ok && delivered)))
}

/// The viewer shares the engine's executor but never awaits its scheduling or observers.
struct RunViewer {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl RunViewer {
    fn start(engine: &Engine, root: &Path, open: bool) -> std::io::Result<Self> {
        let server = engine.handle.block_on(grida_fx_viewer::bind(root, 0))?;
        let url = server.url().to_string();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = engine.handle.spawn(async move {
            if server
                .serve_until(async move {
                    let _ = stopped.await;
                })
                .await
                .is_err()
            {
                crate::print::print_error(&Error::usage(
                    "viewer stopped; the workflow will continue.",
                ));
            }
        });
        print_line(&labelled("view", &url));
        if open {
            super::view::launch_browser(url);
        }
        Ok(Self {
            stop: Some(stop),
            task,
        })
    }
}

impl Drop for RunViewer {
    fn drop(&mut self) {
        // Aborting also bounds shutdown when a browser keeps an artifact response open.
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}

/// Where a stand-in run's stand-in is (spec/protocol.md §5.7), checked (module doc, step 0).
#[derive(Debug)]
enum StandInSource {
    /// `<file>.py#<function>`: `file` as typed, relative to the working directory, and its
    /// absolute path.
    File {
        file: String,
        path: PathBuf,
        function: String,
    },
    /// `-`: the answerer's socket, from standard input.
    Socket(SocketOnStdin),
}

impl StandInSource {
    /// Checks `--stand-in <text>` against the other options and the working directory `cwd`
    /// (module doc, step 0).
    fn parse(text: &str, args: &RunArgs, cwd: &Path) -> Result<StandInSource, Error> {
        if args.live {
            return Err(Error::usage("--stand-in cannot be used with --live"));
        }
        if args.yes_up_to.is_some() {
            return Err(Error::usage("--stand-in cannot be used with --yes-up-to"));
        }
        if text == "-" {
            return socket_on_stdin().map(StandInSource::Socket).map_err(|()| {
                Error::usage("--stand-in - needs the stand-in's socket on standard input")
            });
        }
        let (file, function) = split_stand_in(text)
            .ok_or_else(|| Error::usage("--stand-in takes <file>.py#<function>, or -"))?;
        let path = cwd.join(file);
        if !path.is_file() {
            return Err(Error::usage(format!("no stand-in file {file}")));
        }
        Ok(StandInSource::File {
            file: file.to_string(),
            path,
            function: function.to_string(),
        })
    }
}

/// `<file>.py#<function>` split at its last `#`: a file named `….py` and a Python identifier.
fn split_stand_in(text: &str) -> Option<(&str, &str)> {
    let (file, function) = text.rsplit_once('#')?;
    let named = file.len() > ".py".len()
        && file.ends_with(".py")
        && !file.ends_with("/.py")
        && !file.ends_with("\\.py");
    let mut chars = function.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    (named && identifier).then_some((file, function))
}

/// The answerer's end of a socket the command was given as its standard input.
#[cfg(unix)]
type SocketOnStdin = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
type SocketOnStdin = ();

/// Standard input as a Unix-domain stream socket (a copy of the descriptor, closed on exec, so
/// nothing the engine starts inherits it); `Err` when it is anything else.
#[cfg(unix)]
fn socket_on_stdin() -> Result<SocketOnStdin, ()> {
    use std::os::fd::AsFd;
    use std::os::unix::fs::FileTypeExt;
    let descriptor = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(drop)?;
    let file = std::fs::File::from(descriptor);
    let socket = file
        .metadata()
        .is_ok_and(|metadata| metadata.file_type().is_socket());
    if !socket {
        return Err(());
    }
    let stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(file));
    // A socket of another domain has no Unix-domain address.
    stream.local_addr().map_err(drop)?;
    Ok(stream)
}

/// Stand-ins on standard input need a Unix-domain socket.
#[cfg(not(unix))]
fn socket_on_stdin() -> Result<SocketOnStdin, ()> {
    Err(())
}

/// Starts the stand-in's answerer for the planning project (module doc, step 2).
fn start_stand_in(
    runtime: &tokio::runtime::Runtime,
    source: StandInSource,
    planner: &Planner,
    cwd: &Path,
) -> Result<Arc<StandIn>, Error> {
    let project = &planner.project;
    let answerer = match source {
        StandInSource::File {
            file,
            path,
            function,
        } => {
            let env = |name: &str| std::env::var(name).ok();
            let python = python_interpreter(&project.root, None, &env);
            let spec = HostSpec {
                label: grida_fx_runtime::host::label(&python, &project.root),
                python,
                project_root: project.root.clone(),
                sources: project.document.sources.clone(),
            };
            let path = std::path::absolute(&path).unwrap_or(path);
            runtime
                .block_on(ConnectionAnswerer::start_host(&spec, cwd, &path, &function))
                .map_err(|reason| {
                    Error::new(
                        ErrorKind::Host,
                        format!("the stand-in {file}#{function} could not be loaded: {reason}"),
                    )
                })?
        }
        StandInSource::Socket(socket) => runtime
            .block_on(answerer_on_socket(socket, project))
            .map_err(|reason| {
                Error::new(
                    ErrorKind::Host,
                    format!("the stand-in on standard input could not be started: {reason}"),
                )
            })?,
    };
    Ok(Arc::new(StandIn::new(Arc::new(answerer))))
}

/// The answerer on the socket of `--stand-in -`.
#[cfg(unix)]
async fn answerer_on_socket(
    socket: SocketOnStdin,
    project: &grida_fx_core::docs::project::Project,
) -> Result<ConnectionAnswerer, String> {
    socket
        .set_nonblocking(true)
        .map_err(|error| grida_fx_core::error::io_reason(&error))?;
    let socket = tokio::net::UnixStream::from_std(socket)
        .map_err(|error| grida_fx_core::error::io_reason(&error))?;
    let (reader, writer) = socket.into_split();
    ConnectionAnswerer::start_socket(reader, writer, &project.root, &project.document.sources).await
}

#[cfg(not(unix))]
async fn answerer_on_socket(
    _socket: SocketOnStdin,
    _project: &grida_fx_core::docs::project::Project,
) -> Result<ConnectionAnswerer, String> {
    Err("stand-ins on standard input need a Unix-domain socket".into())
}

/// The summary lines (module doc, step 6). `stand_in`: the stand-in's source as typed.
fn summary(
    label: &str,
    stand_in: Option<&str>,
    outcome: &RunOutcome,
    scrub: &Scrub,
) -> Vec<String> {
    let result = if outcome.ok {
        "ok"
    } else if outcome.incomplete {
        "incomplete"
    } else {
        "failed"
    };
    let mut lines = vec![labelled("run", label)];
    if let Some(source) = stand_in {
        lines.push(labelled("stand-in", source));
    }
    lines.push(labelled(
        "result",
        &format!("{result}   spent {}", outcome.charged.dollars_2()),
    ));
    for (id, error) in &outcome.failed {
        let error = error.as_deref().unwrap_or("no reason recorded");
        lines.push(labelled("failed", &format!("{id}: {}", scrub.apply(error))));
    }
    if let Some(stopped) = &outcome.stopped {
        lines.push(labelled("stopped", &scrub.apply(stopped)));
    }
    lines
}

/// Replaces the engine's private absolute paths in a message with labels relative to the
/// planning project (`grida_fx_runtime::engine::scrub_paths`): `<project>/x` becomes `x` and the
/// project itself `.`, the store and the home their project-relative path. The engine has
/// already done so for its own roots; this is a safety net for the planning project's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scrub {
    /// `(absolute path, label)`; the project root's label is `.`.
    paths: Vec<(String, String)>,
}

impl Scrub {
    fn new(planner: &Planner, store: &Store) -> Scrub {
        let project = &planner.project.root;
        let mut paths = vec![(project.display().to_string(), ".".to_string())];
        for other in [store.root(), planner.home.root.as_path()] {
            if other != project.as_path() {
                paths.push((other.display().to_string(), shown_path(other, project)));
            }
        }
        Scrub { paths }
    }

    fn apply(&self, text: &str) -> String {
        scrub_paths(text, &self.paths)
    }
}

/// One `--deliver OUTPUT=PATH` request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Delivery {
    /// As given, for messages.
    request: String,
    name: String,
    pattern: String,
}

impl Delivery {
    fn parse(pair: &str) -> Result<Delivery, Error> {
        match pair.split_once('=') {
            Some((name, pattern)) if !name.is_empty() && !pattern.is_empty() => Ok(Delivery {
                request: pair.to_string(),
                name: name.to_string(),
                pattern: pattern.to_string(),
            }),
            _ => Err(Error::usage(format!("--deliver {pair}: write OUTPUT=PATH"))),
        }
    }

    /// Refuses a name the workflow does not declare.
    fn check_name(&self, declared: &[&str]) -> Result<(), Error> {
        if declared.contains(&self.name.as_str()) {
            return Ok(());
        }
        if declared.is_empty() {
            return Err(Error::usage(format!(
                "--deliver {}: the workflow declares no outputs",
                self.request
            )));
        }
        let mut names = declared.to_vec();
        names.sort_unstable();
        Err(Error::usage(format!(
            "--deliver {}: name one of {}",
            self.request,
            names.join(", ")
        )))
    }

    /// Where each of the output's files goes: `(path as named, file)`. Errors are usage errors.
    fn targets(&self, files: OutputFiles) -> Result<Vec<(String, FileValue)>, Error> {
        let keyed = self.pattern.contains(KEY);
        match files {
            OutputFiles::One(_) if keyed => Err(Error::usage(format!(
                "--deliver {}: {} is one file, so no {KEY}",
                self.request, self.name
            ))),
            OutputFiles::One(file) => Ok(vec![(self.pattern.clone(), file)]),
            OutputFiles::Several(files) if !keyed && files.len() > 1 => Err(Error::usage(format!(
                "--deliver {}: name each element with {KEY}",
                self.request
            ))),
            OutputFiles::Several(files) => Ok(files
                .into_iter()
                .map(|(label, file)| (self.pattern.replace(KEY, &keyed_path(&label)), file))
                .collect()),
        }
    }
}

/// The files of one output value.
#[derive(Debug, Clone, PartialEq)]
enum OutputFiles {
    /// The value is one file.
    One(FileValue),
    /// Each file of a list, keyed collection or object, with its label.
    Several(Vec<(String, FileValue)>),
}

/// The files of an output, or `None` when it holds none.
fn output_files(value: Option<&Val>) -> Option<OutputFiles> {
    match value? {
        Val::File(file) => Some(OutputFiles::One((**file).clone())),
        other => {
            let mut found = Vec::new();
            collect_files(other, None, &mut found);
            let labelled: Vec<(String, FileValue)> = found
                .into_iter()
                .enumerate()
                .map(|(position, (key, file))| (key.unwrap_or_else(|| position.to_string()), file))
                .collect();
            (!labelled.is_empty()).then_some(OutputFiles::Several(labelled))
        }
    }
}

/// Every file a value holds, in order, with its key: the file's own when not empty, else the key
/// of the collection item it is.
fn collect_files(value: &Val, item_key: Option<&str>, out: &mut Vec<(Option<String>, FileValue)>) {
    match value {
        Val::File(file) => {
            let key = file
                .key
                .as_deref()
                .filter(|k| !k.is_empty())
                .or(item_key.filter(|k| !k.is_empty()))
                .map(str::to_string);
            out.push((key, (**file).clone()));
        }
        Val::List(items) => {
            for item in items {
                collect_files(item, None, out);
            }
        }
        Val::Collection(collection) => {
            for (key, item) in &collection.items {
                collect_files(item, Some(key), out);
            }
        }
        Val::Object(members) => {
            for item in members.values() {
                collect_files(item, None, out);
            }
        }
        _ => {}
    }
}

/// One step of delivering, planned before anything is written.
enum Planned {
    Copy { shown: String, file: FileValue },
    Missing(String),
}

/// Delivers the requested outputs (module doc, step 7): `Ok(true)` when every output existed.
fn deliver(
    deliveries: &[Delivery],
    outputs: &IndexMap<String, Val>,
    store: &Store,
    cwd: &Path,
) -> Result<bool, Error> {
    let planned = plan_deliveries(deliveries, outputs)?;
    let mut complete = true;
    for step in planned {
        match step {
            Planned::Missing(name) => {
                complete = false;
                print_line(&labelled(
                    "missing",
                    &format!("{name}: the run did not produce it"),
                ));
            }
            Planned::Copy { shown, file } => {
                let target = cwd.join(&shown);
                if holds(&target, &file) {
                    continue;
                }
                copy_from_store(store, &file, &target, &shown)?;
                print_line(&labelled("delivered", &shown));
            }
        }
    }
    Ok(complete)
}

/// Matches every request with its files, refusing a pattern that does not fit, before anything
/// is written.
fn plan_deliveries(
    deliveries: &[Delivery],
    outputs: &IndexMap<String, Val>,
) -> Result<Vec<Planned>, Error> {
    let mut planned = Vec::new();
    for delivery in deliveries {
        match output_files(outputs.get(&delivery.name)) {
            None => planned.push(Planned::Missing(delivery.name.clone())),
            Some(files) => planned.extend(
                delivery
                    .targets(files)?
                    .into_iter()
                    .map(|(shown, file)| Planned::Copy { shown, file }),
            ),
        }
    }
    Ok(planned)
}

/// Whether `target` already holds the file's bytes.
fn holds(target: &Path, file: &FileValue) -> bool {
    let same_size = std::fs::metadata(target).is_ok_and(|m| m.is_file() && m.len() == file.size);
    same_size
        && std::fs::read(target)
            .is_ok_and(|bytes| grida_fx_core::value::file_digest(&bytes) == file.digest)
}

/// Copies the store's copy of `file` to `target`: checked against its digest, written under a
/// temporary name beside the target, flushed, and renamed over it. Errors name `shown`.
fn copy_from_store(
    store: &Store,
    file: &FileValue,
    target: &Path,
    shown: &str,
) -> Result<(), Error> {
    let source = store.file_path(&file.digest)?;
    if !store.has(&file.digest, file.size) || !store.verify(&file.digest)? {
        return Err(Error::new(
            ErrorKind::Io,
            format!(
                "{shown}: the store's copy of {} is missing or does not match its digest; run \
                 again to make it",
                file.name
            ),
        ));
    }
    let parent = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent).map_err(|e| Error::io(shown, &e))?;
    let base = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "delivered".into());
    let temporary = parent.join(format!(".{base}.{}.part", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut reader = std::fs::File::open(&source)?;
        let mut writer = std::fs::File::create(&temporary)?;
        std::io::copy(&mut reader, &mut writer)?;
        writer.flush()?;
        writer.sync_all()?;
        std::fs::rename(&temporary, target)
    })();
    written.map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::io(shown, &error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::money::Usd;
    use grida_fx_core::val::Collection;

    fn file(digest: &str, key: Option<&str>) -> FileValue {
        FileValue {
            digest: digest.repeat(64),
            kind: "text/plain".into(),
            name: "x/text".into(),
            size: 3,
            key: key.map(str::to_string),
            content: None,
            location: None,
        }
    }

    fn files(value: &Val) -> Option<OutputFiles> {
        output_files(Some(value))
    }

    #[test]
    fn deliver_pairs_need_a_name_and_a_path() {
        let delivery = Delivery::parse("each=out/{key}.png").unwrap();
        assert_eq!(delivery.name, "each");
        assert_eq!(delivery.pattern, "out/{key}.png");
        assert_eq!(
            Delivery::parse("a=b=c").unwrap().pattern,
            "b=c",
            "split at the first ="
        );
        for pair in ["each", "=out.png", "each=", ""] {
            let error = Delivery::parse(pair).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Usage);
            assert_eq!(
                error.message,
                format!("--deliver {pair}: write OUTPUT=PATH")
            );
        }
    }

    #[test]
    fn deliver_names_must_be_declared_outputs() {
        let delivery = Delivery::parse("nosuch=x.txt").unwrap();
        assert!(
            Delivery::parse("one=x")
                .unwrap()
                .check_name(&["one"])
                .is_ok()
        );
        let error = delivery.check_name(&["one", "each", "all"]).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Usage);
        assert_eq!(
            error.message,
            "--deliver nosuch=x.txt: name one of all, each, one"
        );
        assert_eq!(
            delivery.check_name(&[]).unwrap_err().message,
            "--deliver nosuch=x.txt: the workflow declares no outputs"
        );
    }

    #[test]
    fn an_outputs_files_and_their_labels() {
        assert_eq!(files(&Val::Null), None);
        assert_eq!(files(&Val::Failed("a#1".into())), None);
        assert_eq!(files(&Val::Str("x".into())), None);
        assert_eq!(files(&Val::List(Vec::new())), None);
        assert_eq!(output_files(None), None);
        assert_eq!(
            files(&Val::File(Box::new(file("a", Some("k"))))),
            Some(OutputFiles::One(file("a", Some("k"))))
        );
        let collection = Val::Collection(Box::new(Collection {
            items: vec![
                ("x".into(), Val::File(Box::new(file("a", None)))),
                ("y z".into(), Val::File(Box::new(file("b", Some("own"))))),
                (
                    "w".into(),
                    Val::List(vec![Val::File(Box::new(file("c", None)))]),
                ),
                ("v".into(), Val::Null),
            ],
            verdicts: IndexMap::new(),
        }));
        let Some(OutputFiles::Several(found)) = files(&collection) else {
            panic!("several files")
        };
        let labels: Vec<&str> = found.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["x", "own", "2"]);
        let list = Val::List(vec![
            Val::File(Box::new(file("a", None))),
            Val::Missing,
            Val::File(Box::new(file("b", Some("")))),
        ]);
        let Some(OutputFiles::Several(found)) = files(&list) else {
            panic!("several files")
        };
        let labels: Vec<&str> = found.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["0", "1"]);
    }

    #[test]
    fn a_pattern_must_fit_its_output() {
        let one = OutputFiles::One(file("a", None));
        let several = |n: usize| {
            OutputFiles::Several(
                (0..n)
                    .map(|i| (i.to_string(), file(&i.to_string(), None)))
                    .collect(),
            )
        };
        let delivery = |pair: &str| Delivery::parse(pair).unwrap();
        assert_eq!(
            delivery("one=out/one.txt").targets(one.clone()).unwrap(),
            [("out/one.txt".to_string(), file("a", None))]
        );
        assert_eq!(
            delivery("one=out/{key}.txt")
                .targets(one)
                .unwrap_err()
                .message,
            "--deliver one=out/{key}.txt: one is one file, so no {key}"
        );
        assert_eq!(
            delivery("each=out/all.txt")
                .targets(several(2))
                .unwrap_err()
                .message,
            "--deliver each=out/all.txt: name each element with {key}"
        );
        // A single element needs no {key}.
        assert_eq!(
            delivery("each=out/all.txt").targets(several(1)).unwrap(),
            [("out/all.txt".to_string(), file("0", None))]
        );
    }

    #[test]
    fn keys_become_paths_inside_the_pattern() {
        let several = OutputFiles::Several(vec![
            ("x".into(), file("a", None)),
            ("y z".into(), file("b", None)),
            ("../up".into(), file("c", None)),
            ("a/b".into(), file("d", None)),
        ]);
        let targets = Delivery::parse("each=out/{key}.txt")
            .unwrap()
            .targets(several)
            .unwrap();
        let shown: Vec<&str> = targets.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            shown,
            ["out/x.txt", "out/y_z.txt", "out/_/up.txt", "out/a/b.txt"]
        );
    }

    #[test]
    fn deliveries_are_checked_before_anything_is_written() {
        let outputs: IndexMap<String, Val> = [
            ("one".to_string(), Val::File(Box::new(file("a", None)))),
            (
                "each".to_string(),
                Val::List(vec![
                    Val::File(Box::new(file("b", None))),
                    Val::File(Box::new(file("c", None))),
                ]),
            ),
            ("bad".to_string(), Val::Failed("x#1".into())),
        ]
        .into_iter()
        .collect();
        let requests = |pairs: &[&str]| -> Vec<Delivery> {
            pairs.iter().map(|p| Delivery::parse(p).unwrap()).collect()
        };
        // The second request does not fit: nothing is planned, so nothing is written.
        let error = plan_deliveries(&requests(&["one=o.txt", "each=e.txt"]), &outputs)
            .err()
            .unwrap();
        assert_eq!(
            error.message,
            "--deliver each=e.txt: name each element with {key}"
        );
        let planned = plan_deliveries(&requests(&["bad=b.txt", "one=o.txt"]), &outputs).unwrap();
        assert!(matches!(&planned[0], Planned::Missing(name) if name == "bad"));
        assert!(matches!(&planned[1], Planned::Copy { shown, .. } if shown == "o.txt"));
        // Delivering a missing output prints its line and reports it.
        let store = Store::open(Path::new("/nonexistent-store"));
        let folder = tempfile::tempdir().unwrap();
        assert!(!deliver(&requests(&["bad=b.txt"]), &outputs, &store, folder.path()).unwrap());
        assert!(deliver(&[], &outputs, &store, folder.path()).unwrap());
    }

    #[test]
    fn a_target_holding_the_same_bytes_is_left_alone() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("one.txt");
        let bytes = b"abc";
        let mut same = file("a", None);
        same.digest = grida_fx_core::value::file_digest(bytes);
        same.size = 3;
        assert!(!holds(&target, &same));
        std::fs::write(&target, bytes).unwrap();
        assert!(holds(&target, &same));
        std::fs::write(&target, b"abd").unwrap();
        assert!(!holds(&target, &same));
        std::fs::write(&target, b"abcd").unwrap();
        assert!(!holds(&target, &same));
    }

    #[test]
    fn a_delivered_file_is_a_writable_copy_of_the_stores() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::open(&folder.path().join("cache"));
        let stored = store.put_bytes(b"ADA").unwrap();
        let mut value = file("a", None);
        value.digest = stored.digest.clone();
        value.size = stored.size;
        let outputs: IndexMap<String, Val> = [("one".to_string(), Val::File(Box::new(value)))]
            .into_iter()
            .collect();
        let requests = vec![Delivery::parse("one=out/one.txt").unwrap()];
        assert!(deliver(&requests, &outputs, &store, folder.path()).unwrap());
        let target = folder.path().join("out/one.txt");
        assert_eq!(std::fs::read(&target).unwrap(), b"ADA");
        assert!(!std::fs::metadata(&target).unwrap().permissions().readonly());
        // Again: the same bytes are left alone.
        assert!(deliver(&requests, &outputs, &store, folder.path()).unwrap());
    }

    fn outcome() -> RunOutcome {
        RunOutcome {
            folder: PathBuf::from("/work/acme/runs/one"),
            ok: false,
            incomplete: false,
            stopped: None,
            charged: Usd(125_000),
            failed: vec![
                ("nope#1".into(), Some("refused on purpose".into())),
                ("bang#1".into(), None),
            ],
            outputs: IndexMap::new(),
            cancelled: false,
        }
    }

    fn no_scrub() -> Scrub {
        Scrub { paths: Vec::new() }
    }

    #[test]
    fn the_summary_lines() {
        assert_eq!(
            summary("runs/one", None, &outcome(), &no_scrub()),
            [
                "run       runs/one",
                "result    failed   spent $0.12",
                "failed    nope#1: refused on purpose",
                "failed    bang#1: no reason recorded",
            ]
        );
        let stopped = RunOutcome {
            incomplete: true,
            failed: Vec::new(),
            stopped: Some(
                "phase 2 may cost up to $0.08, which takes the run past --yes-up-to 0.01; approve \
                 it with a higher --yes-up-to"
                    .into(),
            ),
            ..outcome()
        };
        let lines = summary("runs/one", None, &stopped, &no_scrub());
        assert_eq!(lines[1], "result    incomplete   spent $0.12");
        assert_eq!(
            lines[2],
            "stopped   phase 2 may cost up to $0.08, which takes the run past --yes-up-to 0.01; \
             approve it with a higher --yes-up-to"
        );
        let ok = RunOutcome {
            ok: true,
            failed: Vec::new(),
            charged: Usd(0),
            ..outcome()
        };
        assert_eq!(
            summary("runs/case/2026-10-06-1", None, &ok, &no_scrub()),
            [
                "run       runs/case/2026-10-06-1",
                "result    ok   spent $0.00"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_paths_are_shown_relative_to_the_project() {
        let scrub = Scrub {
            paths: vec![
                ("/work/acme/.fx/cache".into(), ".fx/cache".into()),
                ("/work/shared".into(), "../shared".into()),
                ("/work/acme".into(), ".".into()),
            ],
        };
        assert_eq!(
            scrub.apply(
                "cannot read /work/acme/.fx/cache/work/r1/x.txt or /work/acme/nodes/n.py in \
                 /work/acme, nor /work/shared/y"
            ),
            "cannot read .fx/cache/work/r1/x.txt or nodes/n.py in ., nor ../shared/y"
        );
        assert_eq!(scrub.apply("nothing private"), "nothing private");
        // A longer name is another path; a sentence may end right after the root.
        assert_eq!(
            scrub.apply("/work/acme2/x and /x/work/acme/y in /work/acme."),
            "/work/acme2/x and /x/work/acme/y in .."
        );
        assert_eq!(scrub.apply("(/work/acme)"), "(.)");
    }
}
