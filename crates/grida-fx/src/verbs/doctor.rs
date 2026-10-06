//! `doctor [TARGET]`: without a target, `engine    grida-fx <version>` and the Python host line
//! (`python    <interpreter> (grida <sdk_version>)` or `python    <interpreter> NOT FOUND`);
//! with a target, the workflow is loaded (no inputs needed, FX), every project node type it uses
//! (depth-first, groups after their members' group, not descending into used workflows) is
//! resolved (`missing   <uses>: <problem>`), and each declared tool printed
//! `format!("tool      {name:12} {path or NOT FOUND}")`. Exit 1 when anything is missing.
//!
//! The engine and Python lines come first, with or without a target. The Python line names the
//! interpreter the home project's node host would use (the home's `.venv`, then the planning
//! project's; relative to the home when it is one of those, else as configured) and the `grida` version it reports once started; `NOT FOUND`
//! when it cannot be started. That line is information: a workflow that needs the host and cannot
//! start it shows `missing` lines for its node types, which set the exit status. Steps are visited
//! in declaration order, each group before its members; built-ins and repeated `uses` are
//! skipped.
//!
//! **Providers** (spec/providers.md §3, §10), after the engine and Python lines and before the
//! `tool` and `missing` lines, for the **planning** project (the one found from where the command
//! runs; with a target, the project planning would use). Nothing that could reach the network is
//! constructed:
//! - the allowlisted variables are read as a live run reads them (the process environment, then
//!   the planning project's `.env` unless `GRIDA_FX_DISABLE_DOTENV=1`). A `.env` that is refused
//!   prints `keys      <sentence>` (the sentence names the file, the variable and the line, never a
//!   value) and the rest is reported from the process environment alone; a base-URL variable
//!   that is refused prints `keys      <VARIABLE>: <sentence>` (or `… holds a credential`).
//!   Either makes doctor exit 1, as either stops a live run;
//! - one line per provider in `live::PROVIDERS` order: `key       <VARIABLE> present
//!   (environment)`, `… present (.env)` or `… missing`. A missing key alone does not change the
//!   exit status. A key's value is never printed;
//! - one line per route of the planning project's catalog (the built-in table, then `fx.yaml`
//!   `route_tables`), sorted by capability, then id: `route     <capability> <model@provider>
//!   servable`, `… no key (<VARIABLE>)` or `… no adapter`, as `live::servable` answers over every
//!   provider's adapters built on `live::offline_setup`.

use crate::cli::DoctorArgs;
use crate::print::{labelled, print_line};
use grida_fx_core::Error;
use grida_fx_core::docs::lock::read_lock;
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::workflow::{Step, Steps};
use grida_fx_core::host::NodeHost;
use grida_fx_core::project::load_target;
use grida_fx_core::registry::{Registry, RegistryError, Resolved};
use grida_fx_core::routes::{Route, RouteTable, load_catalog};
use grida_fx_protocol::DescribeParams;
use grida_fx_providers::keys::{ALLOWED, Environment, KeySource};
use grida_fx_providers::live::{self, Servable};
use grida_fx_providers::{Endpoints, KeyName, Keys};
use grida_fx_runtime::host::PythonHost;
use grida_fx_runtime::host::locate::python_interpreter;
use grida_fx_runtime::tools::{resolve_tool, tool_name};
use indexmap::IndexMap;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

pub fn run(args: &DoctorArgs) -> Result<u8, Error> {
    let env = |name: &str| std::env::var(name).ok();
    let cwd = super::planning::working_directory()?;
    print_line(&format!(
        "engine    {} {}",
        grida_fx_core::ENGINE_NAME,
        grida_fx_core::ENGINE_VERSION
    ));
    let Some(target) = &args.target else {
        let mut host = crate::print::host();
        let project = Project::find(&cwd)?;
        print_line(&python_line(&mut host, &project, None, &env));
        let providers = Providers::of(&project, &env)?;
        providers.print();
        return Ok(u8::from(providers.refused));
    };
    let mut host = crate::print::planning_host(target, &cwd);
    let (project, workflow) = load_target(target, &cwd, &IndexMap::new(), &mut host)?;
    let home = super::schema::home_of(target, project.clone(), &workflow)?;
    print_line(&python_line(&mut host, &home, Some(&project.root), &env));
    let providers = Providers::of(&project, &env)?;
    providers.print();
    let mut route_defaults = home.route_defaults();
    route_defaults.extend(project.route_defaults());
    let mut registry = Registry::new(
        home.root.clone(),
        home.document.sources.clone(),
        read_lock(&home.root)?,
        route_defaults,
    );
    host.open_project(&home.root, &home.document.sources)?;
    let mut missing = usize::from(providers.refused);
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

/// The provider lines of the planning project (module doc, "Providers").
#[derive(Debug, Clone, PartialEq, Eq)]
struct Providers {
    lines: Vec<String>,
    /// The `.env` or a base URL was refused: a live run would not start.
    refused: bool,
}

impl Providers {
    /// Reads the allowlisted variables of `project` through `env` and describes them and the
    /// project's routes. Only a route table that cannot be read is an error.
    fn of(project: &Project, env: &dyn Fn(&str) -> Option<String>) -> Result<Providers, Error> {
        let mut lines = Vec::new();
        let environment =
            match Environment::read(env, Some(&crate::engine::dotenv_path(&project.root))) {
                Ok(environment) => environment,
                Err(sentence) => {
                    lines.push(dotenv_refusal(&sentence));
                    process_environment(env)
                }
            };
        let keys = Keys::from_environment(&environment);
        if let Err(sentence) = Endpoints::from_environment(&environment, &keys) {
            lines.push(labelled("keys", &sentence));
        }
        let refused = !lines.is_empty();
        lines.extend(key_lines(&keys));
        let catalog = load_catalog(project, &crate::engine::builtin_routes()?, &[])?;
        lines.extend(route_lines(&catalog, &keys));
        Ok(Providers { lines, refused })
    }

    fn print(&self) {
        for line in &self.lines {
            print_line(line);
        }
    }
}

/// The line of a refused `.env`: `keys      <sentence>`, the sentence naming the file (`.env:`
/// is put first when it does not).
fn dotenv_refusal(sentence: &str) -> String {
    if sentence.starts_with(".env") {
        labelled("keys", sentence)
    } else {
        labelled("keys", &format!(".env: {sentence}"))
    }
}

/// The allowlisted variables of the process alone (trimmed; blank is unset), for when the `.env`
/// is refused.
fn process_environment(env: &dyn Fn(&str) -> Option<String>) -> Environment {
    let values: Vec<(&str, String)> = ALLOWED
        .iter()
        .filter_map(|name| env(name).map(|value| (*name, value)))
        .collect();
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    Environment::from_pairs(&pairs)
}

/// One `key` line per provider, in `live::PROVIDERS` order: where its key came from, never its
/// value.
fn key_lines(keys: &Keys) -> Vec<String> {
    live::PROVIDERS
        .iter()
        .filter_map(|provider| KeyName::of_provider(provider))
        .map(|key| {
            let state = match keys.source(key) {
                Some(KeySource::Environment) => "present (environment)",
                Some(KeySource::DotEnv) => "present (.env)",
                None => "missing",
            };
            labelled("key", &format!("{} {state}", key.variable()))
        })
        .collect()
}

/// One `route` line per route of `catalog`, sorted by capability, then id.
fn route_lines(catalog: &RouteTable, keys: &Keys) -> Vec<String> {
    let adapters = live::adapters(&live::offline_setup(keys.clone()));
    let mut routes: Vec<(&str, String, &Route)> = catalog
        .entries
        .values()
        .map(|route| (route.capability.as_str(), route.id(), route))
        .collect();
    routes.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    routes
        .into_iter()
        .map(|(capability, id, route)| {
            let state = match live::servable(&adapters, keys, capability, &route.provider) {
                Servable::Yes => "servable".to_string(),
                Servable::NoKey(key) => format!("no key ({})", key.variable()),
                Servable::NoAdapter => "no adapter".to_string(),
            };
            labelled("route", &format!("{capability} {id} {state}"))
        })
        .collect()
}

/// `python    <interpreter> (grida <version>)`, or `… NOT FOUND` when the host cannot start.
fn python_line(
    host: &mut PythonHost,
    project: &Project,
    planning_root: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let interpreter = python_interpreter(&project.root, planning_root, env);
    let shown = match interpreter.strip_prefix(&project.root) {
        Ok(inside) => inside
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) if planning_root.is_some_and(|root| interpreter.starts_with(root)) => {
            crate::print::shown_path(&interpreter, &project.root)
        }
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
    fn key_lines_name_each_providers_variable_and_where_it_came_from() {
        let value = "fx-made-up-fal-value-41d7";
        let keys = Keys::from_pairs(&[(KeyName::Fal, value)]);
        let lines = key_lines(&keys);
        assert_eq!(
            lines,
            [
                "key       OPENAI_API_KEY missing",
                "key       OPENROUTER_API_KEY missing",
                "key       FAL_KEY present (environment)",
                "key       TRIPO_API_KEY missing",
                "key       ELEVENLABS_API_KEY missing",
            ]
        );
        assert!(lines.iter().all(|line| !line.contains(value)));
    }

    /// A table of `(capability, route)` pairs, priced alike.
    fn table(routes: &[(&str, &str)]) -> RouteTable {
        let entries: Vec<String> = routes
            .iter()
            .map(|(capability, route)| {
                format!(
                    "  - {{ capability: {capability}, route: {route}, price: {{ low_usd: 0.01, \
                     high_usd: 0.02 }} }}\n"
                )
            })
            .collect();
        let text = format!("fx: routes/v1\nroutes:\n{}", entries.concat());
        let document = grida_fx_core::yaml::load(text.as_bytes(), "routes.yaml").unwrap();
        RouteTable::from_document(&document, "routes.yaml").unwrap()
    }

    #[test]
    fn route_lines_say_whether_a_live_run_could_serve_each_route() {
        let catalog = table(&[
            (
                "video.generate",
                "google/gemini-omni-flash/v1.1/image-to-video@fal",
            ),
            ("image.generate", "img-a@acme"),
            ("image.generate", "gpt-image-2.5-sunburst@openai"),
            ("background.remove", "bg@fal"),
            ("vision.review", "judge@openrouter"),
        ]);
        let value = "fx-made-up-fal-value-41d7";
        let keys = Keys::from_pairs(&[(KeyName::Fal, value)]);
        let lines = route_lines(&catalog, &keys);
        assert_eq!(
            lines,
            [
                "route     background.remove bg@fal servable",
                "route     image.generate gpt-image-2.5-sunburst@openai no key (OPENAI_API_KEY)",
                "route     image.generate img-a@acme no adapter",
                "route     video.generate google/gemini-omni-flash/v1.1/image-to-video@fal \
                 servable",
                "route     vision.review judge@openrouter no adapter",
            ]
        );
        assert!(lines.iter().all(|line| !line.contains(value)));
    }

    #[test]
    fn every_built_in_route_has_a_line() {
        let catalog = crate::engine::builtin_routes().unwrap();
        let lines = route_lines(&catalog, &Keys::none());
        assert_eq!(lines.len(), catalog.entries.len());
        assert!(
            lines.iter().all(|line| line.contains(" no key (")),
            "{lines:#?}"
        );
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(sorted, lines, "sorted by capability, then id");
        assert!(
            lines.contains(
                &"route     image.generate gpt-image-2.5-sunburst@openai no key (OPENAI_API_KEY)"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_refused_key_file_is_named_once() {
        assert_eq!(
            dotenv_refusal(".env contains duplicate key: FAL_KEY"),
            "keys      .env contains duplicate key: FAL_KEY"
        );
        assert_eq!(
            dotenv_refusal("cannot be read"),
            "keys      .env: cannot be read"
        );
    }

    #[test]
    fn without_the_key_file_the_process_environment_still_counts() {
        let values = [
            ("FAL_KEY", " fal-made-up "),
            ("TRIPO_API_KEY", "  "),
            ("HOME", "/nowhere"),
            ("OPENAI_BASE_URL", "https://proxy.example.test/v1"),
        ];
        let env = |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        };
        let environment = process_environment(&env);
        assert_eq!(environment.get("FAL_KEY"), Some("fal-made-up"));
        assert_eq!(environment.source("FAL_KEY"), Some(KeySource::Environment));
        assert_eq!(environment.get("TRIPO_API_KEY"), None);
        assert_eq!(environment.get("HOME"), None);
        assert_eq!(
            environment.get("OPENAI_BASE_URL"),
            Some("https://proxy.example.test/v1")
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
