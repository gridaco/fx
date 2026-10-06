//! The runtime the command starts when it plans or runs anything: a multi-thread tokio runtime
//! and the runtime's [`Engine`] (threads: the verb's own thread is the scheduler and never a
//! runtime thread; everything that waits runs as tasks on the runtime).
//!
//! - [`runtime`]: a multi-thread runtime with every driver enabled, its worker threads named
//!   [`WORKER_THREAD`].
//! - [`engine_for`] (the process environment) and [`engine_with_env`] (an environment given as a
//!   function, for tests): the engine of one planner:
//!   - **node hosts** start in the workflow's **home** project (its node modules, `fx.lock` and
//!     `sources` live there): `<python> -P -m grida.fx.host` with the interpreter
//!     `host::locate::python_interpreter` finds for the home (`GRIDA_FX_PYTHON`, else the home's
//!     `.venv`, else `python3`), named in messages relative to the home when it lies inside it;
//!     at most `HostPool::default_size()` of them;
//!   - the **store** is the **planning** project's cache (`planner.project.cache_dir()`), and runs
//!     go under the planning project's runs folder: the project found from where the command
//!     runs owns `runs/` and the cache (spec/store.md §8), also when the workflow comes from
//!     another project;
//!   - **provider adapters** only when `live` (`--live`): [`adapters_for`];
//!   - `live` as asked (`--live`).
//! - [`adapters_for`]: without `live`, an empty registry and nothing constructed, so every
//!   uncached paid call is refused (a recorded call still replays from the store). With `live`
//!   (spec/providers.md §1, §3):
//!   1. the allowlisted variables ([`provider_environment`]): the process environment, then the
//!      planning project's `.env` (`<planning project>/.env`) for the names the process does not
//!      set, unless the process sets `GRIDA_FX_DISABLE_DOTENV=1`;
//!   2. the keys of that environment, and the endpoints (the default bases, with each base-URL
//!      variable in its place);
//!   3. `grida_fx_providers::live::live_setup` over the default transport, or over the `Offline`
//!      transport when `GRIDA_FX_NETWORK=off` (every exchange is refused before it leaves);
//!   4. every provider's adapters on that setup (`live::adapters`), registered whether or not
//!      their key is present: a call without its key is refused before sending.
//!
//!   A `.env` or base-URL refusal is a usage error (exit 2) whose message names the variable or
//!   the `.env` line, never a value; building the adapters prints nothing.
//! - [`builtin_routes`]: the built-in default route table the planning verbs and `run` hand to
//!   planning (`PlanRequest::builtin_routes`; spec/identity.md §7).
//! - [`plan`]: `make_plan` with [`PlanTime`] as the plan-time runner and the store as the result
//!   cache, so `at: plan` steps run while planning and `cached` is true to the store. The
//!   planning verbs and `run` plan through it.
//! - [`shutdown`]: ends the node hosts an `at: plan` step left idle, politely (the runner shuts
//!   the pool down itself at the end of a run).
//!
//! [`plan`] and [`shutdown`] also mark when a run's runner takes interruptions
//! ([`crate::interrupt`]): from the end of planning until the engine is shut down.

use grida_fx_core::error::io_reason;
use grida_fx_core::host::{NodeHost, PlanTimeRunner};
use grida_fx_core::plan::{Plan, make_plan};
use grida_fx_core::project::Planner;
use grida_fx_core::routes::RouteTable;
use grida_fx_core::{Error, ErrorKind};
use grida_fx_providers::keys::Environment;
use grida_fx_providers::live::{self, Network};
use grida_fx_providers::{Adapters, Endpoints, Keys};
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::host;
use grida_fx_runtime::host::locate::python_interpreter;
use grida_fx_runtime::host::pool::HostPool;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::plantime::PlanTime;
use std::path::Path;
use std::sync::Arc;

/// The name of the runtime's worker threads.
pub const WORKER_THREAD: &str = "grida-fx-worker";

/// A multi-thread runtime for the command.
pub fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name(WORKER_THREAD)
        .build()
        .map_err(|error| {
            Error::new(
                ErrorKind::Internal,
                format!("cannot start the engine's runtime: {}", io_reason(&error)),
            )
        })
}

/// The engine of `planner` (module doc), reading the process environment.
pub fn engine_for(
    runtime: &tokio::runtime::Runtime,
    planner: &Planner,
    live: bool,
) -> Result<Arc<Engine>, Error> {
    engine_with_env(runtime, planner, live, &|name: &str| {
        std::env::var(name).ok()
    })
}

/// The engine of `planner` (module doc); `env` reads an environment variable.
pub fn engine_with_env(
    runtime: &tokio::runtime::Runtime,
    planner: &Planner,
    live: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<Engine>, Error> {
    // The transport is built inside the runtime, whatever it needs from it.
    let adapters = {
        let _entered = runtime.enter();
        adapters_for(planner, live, env)?
    };
    Ok(Arc::new(Engine::new(
        runtime.handle().clone(),
        host_spec(planner, env),
        &planner.project.cache_dir(),
        HostPool::default_size(),
        adapters,
        live,
    )))
}

/// The provider adapters of an invocation (module doc): none unless `live`. A key file or a base
/// URL that cannot be read is a usage error naming the variable or `.env` line, never a value.
pub fn adapters_for(
    planner: &Planner,
    live: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Adapters, Error> {
    if !live {
        return Ok(Adapters::new());
    }
    let (keys, endpoints) = provider_environment(&planner.project.root, env)?;
    let setup = live::live_setup(keys, endpoints, Network::from_env(env)).map_err(|reason| {
        Error::new(
            ErrorKind::Internal,
            format!("cannot start the provider transport: {reason}"),
        )
    })?;
    Ok(live::adapters(&setup))
}

/// The keys and endpoints of the planning project at `project_root` (spec/providers.md §3): the
/// allowlisted variables from `env`, then from `<project_root>/.env` (unless `env` gives
/// `GRIDA_FX_DISABLE_DOTENV=1`). A `.env` or base-URL refusal is a usage error; its sentence
/// names the variable or the line, never a value.
pub fn provider_environment(
    project_root: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Keys, Endpoints), Error> {
    let environment =
        Environment::read(env, Some(&dotenv_path(project_root))).map_err(Error::usage)?;
    let keys = Keys::from_environment(&environment);
    let endpoints = Endpoints::from_environment(&environment, &keys).map_err(Error::usage)?;
    Ok((keys, endpoints))
}

/// The planning project's key file.
pub fn dotenv_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(".env")
}

/// The built-in default route table (spec/identity.md §7), embedded in the binary.
pub fn builtin_routes() -> Result<RouteTable, Error> {
    grida_fx_providers::routes::default_table()
        .map_err(|message| Error::new(ErrorKind::Internal, message))
}

/// How node hosts start for `planner`: in the home project, with the interpreter `locate` finds
/// there. `env` reads an environment variable.
fn host_spec(planner: &Planner, env: &dyn Fn(&str) -> Option<String>) -> HostSpec {
    let home = &planner.home;
    let python = python_interpreter(&home.root, env);
    HostSpec {
        label: host::label(&python, &home.root),
        python,
        project_root: home.root.clone(),
        sources: home.document.sources.clone(),
    }
}

/// Makes the plan with the engine's plan-time runner and its store as the result cache.
pub fn plan(
    engine: &Arc<Engine>,
    planner: &mut Planner,
    host: &mut dyn NodeHost,
) -> Result<Plan, Error> {
    let mut plan_time = PlanTime::new(Arc::clone(engine));
    let runner: &mut dyn PlanTimeRunner = &mut plan_time;
    let plan = make_plan(planner, host, Some(runner), &*engine.store);
    crate::interrupt::planning_ended();
    plan
}

/// Ends the idle node hosts of `engine` (from `at: plan` steps). Call it from the verb's own
/// thread, never from a runtime thread.
pub fn shutdown(runtime: &tokio::runtime::Runtime, engine: &Engine) {
    crate::interrupt::runner_ended();
    runtime.block_on(engine.hosts.shutdown());
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::host::FakeHost;
    use grida_fx_core::project::{PlanRequest, make_planner};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_runtime_runs_tasks_on_named_workers() {
        let runtime = runtime().unwrap();
        let name = runtime.block_on(async {
            tokio::spawn(async { std::thread::current().name().map(str::to_string) })
                .await
                .unwrap()
        });
        assert_eq!(name.as_deref(), Some(WORKER_THREAD));
    }

    /// A planner over a project at `root` whose workflow uses a built-in only.
    fn planner_in(root: &Path, project_file: &str) -> Planner {
        std::fs::write(root.join("fx.yaml"), project_file).unwrap();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(
            root.join("workflows/case.yaml"),
            "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  draw:\n    uses: \
             fx/image.generate@1\n    with: { prompt: a kite }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("routes.yaml"),
            "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, \
             price: { low_usd: 0.01, high_usd: 0.04 } }\n",
        )
        .unwrap();
        let request = PlanRequest {
            target: "case".into(),
            cwd: root.to_path_buf(),
            routes: vec!["routes.yaml".into()],
            ..PlanRequest::default()
        };
        make_planner(&request, &mut FakeHost::new()).unwrap()
    }

    /// The environment of a live test: the network off (the `Offline` transport, never the
    /// default one), no `.env`, and `extra`.
    fn offline_env(extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let mut values: Vec<(String, String)> = vec![
            ("GRIDA_FX_NETWORK".into(), "off".into()),
            ("GRIDA_FX_DISABLE_DOTENV".into(), "1".into()),
        ];
        values.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        move |name: &str| {
            values
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn the_engine_keeps_the_planning_projects_cache_and_the_homes_hosts() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = planner_in(
            &root,
            "fx: project/v1\ncache: shared/cache\nsources: [lib]\nroutes:\n  image.generate: \
             img-a@acme\n",
        );
        let runtime = runtime().unwrap();
        let engine = engine_with_env(&runtime, &planner, true, &offline_env(&[])).unwrap();
        assert_eq!(engine.store.root(), root.join("shared/cache"));
        assert_eq!(engine.project_root, root);
        assert_eq!(engine.sources, ["lib"]);
        assert!(engine.live);
        // Live: every provider's adapters, whether or not a key is present.
        let table = builtin_routes().unwrap();
        for ((capability, id), route) in &table.entries {
            assert!(
                engine.adapters.serves(capability, &route.provider),
                "no adapter serves {id} for {capability}"
            );
        }
        assert!(!engine.adapters.serves("image.generate", "acme"));
        let spec = engine.hosts.spec();
        assert_eq!(spec.project_root, root);
        assert_eq!(spec.sources, ["lib"]);
        let engine = engine_for(&runtime, &planner, false).unwrap();
        assert!(!engine.live);
        assert!(engine.adapters.is_empty());
    }

    #[test]
    fn without_live_nothing_is_read_or_constructed() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = planner_in(&root, "fx: project/v1\n");
        // A key file that would be refused, and an environment that must not be asked.
        std::fs::write(root.join(".env"), "FAL_KEY=\"unterminated\n").unwrap();
        let asked = std::cell::Cell::new(0usize);
        let counting = |_: &str| {
            asked.set(asked.get() + 1);
            None
        };
        let adapters = adapters_for(&planner, false, &counting).unwrap();
        assert!(adapters.is_empty());
        assert_eq!(asked.get(), 0);
    }

    #[test]
    fn a_live_engine_reads_the_planning_projects_key_file() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = planner_in(&root, "fx: project/v1\n");
        let value = "fx-made-up-dotenv-value-7c1e";
        std::fs::write(
            root.join(".env"),
            format!("FAL_KEY={value}\nFAL_KEY={value}\n"),
        )
        .unwrap();
        let network_off = |name: &str| (name == "GRIDA_FX_NETWORK").then(|| "off".to_string());
        // The file is read: a refusal is a usage error naming the variable, never the value.
        let error = adapters_for(&planner, true, &network_off).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Usage);
        assert!(error.message.contains("FAL_KEY"), "{}", error.message);
        assert!(!error.message.contains(value), "{}", error.message);
        // GRIDA_FX_DISABLE_DOTENV=1 turns the file off.
        let adapters = adapters_for(&planner, true, &offline_env(&[])).unwrap();
        assert!(adapters.serves("video.generate", "fal"));
        // A good file gives its keys.
        std::fs::write(root.join(".env"), format!("FAL_KEY={value}\n")).unwrap();
        let (keys, _) = provider_environment(&root, &network_off).unwrap();
        assert_eq!(
            keys.source(grida_fx_providers::KeyName::Fal),
            Some(grida_fx_providers::keys::KeySource::DotEnv)
        );
        assert!(!format!("{keys:?}").contains(value));
    }

    #[test]
    fn a_base_url_holding_a_key_is_a_usage_error() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = planner_in(&root, "fx: project/v1\n");
        let value = "fx-made-up-key-0b9d";
        let env = offline_env(&[
            ("OPENAI_API_KEY", value),
            (
                "OPENAI_BASE_URL",
                "https://proxy.example.test/fx-made-up-key-0b9d",
            ),
        ]);
        let error = adapters_for(&planner, true, &env).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Usage);
        assert!(
            error.message.contains("OPENAI_BASE_URL"),
            "{}",
            error.message
        );
        assert!(!error.message.contains(value), "{}", error.message);
        let env = offline_env(&[("OPENAI_BASE_URL", "ftp://proxy.example.test")]);
        let error = adapters_for(&planner, true, &env).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Usage);
        assert!(
            error.message.starts_with("OPENAI_BASE_URL"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_host_interpreter_follows_the_variable_then_the_homes_venv() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = planner_in(&root, "fx: project/v1\n");
        let chosen = |name: &str| (name == "GRIDA_FX_PYTHON").then(|| "/opt/py/python".to_string());
        let spec = host_spec(&planner, &chosen);
        assert_eq!(spec.python, PathBuf::from("/opt/py/python"));
        assert_eq!(spec.label, "/opt/py/python");
        let venv = root.join(".venv/bin/python");
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::fs::write(&venv, "").unwrap();
        let spec = host_spec(&planner, &|_| None);
        if cfg!(windows) {
            assert_eq!(spec.label, "python3");
        } else {
            assert_eq!(spec.python, venv);
            assert_eq!(spec.label, ".venv/bin/python");
        }
    }

    #[test]
    fn a_plan_without_at_plan_steps_needs_no_plan_time_run() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let mut planner = planner_in(
            &root,
            "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n",
        );
        let runtime = runtime().unwrap();
        let engine = engine_for(&runtime, &planner, false).unwrap();
        let plan = plan(&engine, &mut planner, &mut FakeHost::new()).unwrap();
        assert!(plan.ok(), "{:?}", plan.problems);
        assert_eq!(plan.expansion.instances.len(), 1);
        shutdown(&runtime, &engine);
    }
}
