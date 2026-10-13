//! `doctor [TARGET]`: without a target, `engine    grida-fx <version>`, the Python host line
//! (`python    <interpreter> (grida <sdk_version>)`, or why it cannot host nodes: see below) and
//! the TypeScript host line (`node      <node> (@grida/fx <sdk_version>)`, what Node answered, or
//! why it cannot host);
//! with a target, the workflow is loaded (no inputs needed, FX), the routes its paid steps use are
//! listed (`uses` lines, below), every project node type it uses (depth-first, groups after their
//! members' group, not descending into used workflows) is resolved (`missing   <uses>:
//! <problem>`), and each declared tool printed `format!("tool      {name:12} {path or NOT
//! FOUND}")`. Exit 1 when anything is missing or a route the target uses cannot be served.
//!
//! The engine, Python and Node lines come first, with or without a target. The Python line names
//! the interpreter the home project's node host would use (the home's `.venv`, then the planning
//! project's; relative to the home when it is one of those, else as configured), asks it for its
//! version once (`host::locate::probe`), and says:
//! - `python    <interpreter> (grida <version>)`: the host started and reported its `grida`;
//! - `… too old (<version>; FX needs 3.11+)`: the interpreter is older than Python 3.11, which the
//!   host needs (spec/protocol.md §1); it is not started;
//! - `… no grida`: the interpreter runs but cannot import `grida`; it is not started;
//! - `… NOT FOUND`: no program by that name or path;
//! - `… cannot start: <sentence>`: the host was started and failed (a protocol mismatch, …).
//!
//! The Node line names the Node the home project's TypeScript host would use (`GRIDA_FX_NODE`,
//! else `GRIDA_FX_SDK_NODE`, else `node`; as configured), asks it about itself once, and says:
//! - `… NOT FOUND`: no program by that name or path;
//! - `… too old (<version>; FX needs 24+)`: a Node before 24 (spec/protocol.md §1); not started;
//! - `… not Node.js 24+`: a program without `module.registerHooks` (Bun, Deno); not started.
//!
//! Otherwise the TypeScript host is started only for a project with a `package.json` (in the home
//! or the planning project): its host program may come from a `node_modules` above the project,
//! which a project without one never asked for. Without one, the line is what Node answered:
//! `node      <node> (Node <version>)`, or `node      <node>` when it gave no answer. With one:
//! - `node      <node> (@grida/fx <version>)`: the host started and reported its `@grida/fx`;
//! - `… no @grida/fx`: no `@grida/fx` is found from the home, nor from the planning project, nor
//!   one that started the engine;
//! - `… cannot start: <sentence>`: the host could not start (a `@grida/fx` too old to host, a
//!   protocol mismatch, …).
//!
//! These lines are information and do not set the exit status: a workflow whose node types need
//! a host stops doctor with the host's sentence (exit 2) when they are resolved, and a module the
//! host could not load is a `missing` line. Steps are visited in declaration order, each group
//! before its members; built-ins and repeated `uses` are skipped.
//!
//! **The target's routes** (with a target, after the host lines): one line per route its paid
//! steps use, sorted by capability, then id, each with the number of steps using it (a repeated
//! step counts once): `uses      <capability> <model@provider> <state> (<n> steps)`, `<state>`
//! as on `route` lines below. Every step is visited, members of groups and the steps of used
//! workflows included (each use of a workflow counts its steps again; a workflow already being
//! visited is not entered again); `at: plan` steps, which never pay, are skipped. A step's route
//! is chosen as planning chooses it (spec/identity.md §7): its `route:` when its type calls one
//! capability, else the capability's `routes:` default in fx.yaml (the home's, then the planning
//! project's). Neither: `uses      <capability> no route (<n> steps)`; a route the catalog does
//! not hold for the capability: `… <id> not in the catalog (<n> steps)`. A node type that does
//! not resolve adds no line. Any state but `servable` makes doctor exit 1, as a live run could
//! not make that step's calls. Routes need no inputs: they are written in the workflow and
//! fx.yaml.
//!
//! **Providers** (spec/providers.md §3, §10), after the engine and host lines (and the target's
//! routes) and before the `tool` and `missing` lines, for the **planning** project (the one found
//! from where the command runs; with a target, the project planning would use). Nothing that could
//! reach the network is constructed:
//! - the allowlisted variables are read as a live run reads them (the process environment, then
//!   the planning project's `.env` unless `GRIDA_FX_DISABLE_DOTENV=1`). A `.env` that is refused
//!   prints `keys      <sentence>` (the sentence names the file, the variable and the line, never a
//!   value) and the rest is reported from the process environment alone; a base-URL variable
//!   that is refused prints `keys      <VARIABLE>: <sentence>` (or `… holds a credential`).
//!   Either makes doctor exit 1, as either stops a live run;
//! - one line per provider in `live::PROVIDERS` order: `key       <VARIABLE> present
//!   (environment)`, `… present (.env)` or `… missing`. A missing key alone does not change the
//!   exit status, unless a route the target uses needs it (its `uses` line is then not
//!   servable). A key's value is never printed;
//! - one line per route of the planning project's catalog (the built-in table, then `fx.yaml`
//!   `route_tables`), sorted by capability, then id: `route     <capability> <model@provider>
//!   servable`, `… no key (<VARIABLE>)`, `… not installed (codex)` (or `(grok)`) or `… no adapter`, as
//!   `live::servable` answers over every provider's adapters built on `live::offline_setup`.

use crate::cli::DoctorArgs;
use crate::print::{labelled, print_line};
use grida_fx_core::Error;
use grida_fx_core::docs::lock::read_lock;
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::workflow::{Step, Steps};
use grida_fx_core::host::{HostFailure, Language, NodeHost};
use grida_fx_core::project::load_target;
use grida_fx_core::registry::{Registry, RegistryError, Resolved};
use grida_fx_core::routes::{Route, RouteTable, load_catalog};
use grida_fx_protocol::DescribeParams;
use grida_fx_providers::keys::{ALLOWED, Environment, KeySource};
use grida_fx_providers::live::{self, Servable};
use grida_fx_providers::{Endpoints, KeyName, Keys};
use grida_fx_runtime::host::locate::{
    MINIMUM_NODE, MINIMUM_PYTHON, NO_SDK, Probe, node_runtime, python_interpreter,
};
use grida_fx_runtime::host::{LanguageHost, label};
use grida_fx_runtime::tools::{resolve_tool, tool_name};
use indexmap::IndexMap;
use std::collections::{BTreeMap, HashSet};
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
        print_line(&python_line(
            host.member(Language::Python),
            &project,
            None,
            &env,
        ));
        print_line(&node_line(
            host.member(Language::TypeScript),
            &project,
            None,
            &env,
        ));
        let providers = Providers::of(&project, &env)?;
        providers.print();
        return Ok(u8::from(providers.refused));
    };
    let mut host = crate::print::planning_host(target, &cwd);
    let (project, workflow) = load_target(target, &cwd, &IndexMap::new(), &mut host)?;
    let home = super::schema::home_of(target, project.clone(), &workflow)?;
    print_line(&python_line(
        host.member(Language::Python),
        &home,
        Some(&project.root),
        &env,
    ));
    print_line(&node_line(
        host.member(Language::TypeScript),
        &home,
        Some(&project.root),
        &env,
    ));
    let providers = Providers::of(&project, &env)?;
    let mut route_defaults = home.route_defaults();
    route_defaults.extend(project.route_defaults());
    let mut registry = Registry::new(
        home.root.clone(),
        home.document.sources.clone(),
        read_lock(&home.root)?,
        route_defaults,
    );
    host.open_project(&home.root, &home.document.sources)?;
    // The target's routes and the keys first: they need no node host, so they print even when
    // the project's own node types cannot be resolved (no Python with grida, no @grida/fx yet).
    let mut used = UsedRoutes::new();
    used_routes(
        &workflow.workflow.steps,
        &mut registry,
        &mut host,
        &mut Vec::new(),
        &mut used,
    );
    let (uses, unservable) = uses_lines(&used, &providers.catalog, &providers.keys);
    for line in &uses {
        print_line(line);
    }
    providers.print();
    let mut missing = usize::from(providers.refused);
    let mut lines = Vec::new();
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
                    lines.push(format!("tool      {name:12} {shown}"));
                    missing += usize::from(found.is_none());
                }
            }
            Ok(Resolved::Workflow(_)) => {}
            Err(RegistryError::Problem(problem)) => {
                lines.push(format!("missing   {uses}: {problem}"));
                missing += 1;
            }
            Err(RegistryError::Fatal(error)) => return Err(error),
        }
    }
    for line in &lines {
        print_line(line);
    }
    Ok(u8::from(missing > 0 || unservable))
}

/// The capability a paid step calls and the route id chosen for it (`None`: no route), with the
/// number of steps (module doc, "The target's routes").
type UsedRoutes = BTreeMap<(String, Option<String>), usize>;

/// Counts into `used` the routes the paid steps of `steps` use, entering used workflows
/// (`within` holds the ones being visited). A node type that does not resolve, for whatever
/// reason, is skipped: the node-type lines after the keys report it.
fn used_routes(
    steps: &Steps,
    registry: &mut Registry,
    host: &mut dyn NodeHost,
    within: &mut Vec<String>,
    used: &mut UsedRoutes,
) {
    for step in all_steps(steps) {
        let Some(uses) = &step.uses else { continue };
        if step.at_plan {
            continue;
        }
        match registry.node_type(uses, host) {
            Ok(Resolved::Node(resolved)) => {
                let calls = resolved.spec.capability_calls(&IndexMap::new());
                for capability in calls.keys() {
                    count_route(used, &step, registry, capability, calls.len());
                }
            }
            Ok(Resolved::Workflow(loaded)) => {
                if within.contains(uses) {
                    continue;
                }
                within.push(uses.clone());
                used_routes(&loaded.workflow.steps, registry, host, within, used);
                within.pop();
            }
            Err(RegistryError::Problem(_) | RegistryError::Fatal(_)) => {}
        }
    }
}

/// Counts the route `step` uses for `capability`, chosen as planning chooses it: the step's
/// `route:` when its type calls one capability (of `calls`), else fx.yaml's default.
fn count_route(
    used: &mut UsedRoutes,
    step: &Step,
    registry: &Registry,
    capability: &str,
    calls: usize,
) {
    let declared = if calls == 1 {
        step.route.clone().filter(|route| !route.is_empty())
    } else {
        None
    };
    let chosen = declared.or_else(|| registry.route_default(capability).map(str::to_string));
    *used.entry((capability.to_string(), chosen)).or_default() += 1;
}

/// One `uses` line per route in `used` (module doc, "The target's routes"), and whether any of
/// them cannot be served.
fn uses_lines(used: &UsedRoutes, catalog: &RouteTable, keys: &Keys) -> (Vec<String>, bool) {
    let adapters = live::adapters(&live::offline_setup(keys.clone()));
    let mut unservable = false;
    let lines = used
        .iter()
        .map(|((capability, chosen), &count)| {
            let (what, servable) = match chosen {
                None => ("no route".to_string(), false),
                Some(id) => match catalog.resolve(capability, id) {
                    Ok(route) => {
                        let servable = live::servable(&adapters, keys, capability, &route.provider);
                        (
                            format!("{id} {}", servable_text(servable)),
                            servable == Servable::Yes,
                        )
                    }
                    Err(_) => (format!("{id} not in the catalog"), false),
                },
            };
            unservable |= !servable;
            let steps = if count == 1 { "step" } else { "steps" };
            labelled("uses", &format!("{capability} {what} ({count} {steps})"))
        })
        .collect();
    (lines, unservable)
}

/// How a `route` or `uses` line says whether a live run could serve a route.
fn servable_text(servable: Servable) -> String {
    match servable {
        Servable::Yes => "servable".to_string(),
        Servable::NoKey(key) => format!("no key ({})", key.variable()),
        Servable::NoAdapter => "no adapter".to_string(),
        Servable::NoProgram(program) => format!("not installed ({program})"),
    }
}

/// The provider lines of the planning project (module doc, "Providers").
#[derive(Debug, Clone)]
struct Providers {
    lines: Vec<String>,
    /// The `.env` or a base URL was refused: a live run would not start.
    refused: bool,
    /// The keys a live run would have, and the planning project's catalog.
    keys: Keys,
    catalog: RouteTable,
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
        Ok(Providers {
            lines,
            refused,
            keys,
            catalog,
        })
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
            let state = servable_text(live::servable(&adapters, keys, capability, &route.provider));
            labelled("route", &format!("{capability} {id} {state}"))
        })
        .collect()
}

/// The Python host line (module doc).
fn python_line(
    host: &mut LanguageHost,
    project: &Project,
    planning_root: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let interpreter = python_interpreter(&project.root, planning_root, env);
    // Named as the host's messages name it ([`label`]), relative to the planning project when
    // it lies there and outside the home.
    let shown = match interpreter.strip_prefix(&project.root) {
        Err(_) if planning_root.is_some_and(|root| interpreter.starts_with(root)) => {
            crate::print::shown_path(&interpreter, &project.root)
        }
        _ => label(&interpreter, &project.root),
    };
    let opened = host.open_project(&project.root, &project.document.sources);
    if opened.is_ok()
        && let Some(state) = host
            .probe()
            .and_then(|probe| unhostable(Language::Python, &probe))
    {
        return labelled("python", &format!("{shown} {state}"));
    }
    labelled(
        "python",
        &format!("{shown} {}", started(host, opened, "grida")),
    )
}

/// The Node host line (module doc). `planning_root`: the planning project, when it may be
/// another project than `project`, the home.
fn node_line(
    host: &mut LanguageHost,
    project: &Project,
    planning_root: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let opened = host.open_project(&project.root, &project.document.sources);
    // Named as the host's messages name it.
    let shown = host
        .program_label()
        .unwrap_or_else(|| label(&node_runtime(env), &project.root));
    let probe = if opened.is_ok() { host.probe() } else { None };
    if let Some(state) = probe
        .as_ref()
        .and_then(|probe| unhostable(Language::TypeScript, probe))
    {
        return labelled("node", &format!("{shown} {state}"));
    }
    // Only a project with a package.json starts the host, whose program may lie above it.
    let typescript = std::iter::once(project.root.as_path())
        .chain(planning_root)
        .any(|root| root.join("package.json").is_file());
    if !typescript {
        return labelled(
            "node",
            &match probe.as_ref().and_then(Probe::version_text) {
                Some(version) => format!("{shown} (Node {version})"),
                None => shown,
            },
        );
    }
    if opened.is_ok()
        && let Some(Err(sentence)) = host.host_program()
        && sentence == NO_SDK
    {
        return labelled("node", &format!("{shown} no @grida/fx"));
    }
    labelled(
        "node",
        &format!("{shown} {}", started(host, opened, "@grida/fx")),
    )
}

/// Starts the host of an opened project and asks it to describe nothing: `(<sdk> <version>)`
/// from its `initialize`, or `cannot start: <sentence>`.
fn started(host: &mut LanguageHost, opened: Result<(), HostFailure>, sdk: &str) -> String {
    let started = opened.and_then(|()| {
        host.describe(&DescribeParams {
            targets: Vec::new(),
            builtins: false,
        })
    });
    match (started, &host.info) {
        (Ok(_), Some(info)) => format!("({sdk} {})", info.host.sdk_version),
        (Err(HostFailure::Unavailable(sentence)), _) => format!("cannot start: {sentence}"),
        (Err(HostFailure::Rpc(error)), _) => format!("cannot start: {}", error.message),
        (Ok(_), None) => "NOT FOUND".to_string(),
    }
}

/// Why the program that answered `probe` cannot host `language`'s nodes, so it is not started:
/// `NOT FOUND`; `too old (<version>; FX needs 3.11+)` or `(…; FX needs 24+)`; `no grida`
/// (Python) or `not Node.js 24+` (Node); `None` when it may.
fn unhostable(language: Language, probe: &Probe) -> Option<String> {
    let (minimum, not_ready) = match language {
        Language::Python => {
            let (major, minor) = MINIMUM_PYTHON;
            (format!("{major}.{minor}+"), "no grida".to_string())
        }
        Language::TypeScript => (
            format!("{MINIMUM_NODE}+"),
            format!("not Node.js {MINIMUM_NODE}+"),
        ),
    };
    match probe {
        Probe::Missing => Some("NOT FOUND".to_string()),
        Probe::Answered { .. } if probe.is_too_old(language) => Some(format!(
            "too old ({}; FX needs {minimum})",
            probe.version_text().unwrap_or_default()
        )),
        Probe::Answered { ready: false, .. } => Some(not_ready),
        Probe::Answered { .. } | Probe::Unknown => None,
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
                "key       GG_API_KEY missing",
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
        // The Codex and Grok Build routes need no key; whether they are servable depends on this
        // machine.
        let (grok, lines_but_grok): (Vec<&String>, Vec<&String>) =
            lines.iter().partition(|line| line.contains("@grok "));
        let (codex, keyed): (Vec<&String>, Vec<&String>) = lines_but_grok
            .into_iter()
            .partition(|line| line.contains("@codex "));
        assert_eq!(grok.len(), 2, "image.edit and image.generate");
        for line in grok {
            assert!(
                line.ends_with("imagine@grok servable")
                    || line.ends_with("imagine@grok not installed (grok)"),
                "{line}"
            );
        }
        assert!(
            keyed.iter().all(|line| line.contains(" no key (")),
            "{lines:#?}"
        );
        assert_eq!(codex.len(), 2, "image.edit and image.generate");
        for line in codex {
            assert!(
                line.ends_with("imagegen@codex servable")
                    || line.ends_with("imagegen@codex not installed (codex)"),
                "{line}"
            );
        }
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
    fn uses_lines_count_steps_and_say_whether_a_live_run_could_serve_each_route() {
        let catalog = table(&[
            ("background.remove", "bg@fal"),
            ("image.generate", "gpt-image-2.5-sunburst@openai"),
        ]);
        let keys = Keys::from_pairs(&[(KeyName::Fal, "fx-made-up-fal-value-41d7")]);
        let used = |routes: &[(&str, Option<&str>, usize)]| -> UsedRoutes {
            routes
                .iter()
                .map(|(capability, route, count)| {
                    ((capability.to_string(), route.map(str::to_string)), *count)
                })
                .collect()
        };
        let (lines, unservable) = uses_lines(
            &used(&[
                ("background.remove", Some("bg@fal"), 2),
                ("image.generate", Some("gpt-image-2.5-sunburst@openai"), 1),
                ("image.generate", None, 3),
                ("image.generate", Some("nope@acme"), 1),
            ]),
            &catalog,
            &keys,
        );
        assert_eq!(
            lines,
            [
                "uses      background.remove bg@fal servable (2 steps)",
                "uses      image.generate no route (3 steps)",
                "uses      image.generate gpt-image-2.5-sunburst@openai no key (OPENAI_API_KEY) \
                 (1 step)",
                "uses      image.generate nope@acme not in the catalog (1 step)",
            ]
        );
        assert!(unservable);
        let (lines, unservable) = uses_lines(
            &used(&[("background.remove", Some("bg@fal"), 1)]),
            &catalog,
            &keys,
        );
        assert_eq!(
            lines,
            ["uses      background.remove bg@fal servable (1 step)"]
        );
        assert!(!unservable);
        assert_eq!(uses_lines(&used(&[]), &catalog, &keys), (Vec::new(), false));
    }

    #[test]
    fn an_interpreter_that_cannot_host_says_why() {
        let answered = |version, ready| Probe::Answered { version, ready };
        assert_eq!(
            unhostable(Language::Python, &answered((3, 9, 6), true)).as_deref(),
            Some("too old (3.9.6; FX needs 3.11+)")
        );
        // Too old wins over no grida: installing grida would not help.
        assert_eq!(
            unhostable(Language::Python, &answered((3, 9, 6), false)).as_deref(),
            Some("too old (3.9.6; FX needs 3.11+)")
        );
        assert_eq!(
            unhostable(Language::Python, &answered((3, 12, 4), false)).as_deref(),
            Some("no grida")
        );
        assert_eq!(
            unhostable(Language::Python, &Probe::Missing).as_deref(),
            Some("NOT FOUND")
        );
        assert_eq!(
            unhostable(Language::Python, &answered((3, 11, 0), true)),
            None
        );
        assert_eq!(unhostable(Language::Python, &Probe::Unknown), None);
    }

    #[test]
    fn a_node_that_cannot_host_says_why() {
        let answered = |version, ready| Probe::Answered { version, ready };
        assert_eq!(
            unhostable(Language::TypeScript, &answered((22, 12, 0), true)).as_deref(),
            Some("too old (22.12.0; FX needs 24+)")
        );
        assert_eq!(
            unhostable(Language::TypeScript, &answered((20, 1, 0), false)).as_deref(),
            Some("too old (20.1.0; FX needs 24+)")
        );
        assert_eq!(
            unhostable(Language::TypeScript, &answered((24, 3, 0), false)).as_deref(),
            Some("not Node.js 24+")
        );
        assert_eq!(
            unhostable(Language::TypeScript, &Probe::Missing).as_deref(),
            Some("NOT FOUND")
        );
        assert_eq!(
            unhostable(Language::TypeScript, &answered((24, 0, 0), true)),
            None
        );
        assert_eq!(unhostable(Language::TypeScript, &Probe::Unknown), None);
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
