//! Continue an exact existing record with the execution context it saved.

use crate::cli::{ResumeArgs, RunArgs};
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::project::{Target, is_workflow_file};
use grida_fx_runtime::{
    folder::{holds_stand_in, same_file},
    run_index::recording_project,
};
use std::path::Path;

pub fn run(args: &ResumeArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let folder = super::control::exact_folder(&args.run, &cwd, None)
        .map_err(|(_, message)| Error::usage(message))?;
    let label = &args.run;
    let folder = folder
        .canonicalize()
        .map_err(|_| Error::usage(format!("{label} is not a run folder")))?;
    let plan_path = folder.join("plan.json");
    let original_plan = std::fs::File::open(&plan_path)
        .map_err(|_| Error::usage(format!("{label} is not a run folder")))?;
    let plan = super::read_plan(&folder, label)?;
    let source = plan["workflow"]["file"].as_str();
    let missing = |why: &str| {
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
    let saved = super::repeat::Repeat::parse(&started["repeat"])
        .ok_or_else(|| missing("the record lacks the execution context for automatic resume"))?;
    if saved.live && !args.live {
        return Err(Error::usage(format!(
            "{label}: this run used live provider access. Pass --live and supply a spending ceiling when required"
        )));
    }
    let here = Project::find(&cwd).ok().map(|project| project.root);
    let root = recording_project(&folder, here.as_deref().unwrap_or(&folder))
        .ok_or_else(|| missing("the original planning project could not be identified"))?;
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
        },
    )
}

fn project_file(root: &Path, relative: &str) -> bool {
    root.join(relative)
        .canonicalize()
        .is_ok_and(|file| file.starts_with(root) && file.is_file())
}
