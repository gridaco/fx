//! `doctor [TARGET]`: without a target, `engine    grida-fx <version>` and the Python host line
//! (`python    <interpreter> (grida <sdk_version>)` or `python    <interpreter> NOT FOUND`);
//! with a target, the workflow is loaded (no inputs needed, FX), every project node type it uses
//! (depth-first, groups after their members' group, not descending into used workflows) is
//! resolved (`missing   <uses>: <problem>`), and each declared tool printed
//! `format!("tool      {name:12} {path or NOT FOUND}")`. Exit 1 when anything is missing.
//!
//! The engine and Python lines come first, with or without a target. The Python line names the
//! interpreter the home project's node host would use (project-relative when it lies inside the
//! project, else as configured) and the `grida` version it reports once started; `NOT FOUND`
//! when it cannot be started. That line is information: a workflow that needs the host and cannot
//! start it shows `missing` lines for its node types, which set the exit status. Steps are visited
//! in declaration order, each group before its members; built-ins and repeated `uses` are
//! skipped.

use crate::cli::DoctorArgs;
use crate::print::print_line;
use grida_fx_core::Error;
use grida_fx_core::docs::lock::read_lock;
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::workflow::{Step, Steps};
use grida_fx_core::host::NodeHost;
use grida_fx_core::project::load_target;
use grida_fx_core::registry::{Registry, RegistryError, Resolved};
use grida_fx_protocol::DescribeParams;
use grida_fx_runtime::host::PythonHost;
use grida_fx_runtime::host::locate::python_interpreter;
use grida_fx_runtime::tools::{resolve_tool, tool_name};
use indexmap::IndexMap;
use std::collections::HashSet;
use std::rc::Rc;

pub fn run(args: &DoctorArgs) -> Result<u8, Error> {
    let env = |name: &str| std::env::var(name).ok();
    let cwd = super::planning::working_directory()?;
    print_line(&format!(
        "engine    {} {}",
        grida_fx_core::ENGINE_NAME,
        grida_fx_core::ENGINE_VERSION
    ));
    let mut host = crate::print::host();
    let Some(target) = &args.target else {
        let project = Project::find(&cwd)?;
        print_line(&python_line(&mut host, &project, &env));
        return Ok(0);
    };
    let (project, workflow) = load_target(target, &cwd, &IndexMap::new(), &mut host)?;
    let home = super::schema::home_of(target, project.clone(), &workflow)?;
    print_line(&python_line(&mut host, &home, &env));
    let mut route_defaults = home.route_defaults();
    route_defaults.extend(project.route_defaults());
    let mut registry = Registry::new(
        home.root.clone(),
        home.document.sources.clone(),
        read_lock(&home.root)?,
        route_defaults,
    );
    host.open_project(&home.root, &home.document.sources)?;
    let mut missing = 0usize;
    let mut seen = HashSet::new();
    for step in all_steps(&workflow.workflow.steps) {
        let Some(uses) = &step.uses else { continue };
        if is_builtin(uses) || !seen.insert(uses.clone()) {
            continue;
        }
        match registry.node_type(uses, &mut host) {
            Ok(Resolved::Node(resolved)) => {
                for entry in &resolved.spec.tools {
                    let name = tool_name(entry);
                    let found = resolve_tool(name, &env);
                    let shown = found
                        .as_ref()
                        .map_or_else(|| "NOT FOUND".to_string(), |p| p.display().to_string());
                    print_line(&format!("tool      {name:12} {shown}"));
                    missing += usize::from(found.is_none());
                }
            }
            Ok(Resolved::Workflow(_)) => {}
            Err(RegistryError::Problem(problem)) => {
                print_line(&format!("missing   {uses}: {problem}"));
                missing += 1;
            }
            Err(RegistryError::Fatal(error)) => return Err(error),
        }
    }
    Ok(u8::from(missing > 0))
}

/// `python    <interpreter> (grida <version>)`, or `… NOT FOUND` when the host cannot start.
fn python_line(
    host: &mut PythonHost,
    project: &Project,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let interpreter = python_interpreter(&project.root, env);
    let shown = match interpreter.strip_prefix(&project.root) {
        Ok(inside) => inside
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => interpreter.display().to_string(),
    };
    let started = host
        .open_project(&project.root, &project.document.sources)
        .and_then(|()| {
            host.describe(&DescribeParams {
                targets: Vec::new(),
                builtins: false,
            })
        });
    match (started, &host.info) {
        (Ok(_), Some(info)) => format!("python    {shown} (grida {})", info.host.sdk_version),
        _ => format!("python    {shown} NOT FOUND"),
    }
}

/// Every step, depth-first: each step, then a group's members.
fn all_steps(steps: &Steps) -> Vec<Rc<Step>> {
    let mut found = Vec::new();
    for step in steps.values() {
        found.push(step.clone());
        if let Some(members) = &step.steps {
            found.extend(all_steps(members));
        }
    }
    found
}

/// `fx/<name>@<major>`: a built-in, which needs no host and no tool.
fn is_builtin(uses: &str) -> bool {
    let Some((name, major)) = uses
        .strip_prefix("fx/")
        .and_then(|rest| rest.rsplit_once('@'))
    else {
        return false;
    };
    let word = |w: &str| {
        w.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && w.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit()) && name.split('.').all(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::docs::workflow::ViewSetting;

    fn step(uses: Option<&str>, members: Option<Vec<(&str, Rc<Step>)>>) -> Rc<Step> {
        Rc::new(Step {
            uses: uses.map(str::to_string),
            steps: members.map(|m| {
                Rc::new(
                    m.into_iter()
                        .map(|(name, s)| (name.to_string(), s))
                        .collect::<IndexMap<_, _>>(),
                )
            }),
            with: IndexMap::new(),
            if_: None,
            needs: Vec::new(),
            for_each: None,
            as_: "item".into(),
            key: None,
            max: None,
            matrix: None,
            judges: None,
            on_reject: Default::default(),
            regenerate: None,
            takes: None,
            pick: None,
            asserts: Vec::new(),
            at_plan: false,
            budget: None,
            concurrency: None,
            route: None,
            requires: Vec::new(),
            independent_of: Vec::new(),
            view: ViewSetting::Flag(false),
            timeout: None,
            title: None,
            description: None,
        })
    }

    #[test]
    fn steps_in_declaration_order_groups_first() {
        let inner = step(
            None,
            Some(vec![
                ("b", step(Some("./nodes/b.py#b"), None)),
                ("c", step(Some("./nodes/c.py#c"), None)),
            ]),
        );
        let steps: Steps = Rc::new(
            [
                ("a".to_string(), step(Some("./nodes/a.py#a"), None)),
                ("group".to_string(), inner),
                ("d".to_string(), step(Some("./w.yaml"), None)),
            ]
            .into_iter()
            .collect(),
        );
        let uses: Vec<Option<String>> = all_steps(&steps).iter().map(|s| s.uses.clone()).collect();
        assert_eq!(
            uses,
            [
                Some("./nodes/a.py#a".to_string()),
                None,
                Some("./nodes/b.py#b".to_string()),
                Some("./nodes/c.py#c".to_string()),
                Some("./w.yaml".to_string()),
            ]
        );
    }

    #[test]
    fn built_ins_are_recognised() {
        assert!(is_builtin("fx/image.generate@1"));
        assert!(is_builtin("fx/select@12"));
        assert!(is_builtin("fx/nosuch@1"));
        assert!(!is_builtin("fx/image.generate"));
        assert!(!is_builtin("fx/Image@1"));
        assert!(!is_builtin("fx/image..x@1"));
        assert!(!is_builtin("fx/@1"));
        assert!(!is_builtin("fx/x@"));
        assert!(!is_builtin("./nodes/x.py#fx"));
        assert!(!is_builtin("acme/pack.x@1"));
    }
}
