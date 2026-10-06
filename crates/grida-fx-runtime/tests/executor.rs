//! The executor: jobs from planned instances, one attempt, and the plan-time runner.
//!
//! Projects are built in temporary folders and planned with the core's `make_planner` and its
//! `FakeHost`. Attempts that need the store or a node host run a fake host: a stdlib-only Python
//! program started through a shell script named as the host's interpreter, so the engine still
//! runs `<python> -P -m grida.fx.host` and the script ignores those arguments. Those tests need
//! `python3` on `PATH` (they skip without it).

use grida_fx_core::docs::takes::TakeChoice;
use grida_fx_core::expand::{Instance, ResultStatus};
use grida_fx_core::host::{FakeHost, NoCache};
use grida_fx_core::money::Usd;
use grida_fx_core::plan::{Plan, make_plan};
use grida_fx_core::project::{PlanRequest, Planner, make_planner};
use grida_fx_core::routes::{PriceUnit, Route, RoutePrice};
use grida_fx_core::spec::{NodeSpec, Port, Retry};
use grida_fx_core::val::{FileContent, Val};
use grida_fx_core::{Error, Problem};
use grida_fx_protocol::{
    ClosureEntry, DescribedType, ErrorCode, ModuleDescription, RetryMode, StandInAnswerParams,
    TypeSpec,
};
use grida_fx_providers::Adapters;
use grida_fx_providers::testing::media;
use grida_fx_runtime::calls::pacing::Pacing;
use grida_fx_runtime::engine::{Cancel, Engine, Services};
use grida_fx_runtime::executor::{CacheUse, InstanceJob, JobBody, execute};
use grida_fx_runtime::host::process::HostSpec;
use grida_fx_runtime::ledger::Ledger;
use grida_fx_runtime::plantime::PlanTime;
use grida_fx_runtime::stand_in::{Answerer, AnswererError, Reply, StandIn, StandInFile};
use grida_fx_runtime::store::ReadSet;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ------------------------------------------------------------------ fixtures

/// A temporary project folder; `root` has its symbolic links resolved.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        Fixture { _dir: dir, root }
    }

    fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn request(&self, target: &str) -> PlanRequest {
        PlanRequest {
            target: target.to_string(),
            cwd: self.root.clone(),
            ..PlanRequest::default()
        }
    }
}

const PROJECT: &str = "\
fx: project/v1
routes:
  image.generate: { route: img-a@acme, concurrency: 3 }
  structured.generate: llm-a@acme
route_tables: [routes.yaml]
";

const ROUTES: &str = "\
fx: routes/v1
routes:
  - { capability: image.generate, route: img-a@acme, price: { low_usd: 0.01, high_usd: 0.04 }, concurrency: 2 }
  - { capability: structured.generate, route: llm-a@acme, price: { low_usd: 0.001, high_usd: 0.002 }, concurrency: 5 }
";

const WORKFLOW: &str = "\
fx: workflow/v1
id: case
title: Case
steps:
  draw:
    uses: ./nodes/n.py#maker
    timeout: 12.5
    with: { text: hello }
  variants:
    uses: ./nodes/n.py#plain
    takes: 3
    with: { text: again }
  resize:
    uses: fx/image.resize@1
    with: { image: ./pics/a.png, longest_side: 64 }
  pick:
    uses: fx/select@1
    with: { first_of: [null, b] }
  key:
    uses: fx/image.key@1
    with: { image: ./pics/a.png }
  gen:
    uses: fx/image.generate@1
    with: { prompt: a cat }
";

fn type_spec(name: &str, calls: Value, resources: &[&str], tools: &[&str]) -> TypeSpec {
    TypeSpec {
        name: name.to_string(),
        description: None,
        inputs: IndexMap::new(),
        params: IndexMap::from([("text".to_string(), json!({"type": "string"}))]),
        outputs: IndexMap::from([("text".to_string(), "text".to_string())]),
        judge: false,
        calls: calls
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        resources: resources.iter().map(|r| (*r).to_string()).collect(),
        tools: tools.iter().map(|t| (*t).to_string()).collect(),
        view: None,
        version: Some(1),
        retry: if name == "maker" {
            RetryMode::Engine
        } else {
            RetryMode::Service
        },
    }
}

fn host(fixture: &Fixture) -> FakeHost {
    FakeHost::new().with_module(ModuleDescription::Described {
        path: "nodes/n.py".into(),
        types: vec![
            DescribedType {
                attribute: "maker".into(),
                spec: type_spec(
                    "maker",
                    json!({"image.generate": 2, "structured.generate": 1}),
                    &["prompts/r.md"],
                    &["blender>=4.2", "nothere"],
                ),
            },
            DescribedType {
                attribute: "plain".into(),
                spec: type_spec("plain", json!({}), &[], &[]),
            },
        ],
        closure: vec![ClosureEntry {
            label: "nodes/n.py".into(),
            path: fixture
                .root
                .join("nodes/n.py")
                .to_string_lossy()
                .into_owned(),
        }],
    })
}

/// The case project, planned; the plan has no problems.
fn planned(digest: &str) -> (Fixture, Planner, Plan) {
    let fixture = Fixture::new();
    fixture.write("fx.yaml", PROJECT);
    fixture.write("routes.yaml", ROUTES);
    fixture.write("nodes/n.py", "x = 1\n");
    fixture.write("prompts/r.md", "Say ${{ text }}\n");
    fixture.write("pics/a.png", b"not really a png");
    fixture.write("workflows/case.yaml", WORKFLOW);
    fixture.write(
        "workflows/case.takes.yaml",
        format!(
            "draw: {{ take: 1, result: {digest} }}\nvariants: {{ take: 2, result: {digest} }}\n"
        ),
    );
    let mut host = host(&fixture);
    let mut planner = make_planner(&fixture.request("case"), &mut host).unwrap();
    let plan = make_plan(&mut planner, &mut host, None, &NoCache).unwrap();
    assert!(plan.ok(), "{:?}", plan.problems);
    (fixture, planner, plan)
}

fn instance<'a>(plan: &'a Plan, id: &str) -> &'a Instance {
    plan.expansion
        .instances
        .get(id)
        .unwrap_or_else(|| panic!("no instance {id}"))
}

/// An environment with only `PATH` = `<dir>/bin`, where an executable `blender` lives.
fn tool_env(dir: &Path) -> HashMap<String, String> {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let blender = bin.join("blender");
    std::fs::write(&blender, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&blender, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    HashMap::from([("PATH".to_string(), bin.to_string_lossy().into_owned())])
}

fn job_of(plan: &Plan, planner: &Planner, id: &str, env: &HashMap<String, String>) -> InstanceJob {
    let lookup = |name: &str| env.get(name).cloned();
    InstanceJob::from_instance(instance(plan, id), planner, &lookup).unwrap()
}

// ------------------------------------------------------------ from_instance

#[test]
fn a_project_types_job_carries_what_the_run_needs() {
    let digest = "d".repeat(64);
    let (fixture, planner, plan) = planned(&digest);
    let env = tool_env(&fixture.root);
    let draw = instance(&plan, "draw#1");
    let job = job_of(&plan, &planner, "draw#1", &env);
    assert_eq!(job.id, "draw#1");
    assert_eq!(job.path, "draw");
    assert_eq!(job.step, "draw");
    assert_eq!(job.key, None);
    assert_eq!(job.takes, [1]);
    assert_eq!(job.uses, "./nodes/n.py#maker");
    assert_eq!(job.type_identity, draw.ty.identity);
    assert_eq!(job.spec.name, "maker");
    assert_eq!(
        job.body,
        JobBody::Project {
            path: "nodes/n.py".into(),
            attribute: "maker".into()
        }
    );
    assert_eq!(job.with, draw.with);
    assert_eq!(job.identity, draw.identity);
    assert!(job.identity.is_some());
    assert_eq!(job.read, grida_fx_runtime::store::read_set(&draw.with));
    assert_eq!(
        job.calls,
        IndexMap::from([
            ("image.generate".to_string(), 2),
            ("structured.generate".to_string(), 1)
        ])
    );
    assert_eq!(
        job.routes.keys().collect::<Vec<_>>(),
        ["image.generate", "structured.generate"]
    );
    // The project's concurrency wins over the route table's; without one, the route's.
    assert_eq!(
        job.limits,
        IndexMap::from([
            ("image.generate".to_string(), Some(3)),
            ("structured.generate".to_string(), Some(5))
        ])
    );
    assert_eq!(
        job.scopes,
        draw.budget.clone().into_iter().collect::<Vec<_>>()
    );
    assert_eq!(
        job.resources,
        IndexMap::from([(
            "prompts/r.md".to_string(),
            fixture.root.join("prompts/r.md")
        )])
    );
    assert_eq!(
        job.tools,
        IndexMap::from([
            (
                "blender".to_string(),
                Some(fixture.root.join("bin/blender"))
            ),
            ("nothere".to_string(), None)
        ])
    );
    assert_eq!(job.timeout_s, Some(12.5));
    assert_eq!(job.retry, Retry::Engine);
    assert_eq!(job.picked, Some(digest.clone()));

    let site = job.call_site(Cancel::new());
    assert_eq!(site.instance_id, "draw#1");
    assert_eq!(site.type_name, "maker");
    assert_eq!(site.limits, job.limits);
}

#[test]
fn a_pick_names_only_the_take_the_takes_file_names() {
    let digest = "e".repeat(64);
    let (fixture, planner, plan) = planned(&digest);
    let env = tool_env(&fixture.root);
    let picked: Vec<Option<String>> = ["variants#1", "variants#2", "variants#3"]
        .iter()
        .map(|id| job_of(&plan, &planner, id, &env).picked)
        .collect();
    assert_eq!(picked, [None, Some(digest), None]);
    assert_eq!(
        planner.takes["variants"],
        TakeChoice {
            take: 2,
            result: Some("e".repeat(64))
        }
    );
    let job = job_of(&plan, &planner, "variants#2", &env);
    assert_eq!(job.retry, Retry::Service);
    assert!(job.tools.is_empty() && job.resources.is_empty());

    // A take entry without a result picks nothing.
    let mut planner = planner;
    planner.takes.insert(
        "variants".into(),
        TakeChoice {
            take: 2,
            result: None,
        },
    );
    assert_eq!(job_of(&plan, &planner, "variants#2", &env).picked, None);
}

#[test]
fn built_ins_get_their_bodies() {
    let (fixture, planner, plan) = planned(&"f".repeat(64));
    let env = tool_env(&fixture.root);
    assert_eq!(
        job_of(&plan, &planner, "resize#1", &env).body,
        JobBody::Std {
            builtin: "fx/image.resize@1".into()
        }
    );
    assert_eq!(
        job_of(&plan, &planner, "pick#1", &env).body,
        JobBody::Select
    );
    assert_eq!(job_of(&plan, &planner, "key#1", &env).body, JobBody::None);
    let generate = job_of(&plan, &planner, "gen#1", &env);
    assert_eq!(
        generate.body,
        JobBody::Capability {
            capability: "image.generate".into()
        }
    );
    assert_eq!(
        generate.calls,
        IndexMap::from([("image.generate".to_string(), 1)])
    );
    assert_eq!(
        generate.limits,
        IndexMap::from([("image.generate".to_string(), Some(3))])
    );
    assert_eq!(generate.picked, None);
}

#[test]
fn a_pending_with_value_cannot_run() {
    let (fixture, planner, plan) = planned(&"f".repeat(64));
    let env = tool_env(&fixture.root);
    let mut waiting = instance(&plan, "draw#1").clone();
    waiting.with.insert(
        "text".into(),
        Val::Pending(Box::new(grida_fx_core::val::Pending::of("other#1", None))),
    );
    let lookup = |name: &str| env.get(name).cloned();
    assert_eq!(
        InstanceJob::from_instance(&waiting, &planner, &lookup).unwrap_err(),
        "draw#1 cannot run yet: it waits on other#1"
    );
}

// ------------------------------------------------------------ attempts

fn engine(handle: tokio::runtime::Handle, project: &Path, python: &Path) -> Arc<Engine> {
    let spec = HostSpec {
        python: python.to_path_buf(),
        label: "python3".into(),
        project_root: project.to_path_buf(),
        sources: Vec::new(),
    };
    Arc::new(Engine::new(
        handle,
        spec,
        &project.join(".fx/cache"),
        2,
        Adapters::new(),
        false,
    ))
}

/// A hand-made job of a project type with one `text` output.
fn hand_job(body: JobBody, with: Vec<(&str, Val)>, identity: Option<String>) -> InstanceJob {
    let spec = NodeSpec {
        name: "maker".into(),
        description: None,
        inputs: IndexMap::from([("image".to_string(), Port::parse("image?").unwrap())]),
        params: IndexMap::from([
            ("mode".to_string(), json!({"type": "string"})),
            ("first_of".to_string(), json!({"type": "array"})),
        ]),
        outputs: IndexMap::from([("text".to_string(), Port::parse("text").unwrap())]),
        judge: false,
        capability: None,
        calls: IndexMap::new(),
        resources: Vec::new(),
        tools: Vec::new(),
        view: None,
        version: Some(1),
        retry: Retry::Service,
    };
    InstanceJob {
        id: "make#1".into(),
        path: "make".into(),
        step: "make".into(),
        key: None,
        takes: vec![1],
        uses: "./nodes/n.py#maker".into(),
        type_identity: "a".repeat(64),
        spec: Arc::new(spec),
        body,
        with: with.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        identity,
        read: ReadSet::new(),
        calls: IndexMap::new(),
        routes: IndexMap::new(),
        limits: IndexMap::new(),
        scopes: Vec::new(),
        resources: IndexMap::new(),
        tools: IndexMap::new(),
        timeout_s: None,
        retry: Retry::Service,
        picked: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn select_and_body_less_built_ins_need_no_host() {
    let fixture = Fixture::new();
    let engine = engine(
        tokio::runtime::Handle::current(),
        &fixture.root,
        Path::new("python3"),
    );
    let services = Arc::new(Services::planning(engine));

    let select = hand_job(
        JobBody::Select,
        vec![(
            "first_of",
            Val::List(vec![Val::Missing, Val::Str("b".into())]),
        )],
        None,
    );
    let attempt = execute(Arc::clone(&services), Arc::new(select), Cancel::new()).await;
    assert_eq!(attempt.cache, CacheUse::Miss);
    assert_eq!(attempt.result.status, ResultStatus::Succeeded);
    assert_eq!(attempt.result.outputs["value"], Val::Str("b".into()));
    assert_eq!(attempt.result.facts["chosen"], json!(1));
    assert!(!attempt.retryable);
    assert_eq!(attempt.stop, None);

    let mut none = hand_job(JobBody::None, vec![], None);
    none.uses = "fx/image.key@1".into();
    let attempt = execute(Arc::clone(&services), Arc::new(none), Cancel::new()).await;
    assert_eq!(attempt.result.status, ResultStatus::Failed);
    assert_eq!(
        attempt.result.error.as_deref(),
        Some("fx/image.key@1 has no implementation yet")
    );
    assert!(attempt.result.facts.is_empty());
    assert!(!attempt.retryable);
}

// ------------------------------------------------------------ the plan-time runner

const AT_PLAN: &str = "\
fx: workflow/v1
id: early
title: Early
steps:
  pick:
    uses: fx/select@1
    at: plan
    with: { first_of: [null, b] }
  key:
    uses: fx/image.key@1
    at: plan
    with: { image: ./pics/a.png }
";

#[test]
fn at_plan_steps_run_while_planning() {
    let fixture = Fixture::new();
    fixture.write("fx.yaml", "fx: project/v1\n");
    fixture.write("pics/a.png", b"not really a png");
    fixture.write("workflows/early.yaml", AT_PLAN);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let engine = engine(
        runtime.handle().clone(),
        &fixture.root,
        Path::new("python3"),
    );
    let mut runner = PlanTime::new(engine);
    assert!(!runner.services.live());
    let mut host = FakeHost::new();
    let mut planner = make_planner(&fixture.request("early"), &mut host).unwrap();
    let plan = make_plan(&mut planner, &mut host, Some(&mut runner), &NoCache).unwrap();
    let pick = &planner.results["pick#1"];
    assert_eq!(pick.status, ResultStatus::Succeeded);
    assert_eq!(pick.outputs["value"], Val::Str("b".into()));
    assert_eq!(pick.facts["chosen"], json!(1));
    assert_eq!(planner.results["key#1"].status, ResultStatus::Failed);
    assert_eq!(
        plan.problems,
        [Problem::new(
            "key",
            "failed while planning: fx/image.key@1 has no implementation yet"
        )]
    );
}

// ------------------------------------------------------------------------------ with a host

/// The fake node host. It answers `run` by the `mode` param.
const FAKE_HOST: &str = r#"
import json
import os
import sys
import time

IN = sys.stdin.buffer
OUT = sys.stdout.buffer
NEXT_ID = [100]


def read():
    length = None
    while True:
        line = IN.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(IN.read(length).decode("utf-8"))


def write(message):
    body = json.dumps(message, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    OUT.write(b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body)
    OUT.flush()


def answer(request, result):
    write({"jsonrpc": "2.0", "id": request["id"], "result": result})


def fail(request, code, message, data=None):
    error = {"code": code, "message": message}
    if data is not None:
        error["data"] = data
    write({"jsonrpc": "2.0", "id": request["id"], "error": error})


def ask(method, params):
    """Sends a request to the engine and waits for its answer."""
    NEXT_ID[0] += 1
    mine = NEXT_ID[0]
    write({"jsonrpc": "2.0", "id": mine, "method": method, "params": params})
    while True:
        message = read()
        if message is None:
            sys.exit(5)
        if message.get("id") == mine and "method" not in message:
            return message


def run(message):
    params = message["params"]
    mode = params["params"].get("mode")
    work = params["work_dir"]
    run_id = params["run_id"]
    if mode == "ok":
        with open(os.path.join(work, "out.txt"), "w") as f:
            f.write("hi from " + params["instance"]["id"])
        with open("runs.log", "a") as f:
            f.write(run_id + "\n")
        ask("fact", {"run_id": run_id, "name": "score", "value": 7})
        answer(message, {"outputs": {"text": {"work_path": "out.txt"}}, "facts": {"note": "x"}})
    elif mode == "pass":
        answer(message, {"outputs": {"text": {"file": params["inputs"]["image"]}}})
    elif mode == "escape":
        answer(message, {"outputs": {"text": {"work_path": "../../outside.txt"}}})
    elif mode == "fail":
        ask("fact", {"run_id": run_id, "name": "seen", "value": True})
        fail(message, -32000, "the brief asks for nothing", {"facts": {"why": 1}})
    elif mode == "error":
        fail(message, -32001, "boom at " + os.getcwd() + "/nodes/n.py",
             {"exception": "ValueError", "traceback": "..."})
    elif mode == "render":
        reply = ask("prompt.render", {"run_id": run_id, "path": "prompts/gone.md", "variables": {}})
        fail(message, reply["error"]["code"], reply["error"]["message"])
    elif mode == "caught":
        reply = ask("file.put", {"run_id": run_id, "base64": "AAE="})
        fail(message, -32000, "caught: " + reply["error"]["message"])
    elif mode == "caught_ok":
        # Catches whatever the call answers, then answers a result anyway.
        ask("capability", {"run_id": run_id, "capability": "image.generate",
                           "request": {"prompt": "a kite", "size": "64x64",
                                       "background": "opaque"}})
        with open(os.path.join(work, "out.txt"), "w") as f:
            f.write("a fallback")
        answer(message, {"outputs": {"text": {"work_path": "out.txt"}}})
    elif mode == "exit":
        os._exit(3)
    elif mode == "slow":
        time.sleep(60)
    else:
        fail(message, -32004, "no mode " + str(mode))


def main():
    while True:
        message = read()
        if message is None:
            sys.exit(0)
        method = message.get("method")
        if method == "initialize":
            answer(message, {"protocol": message["params"]["protocol"],
                             "host": {"language": "python", "version": "3", "sdk_version": "0"}})
        elif method == "run":
            run(message)
        elif method == "shutdown":
            answer(message, None)
        elif method == "exit":
            sys.exit(0)
        elif method == "$/cancel":
            pass
        elif "id" in message and method is not None:
            fail(message, -32601, "no " + method)


main()
"#;

fn have_python3() -> bool {
    let found = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
    }
    found
}

/// The fake host's interpreter: a script that runs it whatever arguments it gets.
fn fake_python(dir: &Path) -> PathBuf {
    let program = dir.join("fake_host.py");
    std::fs::write(&program, FAKE_HOST).unwrap();
    let script = dir.join("python");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nexec python3 {}\n", program.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

fn mode(mode: &str) -> Vec<(&'static str, Val)> {
    vec![("mode", Val::Str(mode.into()))]
}

async fn attempt_of(
    services: &Arc<Services>,
    job: InstanceJob,
) -> grida_fx_runtime::executor::Attempt {
    execute(Arc::clone(services), Arc::new(job), Cancel::new()).await
}

/// Work dirs under the store's `work/` (an invocation's claim, `<id>.lock`, is a file).
fn work_dirs_left(engine: &Engine) -> usize {
    std::fs::read_dir(engine.store.work_root()).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_engines_own_faults_stop_the_run() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let python = fake_python(fixture.root.parent().unwrap());
    let engine = engine(tokio::runtime::Handle::current(), &fixture.root, &python);
    let services = Arc::new(Services::planning(Arc::clone(&engine)));
    let body = JobBody::Project {
        path: "nodes/n.py".into(),
        attribute: "maker".into(),
    };
    // A resource that cannot be read: the body lets the `internal` answer propagate, and only
    // the node fails.
    let mut job = hand_job(body.clone(), mode("render"), None);
    job.resources.insert(
        "prompts/gone.md".into(),
        fixture.root.join("prompts/gone.md"),
    );
    let attempt = attempt_of(&services, job).await;
    assert_eq!(attempt.result.status, ResultStatus::Failed);
    assert_eq!(
        attempt.result.error.as_deref(),
        Some("cannot read prompts/gone.md: no such file")
    );
    assert_eq!(attempt.stop, None);
    // A store that cannot be written stops the run, even though the body caught the error.
    std::fs::create_dir_all(fixture.root.join(".fx/cache")).unwrap();
    std::fs::write(fixture.root.join(".fx/cache/files"), b"in the way").unwrap();
    let attempt = attempt_of(&services, hand_job(body, mode("caught"), None)).await;
    assert_eq!(attempt.result.status, ResultStatus::Failed);
    let stop = attempt.stop.expect("a store fault stops the run");
    assert!(stop.starts_with("the store's files/"), "{stop}");
    assert_eq!(attempt.result.error, Some(format!("caught: {stop}")));
}

/// A stand-in that faults, or answers a picture, and counts what it was asked.
struct CountingStandIn {
    fault: bool,
    asked: AtomicUsize,
}

impl Answerer for CountingStandIn {
    fn answer<'a>(
        &'a self,
        _params: StandInAnswerParams,
        _cancel: &'a Cancel,
    ) -> grida_fx_providers::BoxFuture<'a, Result<Reply, AnswererError>> {
        Box::pin(async move {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if self.fault {
                return Err(AnswererError::Fault(
                    "the stand-in failed: RuntimeError: harness broke".into(),
                ));
            }
            Ok(Reply::Answer {
                files: IndexMap::from([(
                    "image".to_string(),
                    StandInFile {
                        kind: None,
                        bytes: media::png(64, 64, None),
                    },
                )]),
                data: Value::Null,
            })
        })
    }

    fn shutdown(&self) -> grida_fx_providers::BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// A stand-in run's services over the stand-in store of `project`.
fn stand_in_services(
    project: &Path,
    python: &Path,
    stand_in: Arc<CountingStandIn>,
) -> Arc<Services> {
    let spec = HostSpec {
        python: python.to_path_buf(),
        label: "python3".into(),
        project_root: project.to_path_buf(),
        sources: Vec::new(),
    };
    let engine = Engine::new(
        tokio::runtime::Handle::current(),
        spec,
        &project.join(".fx/cache/stand-in"),
        2,
        Adapters::new(),
        false,
    )
    .with_stand_in(Arc::new(StandIn::new(stand_in)));
    Arc::new(Services {
        engine: Arc::new(engine),
        ledger: Some(Arc::new(Ledger::new(None, None))),
        events: None,
        pacing: Arc::new(Pacing::new()),
        cancel: Cancel::new(),
        invocation_id: "inv".into(),
        runs: AtomicU64::new(0),
        holds: AtomicU64::new(0),
    })
}

/// The caught-fault job: a body that calls `image.generate` once on `img-a@acme`.
fn calling_job(identity: &str) -> InstanceJob {
    let mut job = hand_job(
        JobBody::Project {
            path: "nodes/n.py".into(),
            attribute: "maker".into(),
        },
        mode("caught_ok"),
        Some(identity.to_string()),
    );
    job.calls = IndexMap::from([("image.generate".to_string(), 1)]);
    let route = Route {
        capability: "image.generate".into(),
        model: "img-a".into(),
        provider: "acme".into(),
        price: RoutePrice {
            low: Usd(10_000),
            high: Usd(40_000),
            unit: PriceUnit::Call,
            max_units: None,
            by: None,
            tiers: IndexMap::new(),
        },
        features: Default::default(),
        concurrency: None,
        requests_per_minute: None,
        contract: json!({}),
    };
    job.routes = IndexMap::from([("image.generate".to_string(), route)]);
    job
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_made_after_a_caught_stand_in_fault_is_not_recorded() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let python = fake_python(fixture.root.parent().unwrap());
    let identity = "d".repeat(64);

    // The stand-in faults; the body catches the `internal` answer and answers a result anyway.
    let broken = Arc::new(CountingStandIn {
        fault: true,
        asked: AtomicUsize::new(0),
    });
    let services = stand_in_services(&fixture.root, &python, Arc::clone(&broken));
    let attempt = attempt_of(&services, calling_job(&identity)).await;
    assert_eq!(broken.asked.load(Ordering::SeqCst), 1);
    assert_eq!(attempt.result.status, ResultStatus::Failed);
    assert_eq!(
        attempt.stop.as_deref(),
        Some("the stand-in failed: RuntimeError: harness broke")
    );
    assert_eq!(attempt.result.error, attempt.stop);
    assert_eq!(attempt.code, Some(ErrorCode::Internal));
    let results = fixture.root.join(".fx/cache/stand-in/results");
    assert!(!results.exists() || std::fs::read_dir(&results).unwrap().next().is_none());

    // The next stand-in run finds nothing cached and asks its stand-in again.
    let good = Arc::new(CountingStandIn {
        fault: false,
        asked: AtomicUsize::new(0),
    });
    let services = stand_in_services(&fixture.root, &python, Arc::clone(&good));
    let attempt = attempt_of(&services, calling_job(&identity)).await;
    assert_eq!(attempt.result.error, None);
    assert_eq!(attempt.cache, CacheUse::Miss);
    assert_eq!(good.asked.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_runs_on_a_host_and_its_result_is_cached() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let python = fake_python(fixture.root.parent().unwrap());
    let engine = engine(tokio::runtime::Handle::current(), &fixture.root, &python);
    let services = Arc::new(Services::planning(Arc::clone(&engine)));
    let identity = "c".repeat(64);
    let body = JobBody::Project {
        path: "nodes/n.py".into(),
        attribute: "maker".into(),
    };

    let attempt = attempt_of(
        &services,
        hand_job(body.clone(), mode("ok"), Some(identity.clone())),
    )
    .await;
    assert_eq!(attempt.result.error, None);
    assert_eq!(attempt.result.status, ResultStatus::Succeeded);
    assert_eq!(attempt.cache, CacheUse::Miss);
    let Val::File(text) = &attempt.result.outputs["text"] else {
        panic!("one file");
    };
    assert_eq!(text.name, "make/text");
    assert_eq!(text.kind, "text/plain");
    assert_eq!(
        text.content,
        Some(FileContent::Text("hi from make#1".into()))
    );
    assert_eq!(
        attempt.result.facts,
        IndexMap::from([
            ("score".to_string(), json!(7)),
            ("note".to_string(), json!("x")),
            ("cost_usd".to_string(), Value::Null),
        ])
    );
    assert_eq!(work_dirs_left(&engine), 0);

    // The record answers the next attempt; nothing runs.
    let again = attempt_of(
        &services,
        hand_job(body, mode("never runs"), Some(identity)),
    )
    .await;
    assert_eq!(again.cache, CacheUse::Hit);
    assert_eq!(again.result.outputs, attempt.result.outputs);
    assert_eq!(again.result.facts, attempt.result.facts);
}

#[tokio::test(flavor = "multi_thread")]
async fn host_failures_become_results() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let python = fake_python(fixture.root.parent().unwrap());
    let engine = engine(tokio::runtime::Handle::current(), &fixture.root, &python);
    let services = Arc::new(Services::planning(Arc::clone(&engine)));
    let body = JobBody::Std {
        builtin: "fx/files.copy@1".into(),
    };
    let identity = Some("d".repeat(64));

    let failed = attempt_of(
        &services,
        hand_job(body.clone(), mode("fail"), identity.clone()),
    )
    .await;
    assert_eq!(failed.result.status, ResultStatus::Failed);
    assert_eq!(
        failed.result.error.as_deref(),
        Some("the brief asks for nothing")
    );
    assert_eq!(
        failed.result.facts,
        IndexMap::from([
            ("seen".to_string(), json!(true)),
            ("why".to_string(), json!(1))
        ])
    );
    assert!(!failed.retryable);
    assert_eq!(failed.code, Some(ErrorCode::NodeFailure));
    // A failed attempt writes no record.
    let again = attempt_of(&services, hand_job(body.clone(), mode("fail"), identity)).await;
    assert_eq!(again.cache, CacheUse::Miss);

    // The project root is cut from the message.
    let error = attempt_of(&services, hand_job(body.clone(), mode("error"), None)).await;
    assert_eq!(
        error.result.error.as_deref(),
        Some("ValueError: boom at nodes/n.py")
    );
    assert!(error.retryable);
    assert_eq!(error.code, Some(ErrorCode::NodeError));

    let exited = attempt_of(&services, hand_job(body.clone(), mode("exit"), None)).await;
    assert_eq!(
        exited.result.error.as_deref(),
        Some("the node host exited with status 3")
    );
    assert!(exited.retryable);
    // A host that exited fails `node_error`, as a body's exception does.
    assert_eq!(exited.code, Some(ErrorCode::NodeError));

    let escape = attempt_of(&services, hand_job(body.clone(), mode("escape"), None)).await;
    assert_eq!(
        escape.result.error.as_deref(),
        Some("output text: ../../outside.txt is not a file inside the work dir")
    );
    assert!(!escape.retryable);
    // A result that fails a check fails `invalid_params`.
    assert_eq!(escape.code, Some(ErrorCode::InvalidParams));

    let mut slow = hand_job(body, mode("slow"), None);
    slow.timeout_s = Some(0.5);
    let timed_out = attempt_of(&services, slow).await;
    assert_eq!(
        timed_out.result.error.as_deref(),
        Some("ran past 0.5 seconds")
    );
    assert!(!timed_out.retryable);
    // A timeout carries no code.
    assert_eq!(timed_out.code, None);
    assert_eq!(work_dirs_left(&engine), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn inputs_are_adopted_and_can_be_passed_through() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    let python = fake_python(fixture.root.parent().unwrap());
    let engine = engine(tokio::runtime::Handle::current(), &fixture.root, &python);
    let services = Arc::new(Services::planning(Arc::clone(&engine)));
    let picture = fixture.write("pics/a.png", b"picture bytes");
    let file = grida_fx_core::inputs::bind::read_input_file(&picture, "a.png", "image").unwrap();
    let digest = file.digest.clone();
    let mut with = mode("pass");
    with.push(("image", Val::File(Box::new(file))));
    let job = hand_job(
        JobBody::Project {
            path: "nodes/n.py".into(),
            attribute: "maker".into(),
        },
        with,
        None,
    );
    let attempt = attempt_of(&services, job).await;
    assert_eq!(attempt.result.error, None);
    assert!(engine.store.has(&digest, 13));
    let Val::File(text) = &attempt.result.outputs["text"] else {
        panic!("one file");
    };
    assert_eq!(text.digest, digest);
    assert_eq!(text.kind, "image/png");
    assert_eq!(text.name, "make/text");
}

const AT_PLAN_BODY: &str = "\
fx: workflow/v1
id: early
title: Early
steps:
  make:
    uses: ./nodes/n.py#maker
    at: plan
    with: { mode: ok }
";

#[test]
fn an_at_plan_result_is_recorded_for_the_next_plan() {
    if !have_python3() {
        return;
    }
    let fixture = Fixture::new();
    fixture.write("fx.yaml", "fx: project/v1\n");
    fixture.write("nodes/n.py", "x = 1\n");
    fixture.write("workflows/early.yaml", AT_PLAN_BODY);
    let python = fake_python(fixture.root.parent().unwrap());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let engine = engine(runtime.handle().clone(), &fixture.root, &python);
    let mut spec = type_spec("maker", json!({}), &[], &[]);
    spec.params = IndexMap::from([("mode".to_string(), json!({"type": "string"}))]);
    let mut host = FakeHost::new().with_module(ModuleDescription::Described {
        path: "nodes/n.py".into(),
        types: vec![DescribedType {
            attribute: "maker".into(),
            spec,
        }],
        closure: vec![ClosureEntry {
            label: "nodes/n.py".into(),
            path: fixture
                .root
                .join("nodes/n.py")
                .to_string_lossy()
                .into_owned(),
        }],
    });
    let runs = || {
        std::fs::read_to_string(fixture.root.join("runs.log"))
            .map_or(0, |text| text.lines().count())
    };
    for round in 0..2 {
        let mut runner = PlanTime::new(Arc::clone(&engine));
        let mut planner = make_planner(&fixture.request("early"), &mut host).unwrap();
        let plan: Result<Plan, Error> =
            make_plan(&mut planner, &mut host, Some(&mut runner), &*engine.store);
        let plan = plan.unwrap();
        assert!(plan.ok(), "{:?}", plan.problems);
        let made = &planner.results["make#1"];
        assert_eq!(made.status, ResultStatus::Succeeded, "{:?}", made.error);
        assert_eq!(made.facts["cost_usd"], Value::Null);
        // The second plan is answered by the record the first one wrote.
        assert_eq!(runs(), 1, "round {round}");
    }
}
