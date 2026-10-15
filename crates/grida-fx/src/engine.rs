//! The runtime the command starts when it plans or runs anything: a multi-thread tokio runtime
//! and the runtime's [`Engine`] (threads: the verb's own thread is the scheduler and never a
//! runtime thread; everything that waits runs as tasks on the runtime).
//!
//! - [`runtime`]: a multi-thread runtime with every driver enabled, its worker threads named
//!   [`WORKER_THREAD`].
//! - [`engine_for`] (the process environment) and [`engine_with_env`] (an environment given as a
//!   function, for tests): the engine of one planner:
//!   - **node hosts** start in the workflow's **home** project (its node modules, `fx.lock` and
//!     `sources` live there), one pool per language, each of at most `HostPool::default_size()`
//!     hosts, started as `HostSpec::locate` finds them from the home, else from the planning
//!     project (spec/protocol.md §1, "Starting a host"); without a TypeScript host program,
//!     every TypeScript body fails with its sentence;
//!   - the **store** is the **planning** project's cache (`planner.project.cache_dir()`), and runs
//!     go under the planning project's runs folder: the project found from where the command
//!     runs owns `runs/` and the cache (spec/store.md §8), also when the workflow comes from
//!     another project. A stand-in run's store is the stand-in store under it,
//!     `<cache>/stand-in` ([`stand_in_store`]), and the engine asks its stand-in;
//!   - **provider adapters** only when `live` (`--live`): [`adapters_for`];
//!   - `live` as asked (`--live`);
//!   - **private roots** the engine scrubs from what it records besides the home and the store
//!     (`Engine::with_private_roots`): the planning project, and the folders the verb names (its
//!     working directory, a stand-in file's folder); and, labelled `@grida/fx`
//!     (`Engine::with_labelled_root`), the folder of each `@grida/fx` outside the home whose host
//!     program a TypeScript host of the run would start (`host::locate::host_package`): the
//!     engine's, and a stand-in host's, found from the planning project.
//! - [`adapters_for`]: without `live`, an empty registry and nothing constructed, so every
//!   uncached paid call is refused (a recorded call still replays from the store). With `live`
//!   (spec/adapters.md §1; spec/providers.md §1):
//!   1. the variables the planning project's provider declarations name, FX's own and its
//!      `providers:` ([`provider_variables`]): the process environment, then the planning
//!      project's `.env` (`<planning project>/.env`) for the names the process does not set,
//!      unless the process sets `GRIDA_FX_DISABLE_DOTENV=1`;
//!   2. the keys of that environment, and the endpoints (the default bases, with each base-URL
//!      variable in its place);
//!   3. `grida_fx_providers::live::live_setup` over the default transport, or over the `Offline`
//!      transport when `GRIDA_FX_NETWORK=off` (every exchange is refused before it leaves);
//!   4. every provider's adapters on that setup (`live::adapters`), a family's for each provider
//!      the planning project declares with one included, registered whether or not their key is
//!      present: a call without its key is refused before sending;
//!   5. the planning project's own adapters, which planning loaded
//!      (`grida_fx_runtime::adapter_host::register`): for each declaration with one, an adapter
//!      host, the node host program of the module's language found from the planning project
//!      (`HostSpec::locate`, its sources the planning project's), started on its first call with
//!      the network off too, its environment FX's without every declared variable but its own
//!      declaration's, which it gets as step 1 read them, from the environment or `.env`
//!      (spec/adapters.md §9). Nothing starts while the adapters are built.
//!
//!   A `.env` or base-URL refusal is a usage error (exit 2) whose message names the variable or
//!   the `.env` line, never a value; building the adapters prints nothing.
//! - [`builtin_routes`]: the built-in default route table the planning verbs and `run` hand to
//!   planning (`PlanRequest::builtin_routes`; spec/identity.md §7).
//! - [`plan`]: `make_plan` with [`PlanTime`] as the plan-time runner and the store as the result
//!   cache, so `at: plan` steps run while planning and `cached` is true to the store. The
//!   planning verbs and `run` plan through it.
//! - [`shutdown`]: ends the node hosts an `at: plan` step left idle, politely (the runner shuts
//!   the pool down itself at the end of a run), and a stand-in run's answerer.
//!
//! [`plan`] and [`shutdown`] also mark when a run's runner takes interruptions
//! ([`crate::interrupt`]): from the end of planning until the engine is shut down.

use grida_fx_core::docs::project::Project;
use grida_fx_core::error::io_reason;
use grida_fx_core::host::Language;
use grida_fx_core::host::{NodeHost, PlanTimeRunner};
use grida_fx_core::plan::{Plan, make_plan};
use grida_fx_core::project::Planner;
use grida_fx_core::providers::Declaration;
use grida_fx_core::routes::RouteTable;
use grida_fx_core::{Error, ErrorKind};
use grida_fx_providers::keys::{Environment, Secret};
use grida_fx_providers::live::{self, Network};
use grida_fx_providers::{Adapters, Endpoints, Keys};
use grida_fx_runtime::adapter_host::{self, Launch, ProcessLaunch};
use grida_fx_runtime::engine::Engine;
use grida_fx_runtime::host::locate::{self, node_host_entry};
use grida_fx_runtime::host::pool::HostPool;
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::plantime::PlanTime;
use grida_fx_runtime::stand_in::StandIn;
use std::path::{Path, PathBuf};
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

/// The engine of `planner` (module doc), reading the process environment. With `stand_in`, a
/// stand-in run's engine; `private` names further folders whose paths it scrubs (module doc).
pub fn engine_for(
    runtime: &tokio::runtime::Runtime,
    planner: &Planner,
    live: bool,
    stand_in: Option<Arc<StandIn>>,
    private: &[PathBuf],
) -> Result<Arc<Engine>, Error> {
    engine_with_env(runtime, planner, live, stand_in, private, &|name: &str| {
        std::env::var(name).ok()
    })
}

/// The engine of `planner` (module doc); `env` reads an environment variable.
pub fn engine_with_env(
    runtime: &tokio::runtime::Runtime,
    planner: &Planner,
    live: bool,
    stand_in: Option<Arc<StandIn>>,
    private: &[PathBuf],
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<Engine>, Error> {
    // The transport is built inside the runtime, whatever it needs from it.
    let adapters = {
        let _entered = runtime.enter();
        adapters_for(planner, live, env)?
    };
    let store = match &stand_in {
        Some(_) => stand_in_store(planner),
        None => planner.project.cache_dir(),
    };
    // Whatever this process writes to a store marks the planning project among its users
    // (spec/store.md §6).
    grida_fx_runtime::store::lease::set_user(&planner.project.root, &planner.project.cache_dir());
    let home = &planner.home;
    let typescript = home_spec(Language::TypeScript, planner, env);
    // Node's messages may name the files of a @grida/fx outside the home: the engine's host
    // program's, or a stand-in host's (found from the planning project alone).
    let packages: Vec<PathBuf> = [
        typescript
            .as_ref()
            .ok()
            .and_then(|spec| spec.host_package(&home.root)),
        node_host_entry(&planner.project.root, None, env)
            .ok()
            .and_then(|entry| locate::host_package(&entry, &home.root)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut engine = Engine::new(
        runtime.handle().clone(),
        home_spec(Language::Python, planner, env)
            .map_err(|message| Error::new(ErrorKind::Internal, message))?,
        &store,
        HostPool::default_size(),
        adapters,
        live,
    )
    .with_host(Language::TypeScript, typescript)
    .with_private_roots(
        std::iter::once(planner.project.root.clone()).chain(private.iter().cloned()),
    );
    for package in packages {
        engine = engine.with_labelled_root(package, Language::TypeScript.sdk());
    }
    Ok(Arc::new(match stand_in {
        Some(stand_in) => engine.with_stand_in(stand_in),
        None => engine,
    }))
}

/// The stand-in store of the planning project (spec/store.md §1, §8 "Stand-in runs").
pub fn stand_in_store(planner: &Planner) -> std::path::PathBuf {
    planner.project.cache_dir().join(STAND_IN_STORE)
}

/// The stand-in store's folder under the store root.
pub const STAND_IN_STORE: &str = "stand-in";

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
    let (environment, keys, endpoints) = provider_variables(&planner.project, env)?;
    let setup = live::live_setup(keys, endpoints, Network::from_env(env)).map_err(|reason| {
        Error::new(
            ErrorKind::Internal,
            format!("cannot start the provider transport: {reason}"),
        )
    })?;
    let mut adapters = live::adapters(&setup);
    let project = &planner.project;
    adapter_host::register(&mut adapters, &setup, |declaration, adapter| {
        let (withheld, given) = adapter_environment(declaration, &environment);
        let spec = Language::of_module(&adapter.path).and_then(|language| {
            HostSpec::locate(
                language,
                &project.root,
                None,
                project.document.sources.clone(),
                env,
            )
        });
        let spec = spec.map(|spec| spec.with_environment(withheld, given));
        Arc::new(ProcessLaunch::new(spec)) as Arc<dyn Launch>
    });
    Ok(adapters)
}

/// The environment of `declaration`'s adapter host (module doc, step 5): every variable the
/// declarations of `environment` read but its own, withheld; its own, as `environment` holds
/// them, given.
fn adapter_environment(
    declaration: &Declaration,
    environment: &Environment,
) -> (Vec<String>, Vec<(String, Secret)>) {
    let own: Vec<&str> = declaration.variables().collect();
    let withheld = environment
        .declarations()
        .variables()
        .into_iter()
        .filter(|name| !own.contains(&name.as_str()))
        .collect();
    let given = own
        .iter()
        .filter_map(|name| Some((name.to_string(), Secret::new(environment.get(name)?))))
        .collect();
    (withheld, given)
}

/// The variables, keys and endpoints of the planning project `project` (spec/providers.md §1):
/// the variables its declarations name from `env`, then from its `.env` (unless `env` gives
/// `GRIDA_FX_DISABLE_DOTENV=1`). A `.env` or base-URL refusal is a usage error; its sentence
/// names the variable or the line, never a value.
pub fn provider_variables(
    project: &Project,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Environment, Keys, Endpoints), Error> {
    let dotenv = dotenv_path(&project.root);
    let environment =
        Environment::read(env, Some(&dotenv), &project.declarations()).map_err(Error::usage)?;
    let keys = Keys::from_environment(&environment);
    let endpoints = Endpoints::from_environment(&environment, &keys).map_err(Error::usage)?;
    Ok((environment, keys, endpoints))
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

/// How `language`'s node hosts start for `planner`: in the home project, as
/// [`HostSpec::locate`] finds them from the home, else from the planning project; `Err` is why
/// there is no TypeScript host program. `env` reads an environment variable.
fn home_spec(
    language: Language,
    planner: &Planner,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<HostSpec, String> {
    let home = &planner.home;
    let sources = home.document.sources.clone();
    HostSpec::locate(
        language,
        &home.root,
        Some(&planner.project.root),
        sources,
        env,
    )
}

/// Makes the plan with the engine's plan-time runner and its store as the result cache.
pub fn plan(
    engine: &Arc<Engine>,
    planner: &mut Planner,
    host: &mut dyn NodeHost,
) -> Result<Plan, Error> {
    let mut plan_time = PlanTime::new(Arc::clone(engine));
    let runner: &mut dyn PlanTimeRunner = &mut plan_time;
    make_plan(planner, host, Some(runner), &*engine.store)
}

/// Ends the idle node hosts of `engine` (from `at: plan` steps), and its stand-in's answerer.
/// Call it from the verb's own thread, never from a runtime thread.
pub fn shutdown(runtime: &tokio::runtime::Runtime, engine: &Engine) {
    crate::interrupt::runner_ended();
    runtime.block_on(engine.hosts.shutdown());
    if let Some(stand_in) = &engine.stand_in {
        runtime.block_on(stand_in.shutdown());
    }
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
        let engine =
            engine_with_env(&runtime, &planner, true, None, &[], &offline_env(&[])).unwrap();
        assert_eq!(engine.store.root(), root.join("shared/cache"));
        assert_eq!(engine.project_root, root);
        assert_eq!(engine.sources, ["lib"]);
        assert!(engine.live);
        // Live: every provider's adapters, whether or not a key is present.
        let table = builtin_routes().unwrap();
        for ((capability, id), route) in &table.entries {
            let route = grida_fx_providers::RouteRef::of(route);
            assert!(
                engine.adapters.serving(&route).is_some(),
                "no adapter serves {id} for {capability}"
            );
        }
        assert!(!engine.adapters.serves("image.generate", "acme"));
        let spec = engine.hosts.pool(Language::Python).unwrap().spec();
        assert_eq!(spec.project_root, root);
        assert_eq!(spec.sources, ["lib"]);
        let engine = engine_for(&runtime, &planner, false, None, &[]).unwrap();
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

    /// A planner over a project at `root` that declares `acme` with `adapters/acme.py#acme`,
    /// which the planning host loads as serving `image.generate`, and `other`, declared with a
    /// key of its own.
    fn adapter_planner(root: &Path) -> Planner {
        let project_file = "fx: project/v1\nproviders:\n  acme:\n    credential: { variable: \
                            ACME_API_KEY }\n    base_url: { variable: ACME_BASE_URL, default: \
                            \"https://api.acme.test/v1\" }\n    adapter: adapters/acme.py#acme\n  \
                            other:\n    credential: { variable: OTHER_API_KEY }\n    base_url: { \
                            default: \"https://api.other.test\" }\n";
        std::fs::create_dir_all(root.join("adapters")).unwrap();
        std::fs::write(root.join("adapters/acme.py"), "acme = adapter()\n").unwrap();
        let _ = planner_in(root, "fx: project/v1\n");
        std::fs::write(root.join("fx.yaml"), project_file).unwrap();
        let mut host = FakeHost::new().with_adapter(
            "adapters/acme.py#acme",
            Ok(grida_fx_protocol::AdapterLoadResult {
                adapter: grida_fx_protocol::AdapterDeclaration {
                    version: Some(1),
                    capabilities: [(
                        "image.generate".to_string(),
                        grida_fx_protocol::CallKind::Request,
                    )]
                    .into_iter()
                    .collect(),
                },
                closure: vec![grida_fx_protocol::ClosureEntry {
                    label: "adapters/acme.py".into(),
                    path: root.join("adapters/acme.py").to_string_lossy().into_owned(),
                }],
            }),
        );
        let request = PlanRequest {
            target: "case".into(),
            cwd: root.to_path_buf(),
            routes: vec!["routes.yaml".into()],
            ..PlanRequest::default()
        };
        make_planner(&request, &mut host).unwrap()
    }

    #[test]
    fn a_live_engine_registers_the_planning_projects_own_adapters() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = adapter_planner(&root);
        let route = planner
            .routes
            .resolve("image.generate", "img-a@acme")
            .unwrap();
        assert_eq!(
            route.contract,
            serde_json::json!({"adapter": "adapters/acme.py#acme", "adapter_behavior": 1})
        );
        let runtime = runtime().unwrap();
        let _entered = runtime.enter();
        let plain = adapters_for(
            &planner_in(&root, "fx: project/v1\n"),
            true,
            &offline_env(&[]),
        )
        .unwrap();
        let adapters = adapters_for(&adapter_planner(&root), true, &offline_env(&[])).unwrap();
        let served = adapters
            .serving(&grida_fx_providers::RouteRef::of(route))
            .unwrap();
        let identity = served.identity().unwrap();
        assert_eq!(
            (identity.adapter, identity.behavior),
            ("adapters/acme.py#acme", "1")
        );
        assert!(!adapters.serves("video.generate", "acme"));
        // Its host is kept with the run's runtimes; nothing has started.
        assert_eq!(adapters.runtimes().len(), plain.runtimes().len() + 1);
        assert!(runtime.block_on(adapters.shutdown()));
    }

    #[test]
    fn an_adapter_host_gets_its_own_declarations_variables_and_no_other() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = adapter_planner(&root);
        std::fs::write(
            root.join(".env"),
            "ACME_API_KEY=acme-made-up-key\nOTHER_API_KEY=other-made-up-key\n",
        )
        .unwrap();
        let env = |name: &str| {
            (name == "ACME_BASE_URL").then(|| "https://proxy.acme.test/v1".to_string())
        };
        let (environment, _, endpoints) = provider_variables(&planner.project, &env).unwrap();
        assert_eq!(endpoints.base("acme"), Some("https://proxy.acme.test/v1"));
        let declarations = planner.project.declarations();
        let (withheld, given) =
            adapter_environment(declarations.get("acme").unwrap(), &environment);
        assert!(withheld.iter().any(|name| name == "OTHER_API_KEY"));
        assert!(withheld.iter().any(|name| name == "OPENAI_API_KEY"));
        assert!(withheld.iter().any(|name| name == "GG_BASE_URL"));
        assert!(!withheld.iter().any(|name| name.starts_with("ACME_")));
        let given: Vec<(&str, &str)> = given
            .iter()
            .map(|(name, value)| (name.as_str(), value.expose()))
            .collect();
        assert_eq!(
            given,
            [
                ("ACME_API_KEY", "acme-made-up-key"),
                ("ACME_BASE_URL", "https://proxy.acme.test/v1")
            ]
        );
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
        let (_, keys, _) = provider_variables(&planner.project, &network_off).unwrap();
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
        let spec = home_spec(Language::Python, &planner, &chosen).unwrap();
        assert_eq!(spec.program, PathBuf::from("/opt/py/python"));
        assert_eq!(spec.label, "/opt/py/python");
        let venv = root.join(".venv/bin/python");
        std::fs::create_dir_all(venv.parent().unwrap()).unwrap();
        std::fs::write(&venv, "").unwrap();
        let spec = home_spec(Language::Python, &planner, &|_| None).unwrap();
        if cfg!(windows) {
            assert_eq!(spec.label, "python3");
        } else {
            assert_eq!(spec.program, venv);
            assert_eq!(spec.label, ".venv/bin/python");
        }
    }

    #[test]
    fn the_typescript_host_is_node_with_the_homes_sdk() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        let planner = planner_in(
            &root,
            "fx: project/v1\nsources: [acme_lib, \"@acme/shapes\"]\n",
        );
        let nothing = |_: &str| None;
        // No @grida/fx: every TypeScript body would fail with the sentence.
        assert_eq!(
            home_spec(Language::TypeScript, &planner, &nothing),
            Err(
                "no @grida/fx for the TypeScript node host: run npm install @grida/fx in the \
                 project"
                    .to_string()
            )
        );
        let runtime = runtime().unwrap();
        let engine = engine_with_env(&runtime, &planner, false, None, &[], &nothing).unwrap();
        assert_eq!(
            engine.hosts.pool(Language::TypeScript).err(),
            Some(
                "no @grida/fx for the TypeScript node host: run npm install @grida/fx in the \
                 project"
            )
        );
        assert!(engine.hosts.pool(Language::Python).is_ok());
        // The SDK that started the engine.
        let sdk = root.join("sdk/dist/host-entry.js");
        std::fs::create_dir_all(sdk.parent().unwrap()).unwrap();
        std::fs::write(&sdk, "").unwrap();
        let named = sdk.to_string_lossy().into_owned();
        let started = move |name: &str| match name {
            "GRIDA_FX_SDK_NODE_HOST" => Some(named.clone()),
            "GRIDA_FX_SDK_NODE" => Some("/opt/node/bin/node".to_string()),
            _ => None,
        };
        let spec = home_spec(Language::TypeScript, &planner, &started).unwrap();
        assert_eq!(spec.language, Language::TypeScript);
        assert_eq!(spec.program, PathBuf::from("/opt/node/bin/node"));
        assert_eq!(spec.label, "/opt/node/bin/node");
        assert_eq!(spec.args, [sdk.clone().into_os_string()]);
        assert_eq!(spec.project_root, root);
        assert!(spec.check_version);
        // The home's own @grida/fx comes first; GRIDA_FX_NODE before the SDK's Node.
        let package = root.join("node_modules/@grida/fx");
        std::fs::create_dir_all(package.join("dist")).unwrap();
        std::fs::write(package.join("package.json"), "{}").unwrap();
        std::fs::write(package.join("dist/host-entry.js"), "").unwrap();
        let named = sdk.to_string_lossy().into_owned();
        let chosen = move |name: &str| match name {
            "GRIDA_FX_SDK_NODE_HOST" => Some(named.clone()),
            "GRIDA_FX_NODE" => Some("node24".to_string()),
            _ => None,
        };
        let spec = home_spec(Language::TypeScript, &planner, &chosen).unwrap();
        assert_eq!(spec.program, PathBuf::from("node24"));
        assert_eq!(
            spec.args,
            [package.join("dist/host-entry.js").into_os_string()]
        );
        let engine = engine_with_env(&runtime, &planner, false, None, &[], &chosen).unwrap();
        let pool = engine.hosts.pool(Language::TypeScript).unwrap();
        assert_eq!(pool.spec(), &spec);
        assert_eq!(pool.spec().sources, ["acme_lib", "@acme/shapes"]);
    }

    #[test]
    fn a_home_outside_the_planning_project_falls_back_to_its_sdk_and_hides_it() {
        // `app` plans; its workflows: folder lies in `shared`, the workflow's home.
        let folder = tempfile::tempdir().unwrap();
        let base = folder.path().canonicalize().unwrap();
        let app = base.join("app");
        let shared = base.join("shared");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        planner_in(&shared, "fx: project/v1\n");
        std::fs::write(
            app.join("fx.yaml"),
            "fx: project/v1\nworkflows: [../shared/workflows]\n",
        )
        .unwrap();
        std::fs::copy(shared.join("routes.yaml"), app.join("routes.yaml")).unwrap();
        let request = PlanRequest {
            target: "case".into(),
            cwd: app.clone(),
            routes: vec!["routes.yaml".into()],
            ..PlanRequest::default()
        };
        let planner = make_planner(&request, &mut FakeHost::new()).unwrap();
        assert_eq!(planner.home.root, shared);
        assert_eq!(planner.project.root, app);
        let runtime = runtime().unwrap();
        let shown = |engine: &Engine, package: &Path| {
            engine.scrub(&format!(
                "not defined by \"exports\" in {}/package.json imported from {}/dist/host.js",
                package.display(),
                package.display()
            ))
        };
        let hidden = "not defined by \"exports\" in @grida/fx/package.json imported from @grida/fx/dist/host.js";
        // The SDK that started the engine, outside both projects: its folder is labelled.
        let global = base.join("global/node_modules/@grida/fx");
        std::fs::create_dir_all(global.join("dist")).unwrap();
        std::fs::write(global.join("dist/host-entry.js"), "").unwrap();
        let named = global
            .join("dist/host-entry.js")
            .to_string_lossy()
            .into_owned();
        let started = move |name: &str| (name == "GRIDA_FX_SDK_NODE_HOST").then(|| named.clone());
        let spec = home_spec(Language::TypeScript, &planner, &started).unwrap();
        assert_eq!(
            spec.args,
            [global.join("dist/host-entry.js").into_os_string()]
        );
        let engine = engine_with_env(&runtime, &planner, false, None, &[], &started).unwrap();
        assert_eq!(shown(&engine, &global), hidden);
        // The planning project's own @grida/fx comes before it.
        let planned = app.join("node_modules/@grida/fx");
        std::fs::create_dir_all(planned.join("dist")).unwrap();
        std::fs::write(planned.join("package.json"), "{}").unwrap();
        std::fs::write(planned.join("dist/host-entry.js"), "").unwrap();
        let spec = home_spec(Language::TypeScript, &planner, &started).unwrap();
        assert_eq!(
            spec.args,
            [planned.join("dist/host-entry.js").into_os_string()]
        );
        assert_eq!(spec.project_root, shared);
        let engine = engine_with_env(&runtime, &planner, false, None, &[], &started).unwrap();
        assert_eq!(
            engine.hosts.pool(Language::TypeScript).unwrap().spec(),
            &spec
        );
        assert_eq!(shown(&engine, &planned), hidden);
        // The home's own @grida/fx comes first; inside the home, its path is the home's.
        let own = shared.join("node_modules/@grida/fx");
        std::fs::create_dir_all(own.join("dist")).unwrap();
        std::fs::write(own.join("package.json"), "{}").unwrap();
        std::fs::write(own.join("dist/host-entry.js"), "").unwrap();
        let spec = home_spec(Language::TypeScript, &planner, &started).unwrap();
        assert_eq!(spec.args, [own.join("dist/host-entry.js").into_os_string()]);
        let engine = engine_with_env(&runtime, &planner, false, None, &[], &started).unwrap();
        assert_eq!(
            shown(&engine, &own),
            "not defined by \"exports\" in node_modules/@grida/fx/package.json imported from \
             node_modules/@grida/fx/dist/host.js"
        );
        // A stand-in host's, found from the planning project alone, stays labelled.
        assert_eq!(shown(&engine, &planned), hidden);
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
        let engine = engine_for(&runtime, &planner, false, None, &[]).unwrap();
        let plan = plan(&engine, &mut planner, &mut FakeHost::new()).unwrap();
        assert!(plan.ok(), "{:?}", plan.problems);
        assert_eq!(plan.expansion.instances.len(), 1);
        shutdown(&runtime, &engine);
    }
}
