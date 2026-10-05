//! The command line (conformance/README.md, "The command line the cases use" and "Exit
//! status").
//!
//! Exit status: 0 success; 1 the command read everything and refused or stopped (planning
//! problems, a stale fx.lock, a missing tool in `doctor`, a run refused before it started, a run
//! that is not ok, an output `--deliver` could not find, files `inspect --verify` found changed);
//! 2 unreadable or invalid input and usage errors (clap's own errors exit 2 too); 130 a run a
//! person interrupted (Ctrl-C). An [`grida_fx_core::Error`] prints `grida-fx: <message>` on
//! stderr. Results, and the refusals that are part of a run's story (`refused: …`, `missing …`),
//! go to stdout. The run verbs (`run`, `reroll`, `pick`, `takes`, `jobs`, `project`, `inspect`)
//! are in [`crate::verbs`].
//!
//! The planning verbs (`plan`, `expand`, `identity`, `price`) and `run` take workflow input flags
//! after their own options; [`crate::args::split_plan_args`] separates the two before clap sees
//! them.
//!
//! As with the predecessor's argparse, a repeated option takes its last value (`--max-usd 1
//! --max-usd 2` is a ceiling of $2; `--check --check` is `--check`); repeatable options
//! (`--inputs`, `--routes`, `--arg`, `--same`, `--deliver`) still collect every value.
//! `--max-usd`, `--yes-up-to` and `pick`'s take accept a negative number as their value, so FX's
//! own message refuses it.
//!
//! A verb runs on a thread of its own with a [`VERB_STACK`] stack: expansion, expressions and
//! values recurse as deep as the documents nest, and the main thread's few megabytes overflow on a
//! long chain of steps. The verb's exit status is the command's; a panic in it stays a panic. That
//! thread is never a runtime thread: the verbs that plan or run start the engine's runtime
//! ([`crate::engine`]) and wait on it from there.

use crate::args::split_plan_args;
use crate::print::print_error;
use crate::verbs;
use crate::verbs::planning::PlanVerb;
use clap::{Args, Parser, Subcommand};
use grida_fx_core::Error;
use std::ffi::OsString;
use std::process::ExitCode;

/// `grida-fx`.
#[derive(Debug, Parser)]
#[command(
    name = "grida-fx",
    version,
    about = "Plan and run FX workflows.",
    args_override_self = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub verb: Verb,
}

/// The verbs.
#[derive(Debug, Subcommand)]
pub enum Verb {
    /// expand, check and price a workflow; never spends
    Plan(PlanArgs),
    /// the expanded graph, as JSON
    Expand(PlanArgs),
    /// each instance's identity, as JSON
    Identity(PlanArgs),
    /// the plan's price by phase, as JSON
    Price(PlanArgs),
    /// the JSON Schema a workflow's inputs compile to
    Schema(SchemaArgs),
    /// the node types FX knows
    Nodes(NodesArgs),
    /// the tools and routes a workflow needs
    Doctor(DoctorArgs),
    /// pin versioned node types to their source
    Lock(LockArgs),
    /// run a workflow; --live admits paid calls, within a ceiling
    Run(RunArgs),
    /// draw the next take of one step of a run
    Reroll(RerollArgs),
    /// use one take of a step of a run from now on
    Pick(PickArgs),
    /// list or repair a takes file
    Takes(TakesArgs),
    /// the cache's long provider jobs; --forget one once you have checked the provider
    Jobs(JobsArgs),
    /// a run's record projected to its state, as JSON
    Project(ProjectArgs),
    /// a run's summary, from its own folder
    Inspect(InspectArgs),
}

/// The common options of the planning verbs, plus the verb-specific flags.
#[derive(Debug, Args, Clone, Default)]
pub struct PlanArgs {
    /// a workflow file, a workflow id, or a builder file.py:function
    pub target: String,
    /// inputs YAML; repeatable; later files win
    #[arg(long = "inputs", value_name = "FILE")]
    pub inputs: Vec<String>,
    /// a route table (fx: routes/v1); repeatable; leaves the built-in table out
    #[arg(long = "routes", value_name = "FILE")]
    pub routes: Vec<String>,
    /// an argument for a builder target
    #[arg(long = "arg", value_name = "NAME=VALUE")]
    pub arg: Vec<String>,
    /// the ceiling in US dollars
    #[arg(long = "max-usd", value_name = "USD", allow_negative_numbers = true)]
    pub max_usd: Option<String>,
    /// plan: exit 1 when the plan has problems
    #[arg(long)]
    pub check: bool,
    /// plan: print the graph as JSON
    #[arg(long)]
    pub json: bool,
    /// plan: exit 1 unless every step is cached
    #[arg(long = "expect-cached")]
    pub expect_cached: bool,
    /// workflow input flags (separated before parsing)
    #[arg(skip)]
    pub rest: Vec<String>,
}

#[derive(Debug, Args, Clone)]
pub struct SchemaArgs {
    pub target: String,
}

#[derive(Debug, Args, Clone)]
pub struct NodesArgs {
    /// a built-in's name or `fx/<name>@<major>`
    #[arg(value_name = "TYPE")]
    pub type_: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct DoctorArgs {
    pub target: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct LockArgs {
    /// a folder (or a file) in the project to lock (default: here)
    #[arg(value_name = "WHERE")]
    pub r#where: Option<String>,
    /// confirm that <path>#<attr> did not change behaviour; repeatable
    #[arg(long = "same", value_name = "NODE")]
    pub same: Vec<String>,
    /// check fx.lock without writing it
    #[arg(long)]
    pub check: bool,
}

/// `run`: the planning options, plus the run's own.
#[derive(Debug, Args, Clone, Default)]
pub struct RunArgs {
    /// a workflow file, a workflow id, or a builder file.py:function
    pub target: String,
    /// inputs YAML; repeatable; later files win
    #[arg(long = "inputs", value_name = "FILE")]
    pub inputs: Vec<String>,
    /// a route table (fx: routes/v1); repeatable; leaves the built-in table out
    #[arg(long = "routes", value_name = "FILE")]
    pub routes: Vec<String>,
    /// an argument for a builder target
    #[arg(long = "arg", value_name = "NAME=VALUE")]
    pub arg: Vec<String>,
    /// the ceiling in US dollars; a live run needs one
    #[arg(long = "max-usd", value_name = "USD", allow_negative_numbers = true)]
    pub max_usd: Option<String>,
    /// admit paid calls
    #[arg(long)]
    pub live: bool,
    /// stop before a later phase that may take the run past this amount
    #[arg(long = "yes-up-to", value_name = "USD", allow_negative_numbers = true)]
    pub yes_up_to: Option<String>,
    /// copy an output after the run: OUTPUT=PATH, {key} for each element; repeatable; names are
    /// checked before the run
    #[arg(long = "deliver", value_name = "OUTPUT=PATH")]
    pub deliver: Vec<String>,
    /// the run folder, relative to here (default: a new one under the project's runs folder);
    /// a folder that holds a run of the same plan is resumed
    #[arg(long = "run", value_name = "FOLDER")]
    pub run: Option<String>,
    /// workflow input flags (separated before parsing)
    #[arg(skip)]
    pub rest: Vec<String>,
}

/// `reroll`: the run's takes file gives the step the take after the latest its run drew.
#[derive(Debug, Args, Clone)]
pub struct RerollArgs {
    /// a run folder
    pub run: String,
    /// a step path as the run shows it (`entity['ada'].draw`)
    pub step: String,
    /// say `--live` in the advice printed after
    #[arg(long)]
    pub live: bool,
}

/// `pick`: the run's takes file gives the step this take, with its result's digest.
#[derive(Debug, Args, Clone)]
pub struct PickArgs {
    /// a run folder
    pub run: String,
    /// a step path as the run shows it
    pub step: String,
    /// the take to use from now on (1 to 1000)
    #[arg(allow_negative_numbers = true)]
    pub take: String,
}

#[derive(Debug, Args, Clone)]
pub struct TakesArgs {
    #[command(subcommand)]
    pub verb: TakesVerb,
}

#[derive(Debug, Subcommand, Clone)]
pub enum TakesVerb {
    /// the takes file's entries, by step path
    List {
        /// a workflow file or id
        target: String,
    },
    /// move an entry to another step path (refused onto an existing entry)
    Mv {
        /// a workflow file or id
        target: String,
        old: String,
        new: String,
    },
}

#[derive(Debug, Args, Clone)]
pub struct JobsArgs {
    /// forget one job (its call key, 64 hex) once you have checked the provider
    #[arg(long = "forget", value_name = "KEY")]
    pub forget: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct ProjectArgs {
    /// a run folder
    pub run: String,
}

#[derive(Debug, Args, Clone)]
pub struct InspectArgs {
    /// a run folder, or a workflow id (its newest run)
    pub run: String,
    /// re-check every placed file against its record
    #[arg(long)]
    pub verify: bool,
    /// print the summary as JSON
    #[arg(long)]
    pub json: bool,
}

/// Parses `argv` (the program name first), runs the verb, and maps the outcome to an exit code.
pub fn main(argv: Vec<OsString>) -> ExitCode {
    let (for_clap, rest) = split_plan_args(&argv);
    let cli = match Cli::try_parse_from(for_clap) {
        Ok(cli) => cli,
        Err(error) => {
            // Help and version print to stdout and exit 0; usage errors print to stderr, exit 2.
            let _ = error.print();
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
        }
    };
    match on_big_stack(move || dispatch(cli.verb, rest)) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            print_error(&error);
            ExitCode::from(2)
        }
    }
}

/// The stack a verb runs on. Reserved, not committed: pages are touched only as deep as the
/// recursion goes.
pub const VERB_STACK: usize = 256 * 1024 * 1024;

/// Runs `work` on a thread with a [`VERB_STACK`] stack and returns what it returns. A panic in
/// `work` is resumed on the calling thread (its message was printed where it happened). When the
/// platform cannot make such a thread, `work` runs here.
pub fn on_big_stack<T, F>(work: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    // The closure moves into the thread only when the thread is made; keep it to run here
    // otherwise.
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(work)));
    let shared = std::sync::Arc::clone(&slot);
    let spawned = std::thread::Builder::new()
        .name("grida-fx".into())
        .stack_size(VERB_STACK)
        .spawn(move || {
            let work = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            work.map(|work| work())
        });
    match spawned.map(std::thread::JoinHandle::join) {
        Ok(Ok(Some(value))) => value,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Ok(Ok(None)) | Err(_) => {
            let work = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("the verb ran neither there nor here");
            work()
        }
    }
}

/// Runs one verb: `Ok(exit status)`, or an error (exit 2). `rest` holds a planning verb's
/// workflow input flags.
fn dispatch(verb: Verb, rest: Vec<String>) -> Result<u8, Error> {
    let planning = |which: PlanVerb, mut args: PlanArgs, rest: Vec<String>| {
        args.rest = rest;
        verbs::planning::run(which, &args)
    };
    match verb {
        Verb::Plan(args) => planning(PlanVerb::Plan, args, rest),
        Verb::Expand(args) => planning(PlanVerb::Expand, args, rest),
        Verb::Identity(args) => planning(PlanVerb::Identity, args, rest),
        Verb::Price(args) => planning(PlanVerb::Price, args, rest),
        Verb::Schema(args) => verbs::schema::run(&args),
        Verb::Nodes(args) => verbs::nodes::run(&args),
        Verb::Doctor(args) => verbs::doctor::run(&args),
        Verb::Lock(args) => verbs::lock::run(&args),
        Verb::Run(mut args) => {
            args.rest = rest;
            verbs::run::run(&args)
        }
        Verb::Reroll(args) => verbs::takes::reroll(&args),
        Verb::Pick(args) => verbs::takes::pick(&args),
        Verb::Takes(args) => verbs::takes::takes(&args),
        Verb::Jobs(args) => verbs::jobs::run(&args),
        Verb::Project(args) => verbs::project::run(&args),
        Verb::Inspect(args) => verbs::inspect::run(&args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recurses `depth` times with a 1 KiB frame that the optimizer cannot drop.
    fn deep(depth: usize) -> usize {
        let frame = std::hint::black_box([depth as u8; 1024]);
        if depth == 0 {
            return usize::from(frame[0]);
        }
        deep(depth - 1) + usize::from(std::hint::black_box(frame)[1023] == 0)
    }

    #[test]
    fn a_verb_runs_deeper_than_a_thread_stack() {
        // At least 16 MiB of frames: past a test thread's 2 MiB and the main thread's 8 MiB.
        let depth = 16 * 1024;
        assert!(on_big_stack(move || deep(depth)) > 0);
    }

    #[test]
    fn a_verb_returns_its_value() {
        assert_eq!(on_big_stack(|| Ok::<u8, Error>(1)).unwrap(), 1);
        let error = on_big_stack(|| Err::<u8, Error>(Error::usage("no"))).unwrap_err();
        assert_eq!(error.message, "no");
    }

    #[test]
    fn a_panic_in_a_verb_stays_a_panic() {
        let caught = std::panic::catch_unwind(|| on_big_stack(|| -> u8 { panic!("broken") }));
        let payload = caught.unwrap_err();
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"broken"));
    }

    #[test]
    fn repeated_options_take_the_last_value_and_lists_keep_every_one() {
        let parse = |args: &[&str]| {
            let argv = std::iter::once("grida-fx").chain(args.iter().copied());
            Cli::try_parse_from(argv).unwrap()
        };
        let Verb::Plan(plan) = parse(&[
            "plan",
            "case",
            "--max-usd",
            "1",
            "--max-usd",
            "-2",
            "--check",
            "--check",
            "--inputs",
            "a.yaml",
            "--inputs",
            "b.yaml",
        ])
        .verb
        else {
            panic!("not plan")
        };
        assert_eq!(plan.max_usd.as_deref(), Some("-2"));
        assert!(plan.check);
        assert_eq!(plan.inputs, ["a.yaml", "b.yaml"]);
        let Verb::Lock(lock) = parse(&["lock", "proj", "--same", "a", "--same", "b"]).verb else {
            panic!("not lock")
        };
        assert_eq!(lock.r#where.as_deref(), Some("proj"));
        assert_eq!(lock.same, ["a", "b"]);
    }
}
