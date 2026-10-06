//! `schema <target>`: the compiled inputs schema of the target's workflow, with `$ref` resolved
//! (FX: relative to the home), printed as JSON. Exit 0, or 2 on errors.
//!
//! The home is the nearest fx.yaml above the workflow file; a builder's workflow has the project
//! of the working directory as its home, as when planning.

use crate::cli::SchemaArgs;
use crate::print::print_json;
use grida_fx_core::Error;
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::workflow::LoadedWorkflow;
use grida_fx_core::inputs::{compile_inputs, resolve_ref};
use grida_fx_core::project::{Target, load_target};
use indexmap::IndexMap;

pub fn run(args: &SchemaArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let mut host = crate::print::planning_host(&args.target, &cwd);
    let (project, workflow) = load_target(&args.target, &cwd, &IndexMap::new(), &mut host)?;
    let home = home_of(&args.target, project, &workflow)?;
    let mut resolve = |reference: &str| resolve_ref(&home.root, reference);
    let schema = compile_inputs(&workflow.workflow.inputs, Some(&mut resolve))?;
    print_json(&schema);
    Ok(0)
}

/// The workflow's home project: the planning project for a builder, else the nearest fx.yaml
/// above the workflow file.
pub(crate) fn home_of(
    target: &str,
    project: Project,
    workflow: &LoadedWorkflow,
) -> Result<Project, Error> {
    match Target::parse(target) {
        Target::Builder { .. } => Ok(project),
        Target::Workflow(_) => Project::find(&workflow.path),
    }
}
