//! The command line (conformance/README.md, "The command line the cases use" and "Exit
//! status").
//!
//! Exit status: 0 success; 1 the command read everything and refused (planning problems, a stale
//! fx.lock, a missing tool in `doctor`); 2 unreadable or invalid input and usage errors (clap's
//! own errors exit 2 too). An [`grida_fx_core::Error`] prints `grida-fx: <message>` on stderr.
//! Results go to stdout. Step-3 verbs (`run`, `reroll`, `pick`, `takes`, `jobs`, `project`,
//! `inspect`) parse any arguments and exit 2 with `grida-fx: <verb> is not available until the
//! runner lands`.
//!
//! The planning verbs (`plan`, `expand`, `identity`, `price`) take workflow input flags after
//! their own options; [`crate::args::split_plan_args`] separates the two before clap sees them.
//!
//! As with the predecessor's argparse, a repeated option takes its last value (`--max-usd 1
//! --max-usd 2` is a ceiling of $2; `--check --check` is `--check`); repeatable options
//! (`--inputs`, `--routes`, `--arg`, `--same`) still collect every value. `--max-usd` takes a
//! negative number as its value, so FX's own message refuses it.
//!
//! A verb runs on a thread of its own with a [`VERB_STACK`] stack: expansion, expressions and
//! values recurse as deep as the documents nest, and the main thread's few megabytes overflow on a
//! long chain of steps. The verb's exit status is the command's; a panic in it stays a panic.

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
    /// run a workflow; --live admits paid calls
    Run(LaterArgs),
    /// draw the next take of one step
    Reroll(LaterArgs),
    /// use one take of a step from now on
    Pick(LaterArgs),
    /// list or repair a takes file
    Takes(LaterArgs),
    /// long provider jobs submitted and not collected
    Jobs(LaterArgs),
    /// a run's record projected to its state, as JSON
    Project(LaterArgs),
    /// a run's summary, from its own folder
    Inspect(LaterArgs),
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

/// Any arguments: the verb is not available yet.
#[derive(Debug, Args, Clone)]
pub struct LaterArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub rest: Vec<String>,
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
    let planning = move |which: PlanVerb, mut args: PlanArgs| {
        args.rest = rest;
        verbs::planning::run(which, &args)
    };
    match verb {
        Verb::Plan(args) => planning(PlanVerb::Plan, args),
        Verb::Expand(args) => planning(PlanVerb::Expand, args),
        Verb::Identity(args) => planning(PlanVerb::Identity, args),
        Verb::Price(args) => planning(PlanVerb::Price, args),
        Verb::Schema(args) => verbs::schema::run(&args),
        Verb::Nodes(args) => verbs::nodes::run(&args),
        Verb::Doctor(args) => verbs::doctor::run(&args),
        Verb::Lock(args) => verbs::lock::run(&args),
        Verb::Run(_) => verbs::later::run("run"),
        Verb::Reroll(_) => verbs::later::run("reroll"),
        Verb::Pick(_) => verbs::later::run("pick"),
        Verb::Takes(_) => verbs::later::run("takes"),
        Verb::Jobs(_) => verbs::later::run("jobs"),
        Verb::Project(_) => verbs::later::run("project"),
        Verb::Inspect(_) => verbs::later::run("inspect"),
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
