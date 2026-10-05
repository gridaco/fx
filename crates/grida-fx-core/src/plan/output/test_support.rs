//! Hand-built plans for the tests of the plan text and the plan documents: the instances,
//! prices, pending repeats and problems of the conformance cases, as the expander would leave
//! them, and a planner around them. Nothing here reads a file.

use crate::docs::lock::LockFile;
use crate::docs::project::{Project, ProjectDoc};
use crate::docs::takes::TakeChoice;
use crate::docs::workflow::{LoadedWorkflow, WorkflowDoc};
use crate::error::Problem;
use crate::expand::{CallPrice, Expansion, Instance, PendingRepeat, State};
use crate::inputs::bind::RootInputs;
use crate::money::Usd;
use crate::plan::Plan;
use crate::project::Planner;
use crate::registry::{Registry, ResolvedType, SourceDigests, TypeOrigin};
use crate::routes::{PriceUnit, Route, RoutePrice, RouteTable};
use crate::spec::{BodyKind, NodeSpec, Retry};
use crate::val::{FileValue, Val};
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;

/// 64 times one hex digit: a stand-in identity.
pub(crate) fn hex(digit: char) -> String {
    digit.to_string().repeat(64)
}

fn spec(name: &str, capability: Option<&str>) -> NodeSpec {
    NodeSpec {
        name: name.to_string(),
        description: None,
        inputs: IndexMap::new(),
        params: IndexMap::new(),
        outputs: IndexMap::new(),
        judge: false,
        capability: capability.map(str::to_string),
        calls: IndexMap::new(),
        resources: Vec::new(),
        tools: Vec::new(),
        view: None,
        version: Some(1),
        retry: Retry::Service,
    }
}

/// The built-in `fx/<name>@1`; `paid` makes it a capability type of its own name.
pub(crate) fn builtin_type(name: &str, paid: bool) -> Rc<ResolvedType> {
    Rc::new(ResolvedType {
        uses: format!("fx/{name}@1"),
        identity: format!("fx/{name}@1.1"),
        spec: Rc::new(spec(name, paid.then_some(name))),
        origin: TypeOrigin::Builtin {
            name: name.to_string(),
            major: 1,
        },
        body: if paid {
            BodyKind::Capability
        } else {
            BodyKind::Python
        },
        source: None,
        drift: None,
    })
}

/// A project type `./<path>#<attribute>`.
pub(crate) fn project_type(
    path: &str,
    attribute: &str,
    identity: &str,
    source: SourceDigests,
) -> Rc<ResolvedType> {
    Rc::new(ResolvedType {
        uses: format!("./{path}#{attribute}"),
        identity: identity.to_string(),
        spec: Rc::new(spec(attribute, None)),
        origin: TypeOrigin::Project {
            path: path.to_string(),
            attribute: attribute.to_string(),
        },
        body: BodyKind::Project,
        source: Some(source),
        drift: None,
    })
}

/// A route priced per call, with no contract.
pub(crate) fn route(capability: &str, model: &str, provider: &str) -> Route {
    Route {
        capability: capability.to_string(),
        model: model.to_string(),
        provider: provider.to_string(),
        price: RoutePrice {
            low: Usd(10_000),
            high: Usd(40_000),
            unit: PriceUnit::Call,
            max_units: None,
            by: None,
            tiers: IndexMap::new(),
        },
        features: BTreeSet::new(),
        concurrency: None,
        requests_per_minute: None,
        contract: json!({}),
    }
}

/// An instance of a type; `id` is `<path>#<takes>` and the path has no repeat keys. Prices are
/// `(calls, low, high)` in micro-dollars, each on `img-a@acme`.
pub(crate) fn instance_of(
    id: &str,
    ty: &Rc<ResolvedType>,
    state: State,
    phase: u32,
    prices: &[(u32, i64, i64)],
    identity: Option<&str>,
) -> Instance {
    let (path, takes) = id.split_once('#').expect("an instance id");
    let takes: Vec<u32> = takes.split('.').map(|t| t.parse().unwrap()).collect();
    let capability = ty
        .spec
        .capability
        .clone()
        .unwrap_or_else(|| "image.generate".to_string());
    Instance {
        id: id.to_string(),
        path: path.to_string(),
        step: path.to_string(),
        takes,
        uses: ty.uses.clone(),
        ty: ty.clone(),
        with: IndexMap::new(),
        needs: Vec::new(),
        state,
        identity: identity.map(str::to_string),
        routes: IndexMap::new(),
        prices: prices
            .iter()
            .map(|&(calls, low, high)| CallPrice {
                capability: capability.clone(),
                route: "img-a@acme".to_string(),
                calls,
                low: Usd(low),
                high: Usd(high),
            })
            .collect(),
        phase,
        key: None,
        judges: None,
        judge_policy: None,
        judged_by: Vec::new(),
        view: Value::Bool(false),
        at_plan: false,
        budget: None,
        concurrency_group: None,
        concurrency: None,
        timeout_s: None,
        reason: None,
        reads: BTreeSet::new(),
    }
}

/// An instance of `fx/image.generate@1`.
pub(crate) fn instance(
    id: &str,
    state: State,
    phase: u32,
    prices: &[(u32, i64, i64)],
    identity: Option<&str>,
) -> Instance {
    instance_of(
        id,
        &builtin_type("image.generate", true),
        state,
        phase,
        prices,
        identity,
    )
}

fn pending(path: &str, max: u32, phase: u32, low: i64, high: i64) -> PendingRepeat {
    PendingRepeat {
        path: path.to_string(),
        max,
        waiting_on: BTreeSet::new(),
        per_instance_low: Usd(low),
        per_instance_high: Usd(high),
        phase,
    }
}

/// A planner for a workflow document at `source` (project-relative), under a made-up root.
pub(crate) fn planner_for(source: &str, document: Value) -> Planner {
    let root = PathBuf::from("/fx-test/project");
    let project = Project {
        root: root.clone(),
        document: ProjectDoc::default(),
        has_file: true,
    };
    let id = document["id"].as_str().unwrap_or("case").to_string();
    let title = document["title"].as_str().unwrap_or("Case").to_string();
    let workflow = WorkflowDoc {
        id: id.clone(),
        title,
        description: None,
        inputs: IndexMap::new(),
        tables: IndexMap::new(),
        let_: IndexMap::new(),
        budget: None,
        asserts: Vec::new(),
        steps: Rc::new(IndexMap::new()),
        outputs: IndexMap::new(),
        view: None,
    };
    Planner {
        cwd: root.clone(),
        project: project.clone(),
        home: project,
        workflow: Rc::new(LoadedWorkflow {
            document,
            workflow: Rc::new(workflow),
            source: source.to_string(),
            path: root.join(source),
        }),
        input_schema: json!({"type": "object", "properties": {}}),
        inputs: RootInputs::default(),
        routes: RouteTable::default(),
        takes: IndexMap::new(),
        takes_path: root.join(format!("workflows/{id}.takes.yaml")),
        registry: Registry::new(root, Vec::new(), LockFile::default(), IndexMap::new()),
        results: IndexMap::new(),
        max_usd: None,
    }
}

/// The planner of `workflows/case.yaml`, id `case`.
pub(crate) fn planner() -> Planner {
    planner_for(
        "workflows/case.yaml",
        json!({"fx": "workflow/v1", "id": "case", "title": "Case", "steps": {}}),
    )
}

/// A plan of these instances: their types and the planner's workflow recorded as an expansion
/// records them.
pub(crate) fn plan_of(
    planner: &Planner,
    instances: Vec<Instance>,
    pending: Vec<PendingRepeat>,
    problems: Vec<Problem>,
    ceiling: Option<Usd>,
) -> Plan {
    let mut expansion = Expansion::default();
    expansion.workflows.insert(
        planner.workflow.source.clone(),
        planner.workflow.document.clone(),
    );
    for instance in instances {
        expansion
            .types
            .entry(instance.uses.clone())
            .or_insert_with(|| instance.ty.clone());
        expansion.instances.insert(instance.id.clone(), instance);
    }
    expansion.pending = pending;
    expansion.problems = problems.clone();
    Plan {
        expansion,
        problems,
        ceiling,
        cached: BTreeSet::new(),
    }
}

/// Case `linear`: a local `count` read by `draw`, whose identity waits on it.
pub(crate) fn linear_plan() -> (Plan, Planner) {
    let planner = planner();
    let lines = project_type(
        "nodes/cases.py",
        "lines",
        "nodes/cases.py#lines@1",
        SourceDigests {
            files: BTreeMap::from([("nodes/cases.py".to_string(), hex('e'))]),
            resources: BTreeMap::new(),
        },
    );
    let count = instance_of("count#1", &lines, State::Planned, 1, &[], Some(&hex('d')));
    let mut draw = instance("draw#1", State::Planned, 1, &[(1, 10_000, 40_000)], None);
    draw.reads.insert("count#1".into());
    let plan = plan_of(&planner, vec![count, draw], vec![], vec![], None);
    (plan, planner)
}

/// Built-in steps only, one of them refused its route.
pub(crate) fn builtin_plan() -> (Plan, Planner) {
    let planner = planner();
    let copy = instance_of(
        "copy#1",
        &builtin_type("files.copy", false),
        State::Planned,
        1,
        &[],
        Some(&hex('a')),
    );
    let mut draw = instance("draw#1", State::Planned, 1, &[], None);
    draw.reads.insert("copy#1".into());
    let problems = vec![Problem::new(
        "draw.route",
        "no route img-a@acme serves image.generate (known routes: none)",
    )];
    let plan = plan_of(&planner, vec![copy, draw], vec![], problems, None);
    (plan, planner)
}

/// The five problems of case `refusals`.
pub(crate) fn refusals_problems() -> Vec<Problem> {
    vec![
        Problem::new("workflow.assert[0]", "at most one name"),
        Problem::new("draw.requires", "img-a@acme does not support mask"),
        Problem::new(
            "review.independent_of",
            "shares the model llm-a with write; route one of them to a different model",
        ),
        Problem::new(
            "lost.route",
            "no route img-z@nowhere serves image.generate (known routes: img-a@acme)",
        ),
        Problem::new(
            "unbounded",
            "the list comes from a step, so the plan cannot count it: add max: (the most items this repeat may run)",
        ),
    ]
}

/// Case `refusals`: four steps, three paid calls, an unbounded repeat and five problems.
pub(crate) fn refusals_plan() -> (Plan, Planner) {
    let planner = planner();
    let write = instance_of(
        "write#1",
        &builtin_type("structured.generate", true),
        State::Planned,
        1,
        &[(1, 2_000, 4_000)],
        Some(&hex('1')),
    );
    let review = instance_of(
        "review#1",
        &builtin_type("structured.review", true),
        State::Planned,
        1,
        &[(1, 1_000, 2_000)],
        Some(&hex('2')),
    );
    let draw = instance_of(
        "draw#1",
        &builtin_type("image.edit", true),
        State::Planned,
        1,
        &[(1, 10_000, 40_000)],
        Some(&hex('3')),
    );
    let lost = instance("lost#1", State::Planned, 1, &[], Some(&hex('4')));
    let plan = plan_of(
        &planner,
        vec![write, review, draw, lost],
        vec![pending("unbounded", 1, 2, 0, 0)],
        refusals_problems(),
        None,
    );
    (plan, planner)
}

/// Case `tiered-price`: three clips priced per second at their tiers.
pub(crate) fn tiered_plan() -> (Plan, Planner) {
    let planner = planner();
    let video = builtin_type("video.generate", true);
    let small = instance_of(
        "small#1",
        &video,
        State::Planned,
        1,
        &[(1, 90_000, 112_500)],
        Some(&hex('5')),
    );
    let large = instance_of(
        "large#1",
        &video,
        State::Planned,
        1,
        &[(1, 2_400_000, 3_000_000)],
        Some(&hex('6')),
    );
    let unknown = instance_of(
        "unknown#1",
        &video,
        State::Planned,
        1,
        &[(1, 0, 3_750_000)],
        Some(&hex('7')),
    );
    let plan = plan_of(&planner, vec![small, large, unknown], vec![], vec![], None);
    (plan, planner)
}

/// Case `judge-regenerate` before anything ran: three takes of `draw`, each judged by a free
/// `check`, and `after` reading the chosen take; a ceiling of $1.
pub(crate) fn judge_regenerate_plan() -> (Plan, Planner) {
    let planner = planner();
    let check = builtin_type("image.check_alpha", false);
    let mut instances = Vec::new();
    for take in 1..=3 {
        let state = if take == 1 {
            State::Planned
        } else {
            State::Maybe
        };
        let digit = char::from_digit(take, 10).unwrap();
        instances.push(instance(
            &format!("draw#{take}"),
            state,
            1,
            &[(1, 10_000, 40_000)],
            Some(&hex(digit)),
        ));
        instances.push(instance_of(
            &format!("check#{take}"),
            &check,
            state,
            1,
            &[],
            None,
        ));
    }
    instances.push(instance_of(
        "after#1",
        &builtin_type("files.copy", false),
        State::Planned,
        1,
        &[],
        None,
    ));
    let plan = plan_of(&planner, instances, vec![], vec![], Some(Usd(1_000_000)));
    (plan, planner)
}

/// Case `phase`: one free step whose list a later phase draws over, up to six times; a ceiling
/// of $1.
pub(crate) fn phase_plan() -> (Plan, Planner) {
    let planner = planner();
    let lines = instance_of(
        "lines#1",
        &builtin_type("files.copy", false),
        State::Planned,
        1,
        &[],
        Some(&hex('8')),
    );
    let plan = plan_of(
        &planner,
        vec![lines],
        vec![pending("draw", 6, 2, 10_000, 40_000)],
        vec![],
        Some(Usd(1_000_000)),
    );
    (plan, planner)
}

/// The authored document of identity.md §14 "A plan".
fn tiny_document() -> Value {
    json!({
        "fx": "workflow/v1",
        "id": "tiny",
        "title": "Tiny",
        "inputs": {"brief": {"type": "file", "kind": "text/plain"}},
        "steps": {
            "draw": {
                "uses": "fx/image.generate@1",
                "route": "img-a@acme",
                "with": {"prompt": "${{ inputs.brief }}"},
            },
            "tint": {
                "uses": "fx/image.edit@1",
                "route": "img-a@acme",
                "with": {"image": "${{ steps.draw.outputs.image }}", "prompt": "Make it blue."},
            },
        },
    })
}

/// The digest of the 8 bytes `one\ntwo\n` (identity.md §14 "A file").
const BRIEF: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";

/// identity.md §14 "A plan": `draw` (take 2, from the takes file) and `tint`, both on
/// `img-a@acme`, with the file input `brief`.
pub(crate) fn tiny_plan() -> (Plan, Planner) {
    let mut planner = planner_for("workflows/tiny.yaml", tiny_document());
    planner.inputs.given.insert(
        "brief".into(),
        Val::File(Box::new(FileValue {
            digest: BRIEF.to_string(),
            kind: "text/plain".to_string(),
            name: "brief.txt".to_string(),
            size: 8,
            key: None,
            content: None,
            location: None,
        })),
    );
    planner.inputs.values = planner.inputs.given.clone();
    planner.takes.insert(
        "draw".into(),
        TakeChoice {
            take: 2,
            result: None,
        },
    );
    let mut draw = instance("draw#2", State::Planned, 1, &[(1, 10_000, 40_000)], None);
    draw.takes = vec![2];
    draw.routes.insert(
        "image.generate".into(),
        route("image.generate", "img-a", "acme"),
    );
    let mut tint = instance_of(
        "tint#1",
        &builtin_type("image.edit", true),
        State::Planned,
        1,
        &[(1, 10_000, 40_000)],
        None,
    );
    let mut edit = route("image.edit", "img-a", "acme");
    edit.contract = json!({"mask": true, "sizes": ["1024x1024", "1536x1024"]});
    tint.routes.insert("image.edit".into(), edit);
    tint.reads.insert("draw#2".into());
    let plan = plan_of(&planner, vec![draw, tint], vec![], vec![], None);
    (plan, planner)
}

/// The parts of a plan digest, as plain values: workflows, inputs, takes, types and the route
/// fingerprints.
pub(crate) type PlanParts = (
    Map<String, Value>,
    Map<String, Value>,
    Map<String, Value>,
    Map<String, Value>,
    BTreeSet<String>,
);

/// The parts of identity.md §14 "A plan", as plain values: workflows, inputs, takes, types and
/// the two route fingerprints.
pub(crate) fn tiny_plan_parts() -> PlanParts {
    let mut workflows = Map::new();
    workflows.insert("workflows/tiny.yaml".into(), tiny_document());
    let mut inputs = Map::new();
    inputs.insert("brief".into(), json!({"file": BRIEF}));
    let mut takes = Map::new();
    takes.insert("draw".into(), json!(2));
    let mut types = Map::new();
    types.insert("fx/image.generate@1".into(), json!("fx/image.generate@1.1"));
    types.insert("fx/image.edit@1".into(), json!("fx/image.edit@1.1"));
    let routes = BTreeSet::from([
        // image.edit with its contract, then image.generate (identity.md §14), listed out of
        // order: the plan sorts them.
        "5d727add4c417db9508eb6c7f81ff19f7d90005adf080c0702acb9077a330d6b".to_string(),
        "4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4".to_string(),
    ]);
    (workflows, inputs, takes, types, routes)
}

/// Case `local-identity`'s unversioned type: `nodes/n.py` (`x = 1\n`) declaring
/// `prompts/r.md` (`Hello\n`), identity.md §14 "A node's source".
pub(crate) fn local_identity_plan() -> (Plan, Planner) {
    let planner = planner();
    let echo = project_type(
        "nodes/n.py",
        "echo",
        "source:f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3",
        SourceDigests {
            files: BTreeMap::from([(
                "nodes/n.py".to_string(),
                "9e26bf369911c45c243c684147b23fc9e1dcfcf257d299a1c632016a6fcd33f4".to_string(),
            )]),
            resources: BTreeMap::from([(
                "prompts/r.md".to_string(),
                "66a045b452102c59d840ec097d59d9467e13a3f34f6494e539ffd32c1bb35f18".to_string(),
            )]),
        },
    );
    let a = instance_of("a#1", &echo, State::Planned, 1, &[], Some(&hex('9')));
    let plan = plan_of(&planner, vec![a], vec![], vec![], None);
    (plan, planner)
}
