//! Continue an exact existing record with the execution context it saved.

use crate::cli::{RerunArgs, ResumeArgs, RunArgs, SavedRunOptions};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::project::{Target, is_workflow_file};
use grida_fx_runtime::{
    control,
    folder::{RunFolder, holds_stand_in, same_file},
    run_index::recording_project,
};
use grida_fx_viewer::service::{LaunchOperation, LaunchRequest};
use std::path::{Path, PathBuf};

pub fn run(args: &ResumeArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let folder = select(&args.run, &cwd)?;
    execute(
        &args.run,
        folder,
        cwd,
        &args.options,
        Operation::Resume,
        None,
    )
}

pub fn rerun(args: &RerunArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let label = args
        .run
        .clone()
        .map_or_else(|| super::history::pick(&cwd), Ok)?;
    let folder = super::control::exact_folder_at(&label, &cwd, None, args.attempt)
        .map_err(|(_, message)| Error::usage(message))?;
    let pinned = args.attempt.is_some() || cwd.join(&label).is_dir();
    execute(
        &label,
        folder,
        cwd,
        &args.options,
        Operation::Rerun {
            key: args.request_id.as_deref(),
            pinned,
        },
        None,
    )
}

struct ViewerStart<'a> {
    request: &'a LaunchRequest,
    receipt: &'a super::viewer_run::Receipt,
}

#[cfg(unix)]
pub(super) fn viewer_start(
    request: &LaunchRequest,
    receipt: &super::viewer_run::Receipt,
) -> Result<u8, Error> {
    if super::planning::working_directory()? != request.project_root {
        return Err(receipt.refuse(
            "context_unavailable",
            "The run's planning project is unavailable.",
        ));
    }
    let key = super::history::request_id(&request.request_id).map_err(Error::usage)?;
    let operation = match request.operation {
        LaunchOperation::Resume => Operation::Resume,
        LaunchOperation::Rerun => Operation::Rerun {
            key: Some(&key),
            pinned: true,
        },
    };
    execute(
        "selected attempt",
        request.folder.clone(),
        request.project_root.clone(),
        &SavedRunOptions {
            require_estimates: false,
            live: false,
            max_usd: None,
            yes_up_to: None,
            // This runner owns the wait even after the browser disconnects. Answers
            // and deadlines can continue execution without starting another runner.
            wait_for_answers: true,
            no_view: true,
            standalone: false,
            open: false,
        },
        operation,
        Some(ViewerStart { request, receipt }),
    )
}

fn select(label: &str, cwd: &Path) -> Result<PathBuf, Error> {
    super::control::exact_folder(label, cwd, None).map_err(|(_, message)| Error::usage(message))
}

enum Operation<'a> {
    Resume,
    Rerun { key: Option<&'a str>, pinned: bool },
}

fn execute(
    label: &str,
    folder: PathBuf,
    cwd: PathBuf,
    args: &SavedRunOptions,
    operation: Operation<'_>,
    viewer: Option<ViewerStart<'_>>,
) -> Result<u8, Error> {
    let folder = folder
        .canonicalize()
        .map_err(|_| Error::usage(format!("{label} is not a run folder")))?;
    let plan_path = folder.join("plan.json");
    let original_plan = std::fs::File::open(&plan_path)
        .map_err(|_| Error::usage(format!("{label} is not a run folder")))?;
    let plan = super::read_plan(&folder, label)?;
    let source = plan["workflow"]["file"].as_str();
    let here = Project::find(&cwd).ok().map(|project| project.root);
    let root = recording_project(&folder, here.as_deref().unwrap_or(&folder));
    if let Some(viewer) = &viewer {
        if root.as_ref() != Some(&viewer.request.project_root) {
            return Err(viewer.receipt.refuse(
                "context_unavailable",
                "FX couldn’t find the project or the information it needs to continue this run.",
            ));
        }
        if folder != viewer.request.folder
            || !super::history::recorded(&folder)?.is_some_and(|attempt| {
                attempt.run_id == viewer.request.run_id
                    && attempt.attempt_number == viewer.request.source_attempt
            })
        {
            return Err(viewer.receipt.refuse(
                "invocation_mismatch",
                "This attempt changed. Refresh the page and try again.",
            ));
        }
    }
    let rerun = if let Operation::Rerun { key, pinned } = operation {
        let root = root
            .as_ref()
            .ok_or_else(|| Error::usage("the original planning project could not be identified"))?;
        let attempt = super::history::recorded(&folder)?
            .ok_or_else(|| Error::usage("the source has no recorded attempt"))?;
        let request_digest = key.map(|key| grida_fx_core::value::sha256_hex(key.as_bytes()));
        let index_lock = grida_fx_runtime::run_index::allocate_attempt(root)
            .map_err(|error| Error::io("project run index", &error))?;
        if let Some(path) = super::history::accepted(
            root,
            &attempt.run_id,
            request_digest.as_deref(),
            pinned.then_some(attempt.attempt_number),
        )? {
            super::rerun::report_accepted(&path, root)?;
            if let Some(viewer) = &viewer {
                viewer.receipt.accept_existing(&path)?;
            }
            return Ok(0);
        }
        if let Some(digest) = request_digest.as_deref()
            && !pinned
            && let Some(reservation) = super::history::journal(root, &attempt.run_id)?
                .into_iter()
                .find(|entry| entry.request_digest.as_deref() == Some(digest))
            && reservation.source_attempt != attempt.attempt_number
        {
            let original = super::history::select(folder, &cwd, Some(reservation.source_attempt))?;
            drop(index_lock);
            return execute(
                label,
                original,
                cwd,
                args,
                Operation::Rerun { key, pinned: true },
                viewer,
            );
        }
        if viewer.is_none() {
            // Refuse an active source before loading its builder or planning work.
            // The allocator checks the lock and invocation again after planning.
            RunFolder::lock_existing(&folder, "rerun source")
                .map_err(|error| Error::usage(error.0))?;
        }
        Some(super::rerun::Rerun {
            source: &folder,
            attempt,
            request_digest,
            pinned,
        })
    } else {
        None
    };
    let missing = |why: &str| {
        if let Some(viewer) = &viewer {
            return viewer.receipt.refuse(
                "context_unavailable",
                "FX needs the original workflow and inputs to continue. Check this attempt in the terminal for details.",
            );
        }
        if rerun.is_some() {
            return Error::usage(format!("{label}: {why}"));
        }
        let target = source.map_or("TARGET".into(), super::repeat::path_word);
        Error::usage(format!(
            "{label}: {why}. Supply the original target, inputs and options from the original working directory with {} run \
             {target} --run {}",
            crate::invoker::command(),
            super::repeat::path_word(&crate::print::shown_path(&folder, &cwd))
        ))
    };
    if holds_stand_in(&plan) {
        return Err(missing("this run requires its original --stand-in"));
    }
    let events = super::read_log(&folder, label, true)?;
    if !same_file(&original_plan, &plan_path) {
        return Err(Error::usage(format!(
            "{label} was replaced while selecting the run; inspect the record and try again"
        )));
    }
    let started = events
        .iter()
        .rfind(|event| super::event_name(event) == Some("run_started"))
        .ok_or_else(|| missing("the record has no saved invocation"))?;
    let invocation = started["invocation_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| missing("the record has no invocation ID"))?;
    if let Some(viewer) = &viewer {
        if viewer.request.expected_invocation != invocation {
            return Err(viewer.receipt.refuse(
                "invocation_mismatch",
                "This attempt has started again. Check its status before trying again.",
            ));
        }
        if matches!(viewer.request.operation, LaunchOperation::Resume) {
            let status = control::inspect(&folder);
            if status.invocation_id.as_deref() != Some(invocation) {
                return Err(viewer.receipt.refuse(
                    "invocation_mismatch",
                    "This attempt has started again. Check its status before trying again.",
                ));
            }
            if status.recorded_state != "paused" {
                return Err(viewer.receipt.refuse(
                    "not_paused",
                    "This attempt is no longer paused. Check the run’s status.",
                ));
            }
        }
        // Sample the writer before applying any saved execution context. The runner checks
        // the invocation again when it acquires this lock to commit the new start.
        RunFolder::lock_existing(&folder, "selected attempt").map_err(|_| {
            viewer.receipt.refuse(
                "source_active",
                "This attempt is still running or stopping. Try again once it stops.",
            )
        })?;
    }
    let saved = super::repeat::Repeat::parse(&started["repeat"])
        .ok_or_else(|| missing("the record lacks the execution context for automatic resume"))?;
    if saved.live && !args.live {
        if let Some(viewer) = &viewer {
            return Err(viewer.receipt.refuse(
                "approval_required",
                "This attempt had live access enabled. Use --live in your terminal with an approved spending limit if needed.",
            ));
        }
        return Err(Error::usage(format!(
            "{label}: this run used live provider access. Pass --live and supply a spending ceiling when required"
        )));
    }
    if rerun.is_some() && args.live != saved.live {
        return Err(Error::usage("rerun requires the source execution mode"));
    }
    let root =
        root.ok_or_else(|| missing("the original planning project could not be identified"))?;
    let working_dir = saved
        .cwd
        .as_ref()
        .map_or_else(|| root.clone(), |cwd| root.join(cwd));
    if !working_dir.is_dir() || working_dir.canonicalize().ok().as_ref() != Some(&working_dir) {
        return Err(missing(
            "the recorded working directory is missing or has moved",
        ));
    }
    let path = |relative: &str| -> Result<String, Error> {
        if !project_file(&root, relative) {
            return Err(missing(&format!(
                "the recorded file {relative} is missing or outside the planning project"
            )));
        }
        root.join(relative)
            .into_os_string()
            .into_string()
            .map_err(|_| missing("the recorded path cannot be represented as UTF-8"))
    };
    let target = match saved.target {
        Target::Workflow(file) if is_workflow_file(&file) => path(&file)?,
        Target::Workflow(id) => id,
        Target::Builder { file, function } => format!("{}:{function}", path(&file)?),
    };
    let folder = folder
        .to_str()
        .ok_or_else(|| Error::usage("the run folder cannot be represented as UTF-8"))?;
    super::run::resume(
        &RunArgs {
            require_estimates: args.require_estimates,
            resolved_models: None,
            target,
            inputs: saved
                .inputs
                .iter()
                .map(|file| path(file))
                .collect::<Result<_, _>>()?,
            routes: saved
                .routes
                .iter()
                .map(|file| path(file))
                .collect::<Result<_, _>>()?,
            arg: Vec::new(),
            max_usd: args.max_usd.clone(),
            live: args.live,
            yes_up_to: args.yes_up_to.clone(),
            wait_for_answers: args.wait_for_answers,
            deliver: Vec::new(),
            run: Some(folder.to_owned()),
            name: None,
            resume: None,
            recover: false,
            stand_in: None,
            standalone: args.standalone,
            no_view: args.no_view,
            open: args.open,
            rest: Vec::new(),
        },
        working_dir,
        super::run::ResumeContext {
            root: &root,
            invocation,
            defaulted_inputs_digest: saved.defaulted_inputs_digest.as_deref(),
            rerun,
            viewer_receipt: viewer.map(|viewer| viewer.receipt),
        },
    )
}

fn project_file(root: &Path, relative: &str) -> bool {
    root.join(relative)
        .canonicalize()
        .is_ok_and(|file| file.starts_with(root) && file.is_file())
}
