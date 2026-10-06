//! The runtime the command starts when it plans or runs anything: a multi-thread tokio runtime
//! and the runtime's [`Engine`] (threads: the verb's own thread is the scheduler and never a
//! runtime thread; everything that waits runs as tasks on the runtime).
//!
//! - [`runtime`]: a multi-thread runtime with every driver enabled, its worker threads named
//!   [`WORKER_THREAD`].
//! - [`engine_for`]: the engine of one planner:
//!   - **node hosts** start in the workflow's **home** project (its node modules, `fx.lock` and
//!     `sources` live there): `<python> -P -m grida.fx.host` with the interpreter
//!     `host::locate::python_interpreter` finds for the home (`GRIDA_FX_PYTHON`, else the home's
//!     `.venv`, else `python3`), named in messages relative to the home when it lies inside it;
//!     at most `HostPool::default_size()` of them;
//!   - the **store** is the **planning** project's cache (`planner.project.cache_dir()`), and runs
//!     go under the planning project's runs folder: the project found from where the command
//!     runs owns `runs/` and the cache (spec/store.md §8), also when the workflow comes from
//!     another project;
//!   - **no provider adapters**: none exist in this version, so every uncached paid call is
//!     refused (a recorded call still replays from the store);
//!   - `live` as asked (`--live`).
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
use grida_fx_core::{Error, ErrorKind};
use grida_fx_providers::Adapters;
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::host;
use grida_fx_runtime::host::locate::python_interpreter;
use grida_fx_runtime::host::pool::HostPool;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::plantime::PlanTime;
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

/// The engine of `planner` (module doc).
pub fn engine_for(
    runtime: &tokio::runtime::Runtime,
    planner: &Planner,
    live: bool,
) -> Result<Arc<Engine>, Error> {
    let env = |name: &str| std::env::var(name).ok();
    Ok(Arc::new(Engine::new(
        runtime.handle().clone(),
        host_spec(planner, &env),
        &planner.project.cache_dir(),
        HostPool::default_size(),
        Adapters::new(),
        live,
    )))
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
        let engine = engine_for(&runtime, &planner, true).unwrap();
        assert_eq!(engine.store.root(), root.join("shared/cache"));
        assert_eq!(engine.project_root, root);
        assert_eq!(engine.sources, ["lib"]);
        assert!(engine.live);
        assert!(engine.adapters.is_empty());
        let spec = engine.hosts.spec();
        assert_eq!(spec.project_root, root);
        assert_eq!(spec.sources, ["lib"]);
        let engine = engine_for(&runtime, &planner, false).unwrap();
        assert!(!engine.live);
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
