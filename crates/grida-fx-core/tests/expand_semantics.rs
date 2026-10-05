//! Expansion, the semantics half, through the public API: identities, with-values, prompt files,
//! takes, judges, select, routes and prices (spec/identity.md §8, §14).
//!
//! Workflows are written in YAML and run with a scripted node host whose module mirrors the
//! conformance cases' `nodes/cases.py`. Results a run would produce are injected into the
//! expansion, so each test sees one point of a run.

use grida_fx_core::docs::lock::LockFile;
use grida_fx_core::docs::takes::{TakeChoice, Takes};
use grida_fx_core::docs::workflow::{LoadedWorkflow, parse_workflow};
use grida_fx_core::expand::{
    ExpandEnv, Expansion, Instance, NodeResult, ResultStatus, State, expand,
};
use grida_fx_core::host::FakeHost;
use grida_fx_core::money::Usd;
use grida_fx_core::registry::Registry;
use grida_fx_core::routes::RouteTable;
use grida_fx_core::val::{FileValue, Val};
use grida_fx_core::value::file_digest;
use grida_fx_core::yaml;
use grida_fx_protocol::{ClosureEntry, DescribedType, ModuleDescription, RetryMode, TypeSpec};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

const ROUTES: &str = "\
fx: routes/v1
routes:
  - { capability: image.generate, route: img-a@acme, price: { low_usd: 0.01, high_usd: 0.04 }, features: [alpha] }
  - { capability: image.edit, route: img-a@acme, price: { low_usd: 0.02, high_usd: 0.05 }, features: [mask] }
  - { capability: structured.generate, route: llm-a@acme, price: { low_usd: 0.001, high_usd: 0.002 } }
  - { capability: vision.review, route: llm-a@other, price: { low_usd: 0.002, high_usd: 0.004 } }
  - { capability: vision.review, route: vlm-c@other, price: { low_usd: 0.002, high_usd: 0.004 } }
";

const TIERED_ROUTES: &str = "\
fx: routes/v1
routes:
  - capability: video.generate
    route: clip-a@acme
    features: [first_last_frame]
    price:
      unit: second
      max_units: 10
      low_usd: 0.03
      high_usd: 0.375
      by: resolution
      tiers:
        360p: { low_usd: 0.03, high_usd: 0.0375 }
        4k: { low_usd: 0.3, high_usd: 0.375 }
";

const NOTES: &[u8] = b"one\ntwo\n";

fn type_spec(name: &str, inputs: Value, params: Value, outputs: Value, judge: bool) -> TypeSpec {
    let strings = |v: Value| -> IndexMap<String, String> {
        v.as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect()
    };
    TypeSpec {
        name: name.into(),
        description: None,
        inputs: strings(inputs),
        params: params
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        outputs: strings(outputs),
        judge,
        calls: IndexMap::new(),
        resources: Vec::new(),
        tools: Vec::new(),
        view: None,
        version: Some(1),
        retry: RetryMode::Service,
    }
}

/// The conformance cases' `nodes/cases.py`, plus a type that calls two capabilities.
fn cases_module(root: &std::path::Path) -> ModuleDescription {
    let mut paid = type_spec(
        "paid",
        json!({}),
        json!({"max_steps": {"type": "integer", "minimum": 1, "maximum": 5, "x-fx-optional": true}}),
        json!({"image": "image/png"}),
        false,
    );
    paid.calls.insert("image.generate".into(), json!(2));
    paid.calls
        .insert("structured.generate".into(), json!("max_steps"));
    let types = vec![
        (
            "shout",
            type_spec(
                "shout",
                json!({}),
                json!({"text": {"type": "string"}}),
                json!({"text": "text"}),
                false,
            ),
        ),
        (
            "join",
            type_spec(
                "join",
                json!({"parts": "text{}"}),
                json!({}),
                json!({"text": "text"}),
                false,
            ),
        ),
        (
            "lines",
            type_spec(
                "lines",
                json!({"text": "text"}),
                json!({}),
                json!({"items": "json"}),
                false,
            ),
        ),
        (
            "verdict",
            type_spec(
                "verdict",
                json!({"subject": "file"}),
                json!({"accept_take": {"type": "integer"}}),
                json!({}),
                true,
            ),
        ),
        ("paid", paid),
    ];
    ModuleDescription::Described {
        path: "nodes/cases.py".into(),
        types: types
            .into_iter()
            .map(|(attribute, spec)| DescribedType {
                attribute: attribute.into(),
                spec,
            })
            .collect(),
        closure: vec![ClosureEntry {
            label: "nodes/cases.py".into(),
            path: root.join("nodes/cases.py").to_string_lossy().into_owned(),
        }],
    }
}

/// A scratch project: files, routes, fx.yaml route defaults, takes, results and inputs.
struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    routes: String,
    defaults: IndexMap<String, String>,
    takes: Takes,
    results: IndexMap<String, NodeResult>,
    inputs: IndexMap<String, Val>,
}

impl Project {
    fn new() -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project {
            _dir: dir,
            root,
            routes: ROUTES.into(),
            defaults: [
                ("image.generate", "img-a@acme"),
                ("structured.generate", "llm-a@acme"),
                ("vision.review", "vlm-c@other"),
            ]
            .into_iter()
            .map(|(c, r)| (c.to_string(), r.to_string()))
            .collect(),
            takes: Takes::new(),
            results: IndexMap::new(),
            inputs: IndexMap::new(),
        };
        project.file("nodes/cases.py", b"# node types\n");
        project.file("fx.yaml", b"fx: project/v1\n");
        project
    }

    fn file(&self, path: &str, bytes: &[u8]) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn take(&mut self, path: &str, take: u32) {
        self.takes
            .insert(path.into(), TakeChoice { take, result: None });
    }

    fn result(&mut self, id: &str, result: NodeResult) {
        self.results.insert(id.into(), result);
    }

    fn expand(&self, workflow: &str) -> Expansion {
        self.file("workflows/case.yaml", workflow.as_bytes());
        let document = yaml::load(workflow.as_bytes(), "workflows/case.yaml")
            .unwrap_or_else(|e| panic!("{e:?}"));
        let typed =
            parse_workflow(&document, "workflows/case.yaml").unwrap_or_else(|e| panic!("{e}"));
        let loaded = Rc::new(LoadedWorkflow {
            document,
            workflow: Rc::new(typed),
            source: "workflows/case.yaml".into(),
            path: self.root.join("workflows/case.yaml"),
        });
        let routes = RouteTable::from_document(
            &yaml::load(self.routes.as_bytes(), "routes.yaml").unwrap(),
            "routes.yaml",
        )
        .unwrap();
        let mut registry = Registry::new(
            self.root.clone(),
            Vec::new(),
            LockFile::default(),
            self.defaults.clone(),
        );
        let mut host = FakeHost::new().with_module(cases_module(&self.root));
        expand(ExpandEnv {
            workflow: &loaded,
            inputs: &self.inputs,
            registry: &mut registry,
            host: &mut host,
            routes: &routes,
            takes: &self.takes,
            results: &self.results,
        })
        .unwrap()
    }
}

fn image(name: &str) -> Val {
    Val::File(Box::new(FileValue {
        digest: file_digest(name.as_bytes()),
        kind: "image/png".into(),
        name: "image.png".into(),
        size: name.len() as u64,
        key: None,
        content: None,
        location: None,
    }))
}

fn made(outputs: &[(&str, Val)], facts: Value) -> NodeResult {
    NodeResult {
        status: ResultStatus::Succeeded,
        outputs: outputs
            .iter()
            .map(|(n, v)| (n.to_string(), v.clone()))
            .collect(),
        facts: facts
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
        error: None,
    }
}

fn drew(project: &mut Project, id: &str) {
    project.result(id, made(&[("image", image(id))], json!({})));
}

fn judged(project: &mut Project, id: &str, facts: Value) {
    project.result(id, made(&[], facts));
}

fn failed(project: &mut Project, id: &str) {
    project.result(
        id,
        NodeResult {
            status: ResultStatus::Failed,
            outputs: IndexMap::new(),
            facts: IndexMap::new(),
            error: Some("boom".into()),
        },
    );
}

fn get<'a>(expansion: &'a Expansion, id: &str) -> &'a Instance {
    expansion
        .instances
        .get(id)
        .unwrap_or_else(|| panic!("no instance {id}: {:?}", ids(expansion)))
}

fn ids(expansion: &Expansion) -> Vec<&str> {
    expansion.instances.keys().map(String::as_str).collect()
}

fn state<'a>(expansion: &'a Expansion, id: &str) -> (State, Option<&'a str>) {
    let instance = get(expansion, id);
    (instance.state, instance.reason.as_deref())
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn problems(expansion: &Expansion) -> Vec<String> {
    expansion.problems.iter().map(|p| p.to_string()).collect()
}

fn usd(low: i64, high: i64, instance: &Instance) {
    assert_eq!(
        (instance.low(), instance.high()),
        (Usd(low), Usd(high)),
        "{}",
        instance.id
    );
}

// --- spec/identity.md §14 vectors ------------------------------------------------------------

#[test]
fn identity_of_a_local_step() {
    let project = Project::new();
    project.file("notes.txt", NOTES);
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Linear
steps:
  count:
    uses: ./nodes/cases.py#lines
    with: { text: ./notes.txt }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    let count = get(&expansion, "count#1");
    assert_eq!(count.ty.identity, "nodes/cases.py#lines@1");
    assert_eq!(
        count.identity.as_deref(),
        Some("f8a9e8ecdbd14337e2bd5a3729b8992a2d6a91a51e41dc62841f5bb088c34578")
    );
    assert!(count.prices.is_empty() && count.routes.is_empty());
}

const BUILTIN_STEP: &str = "\
fx: workflow/v1
id: case
title: Built-in
steps:
  draw:
    uses: fx/image.generate@1
    route: img-a@acme
    with: { prompt: A picture of 2 lines }
";

#[test]
fn identity_of_a_builtin_step() {
    let project = Project::new();
    let expansion = project.expand(BUILTIN_STEP);
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    let draw = get(&expansion, "draw#1");
    assert_eq!(
        draw.with.keys().collect::<Vec<_>>(),
        vec!["prompt", "background", "vars"],
        "defaults filled in, the optional size left out"
    );
    assert_eq!(
        draw.routes["image.generate"].fingerprint(),
        "4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4"
    );
    assert_eq!(
        draw.identity.as_deref(),
        Some("1beeb2e37052fc033ae6aabe0036b4932dbafea6b91d0b31be1b5e086064d142")
    );
    usd(10_000, 40_000, draw);
    assert_eq!(draw.prices[0].route, "img-a@acme");
    assert_eq!(draw.prices[0].calls, 1);
}

#[test]
fn identity_of_a_take_two_levels_deep() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Takes
steps:
  build:
    regenerate:
      max: 2
      until: \"${{ 1 == 2 }}\"
    steps:
      draw:
        uses: fx/image.generate@1
        route: img-a@acme
        with: { prompt: A picture of 2 lines }
",
    );
    assert_eq!(ids(&expansion), vec!["build.draw#1.1", "build.draw#2.1"]);
    let deep = get(&expansion, "build.draw#2.1");
    assert_eq!(deep.takes, vec![2, 1]);
    assert_eq!(
        deep.identity.as_deref(),
        Some("081a272804f4af28b7e96bbc810966094959bc581b42fe10f402d8bbb7d64912")
    );
}

#[test]
fn identity_of_select_plain_forms() {
    use grida_fx_core::expand::node::step_identity;
    use grida_fx_core::val::Collection;
    let file = FileValue {
        digest: file_digest(NOTES),
        kind: "text/plain".into(),
        name: "ada.txt".into(),
        size: 8,
        key: Some("ada".into()),
        content: None,
        location: None,
    };
    let collection = Collection {
        items: vec![
            ("ada".into(), Val::File(Box::new(file))),
            ("bo".into(), Val::Failed("entity['bo'].draw#1".into())),
        ],
        verdicts: IndexMap::new(),
    };
    let mut with = IndexMap::new();
    with.insert(
        "first_of".to_string(),
        Val::List(vec![
            Val::Failed("draw#1".into()),
            Val::Missing,
            Val::Collection(Box::new(collection)),
        ]),
    );
    assert_eq!(
        step_identity("fx/select@1.1", &with, &IndexMap::new(), &[1]).as_deref(),
        Some("53c409bf47a7c2f5d831fd7987ae13bcc4d97647b468b51fbc1abe0686e13a65")
    );
}

// --- with-values ------------------------------------------------------------------------------

#[test]
fn with_values_problems_and_states() {
    let mut project = Project::new();
    // An optional input that was not given is null.
    project.inputs.insert("opt".into(), Val::Null);
    failed(&mut project, "broken#1");
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: With
inputs:
  opt: { type: file, kind: text/plain, optional: true }
steps:
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: hi, volume: 11 }
  mute:
    uses: ./nodes/cases.py#shout
  count:
    uses: ./nodes/cases.py#lines
  nothing:
    uses: ./nodes/cases.py#lines
    with: { text: \"${{ inputs.opt }}\" }
  broken:
    uses: ./nodes/cases.py#shout
    with: { text: x }
  after:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.broken.outputs.text }}\" }
  joined:
    uses: ./nodes/cases.py#join
    with:
      parts: { a: \"${{ steps.broken.outputs.text }}\" }
  holes:
    uses: ./nodes/cases.py#join
    with:
      parts: { a: ./absent.txt }
",
    );
    let all = problems(&expansion);
    for want in [
        "loud.with.volume: shout has no input or setting volume",
        "mute.with: shout needs setting text",
        "count.with: lines needs input text",
    ] {
        assert!(all.iter().any(|p| p == want), "{want} in {all:?}");
    }
    assert!(
        all.iter()
            .any(|p| p.starts_with("holes.with.parts: cannot read ./absent.txt: ")),
        "{all:?}"
    );
    assert!(!get(&expansion, "loud#1").with.contains_key("volume"));
    assert_eq!(
        state(&expansion, "nothing#1"),
        (State::Absent, Some("an input it needs does not exist"))
    );
    assert_eq!(
        state(&expansion, "after#1"),
        (State::Blocked, Some("something it reads failed"))
    );
    assert_eq!(state(&expansion, "joined#1").0, State::Blocked);
    assert_eq!(
        state(&expansion, "holes#1"),
        (State::Absent, Some("an input it needs does not exist"))
    );
    // A result that failed lists the instance as failed (fx-graph-v1), with the error.
    assert_eq!(state(&expansion, "broken#1"), (State::Failed, Some("boom")));
    // Identity is computed in every state once nothing is pending.
    assert!(get(&expansion, "after#1").identity.is_some());
    assert!(get(&expansion, "after#1").reads.contains("broken#1"));
}

#[test]
fn prompt_files_render_with_vars_and_inputs() {
    let mut project = Project::new();
    project.inputs.insert("who".into(), Val::Str("Ada".into()));
    project.file(
        "prompts/a.md",
        b"<!-- a note -->\nDraw ${{ vars.n }} lamps for ${{ inputs.who }}, ${{ n }} in all.\n",
    );
    project.file("prompts/plain.md", b"<!-- x -->Just text.\n");
    project.file("prompts/bad.md", b"Draw ${{ nope }}.\n");
    project.file("prompts/pend.md", b"Draw ${{ vars.t }}.\n");
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Prompts
inputs:
  who: { type: string }
steps:
  a:
    uses: fx/image.generate@1
    with: { prompt: ./prompts/a.md, vars: { n: 2 } }
  plain:
    uses: fx/image.generate@1
    with: { prompt: ./prompts/plain.md }
  bad:
    uses: fx/image.generate@1
    with: { prompt: ./prompts/bad.md }
  none:
    uses: fx/image.generate@1
    with: { prompt: ./prompts/none.md }
  inline:
    uses: fx/image.generate@1
    with: { prompt: \"x ${{ vars.a }}\" }
  src:
    uses: ./nodes/cases.py#shout
    with: { text: t }
  pend:
    uses: fx/image.generate@1
    with: { prompt: ./prompts/pend.md, vars: { t: \"${{ steps.src.outputs.text }}\" } }
",
    );
    assert_eq!(
        get(&expansion, "a#1").with["prompt"],
        Val::Str("Draw 2 lamps for Ada, 2 in all.\n".into())
    );
    assert_eq!(
        get(&expansion, "plain#1").with["prompt"],
        Val::Str("Just text.\n".into())
    );
    let all = problems(&expansion);
    assert!(
        all.contains(
            &"bad.with.prompt: prompts/bad.md: a prompt sees vars and inputs, not 'nope'".into()
        ),
        "{all:?}"
    );
    assert!(
        all.iter()
            .any(|p| p.starts_with("none.with.prompt: cannot read ./prompts/none.md: ")),
        "{all:?}"
    );
    assert!(
        all.contains(&"inline.with.prompt: unknown name 'vars'".into()),
        "{all:?}"
    );
    assert_eq!(get(&expansion, "bad#1").with["prompt"], Val::Missing);
    let pend = get(&expansion, "pend#1");
    assert!(matches!(pend.with["prompt"], Val::Pending(_)));
    assert_eq!(pend.identity, None);
    assert_eq!(pend.waiting_on(), set(&["src#1"]));
}

// --- judges, takes and the takes file --------------------------------------------------------

const JUDGE_REGENERATE: &str = "\
fx: workflow/v1
id: case
title: A judge that regenerates
steps:
  draw:
    uses: fx/image.generate@1
    with: { prompt: a lantern }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 2 }
    on_reject: { regenerate: { max: 3, then: fail } }
  after:
    uses: ./nodes/cases.py#shout
    with: { text: \"take ${{ steps.draw.take }}\" }
";

const ONLY_IF: Option<&str> = Some("only if the take before it is rejected");
const SETTLED: Option<&str> = Some("an earlier take settled it");

/// A judge that regenerates, seen at each point of a run, with FX's rule that a failed part of a
/// mixed template blocks its reader.
#[test]
fn judge_regenerate_states() {
    let all_six = set(&[
        "check#1", "check#2", "check#3", "draw#1", "draw#2", "draw#3",
    ]);
    // No results yet.
    let mut project = Project::new();
    let expansion = project.expand(JUDGE_REGENERATE);
    assert_eq!(
        ids(&expansion),
        vec![
            "draw#1", "draw#2", "draw#3", "check#1", "check#2", "check#3", "after#1"
        ]
    );
    assert_eq!(state(&expansion, "draw#1"), (State::Planned, None));
    assert_eq!(state(&expansion, "draw#2"), (State::Maybe, ONLY_IF));
    assert_eq!(state(&expansion, "draw#3"), (State::Maybe, ONLY_IF));
    assert_eq!(state(&expansion, "check#1").0, State::Planned);
    assert_eq!(state(&expansion, "check#2"), (State::Maybe, None));
    assert_eq!(state(&expansion, "check#3"), (State::Maybe, None));
    assert_eq!(get(&expansion, "check#2").judges.as_deref(), Some("draw#2"));
    assert_eq!(get(&expansion, "draw#3").judged_by, vec!["check#3"]);
    assert_eq!(get(&expansion, "check#1").waiting_on(), set(&["draw#1"]));
    let after = get(&expansion, "after#1");
    assert_eq!(after.identity, None);
    assert_eq!(after.waiting_on(), all_six);
    for take in 1..=3 {
        usd(10_000, 40_000, get(&expansion, &format!("draw#{take}")));
        assert!(get(&expansion, &format!("check#{take}")).prices.is_empty());
    }
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));

    // Take 1 drawn and rejected.
    drew(&mut project, "draw#1");
    judged(&mut project, "check#1", json!({"verdict": "reject"}));
    let expansion = project.expand(JUDGE_REGENERATE);
    assert_eq!(state(&expansion, "draw#1").0, State::Done);
    assert_eq!(state(&expansion, "draw#2").0, State::Planned);
    assert_eq!(state(&expansion, "draw#3"), (State::Maybe, ONLY_IF));
    assert_eq!(state(&expansion, "check#2").0, State::Planned);
    assert_eq!(state(&expansion, "check#3").0, State::Maybe);
    assert_eq!(get(&expansion, "after#1").waiting_on(), all_six);

    // Take 1 accepted.
    judged(&mut project, "check#1", json!({"verdict": "accept"}));
    let expansion = project.expand(JUDGE_REGENERATE);
    assert_eq!(state(&expansion, "draw#2"), (State::Absent, SETTLED));
    assert_eq!(state(&expansion, "draw#3"), (State::Absent, SETTLED));
    assert_eq!(state(&expansion, "check#2").0, State::Absent);
    assert_eq!(state(&expansion, "check#3").0, State::Absent);
    let after = get(&expansion, "after#1");
    assert_eq!(after.with["text"], Val::Str("take 1".into()));
    assert!(after.identity.is_some());

    // The judge of take 1 failed.
    failed(&mut project, "check#1");
    let expansion = project.expand(JUDGE_REGENERATE);
    assert_eq!(
        state(&expansion, "draw#2"),
        (State::Absent, Some("the take before it failed"))
    );
    assert_eq!(state(&expansion, "draw#3"), (State::Absent, SETTLED));
    assert_eq!(state(&expansion, "check#1"), (State::Failed, Some("boom")));
    assert_eq!(
        state(&expansion, "after#1"),
        (State::Blocked, Some("something it reads failed"))
    );

    // Three takes, all rejected, then fail.
    for take in 1..=3 {
        drew(&mut project, &format!("draw#{take}"));
        judged(
            &mut project,
            &format!("check#{take}"),
            json!({"verdict": "reject"}),
        );
    }
    let expansion = project.expand(JUDGE_REGENERATE);
    for id in [
        "draw#1", "draw#2", "draw#3", "check#1", "check#2", "check#3",
    ] {
        assert_eq!(state(&expansion, id).0, State::Done, "{id}");
    }
    assert_eq!(
        get(&expansion, "after#1").with["text"],
        Val::Failed("draw#3".into())
    );
    assert_eq!(state(&expansion, "after#1").0, State::Blocked);
}

#[test]
fn a_judge_declared_before_its_subject() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Judge first
steps:
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 2 }
    on_reject: { regenerate: { max: 2 } }
  draw:
    uses: fx/image.generate@1
    with: { prompt: a lantern }
",
    );
    assert_eq!(
        ids(&expansion),
        vec!["draw#1", "draw#2", "check#1", "check#2"]
    );
    assert_eq!(state(&expansion, "draw#2"), (State::Maybe, ONLY_IF));
    assert_eq!(state(&expansion, "check#2").0, State::Maybe);
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
}

fn outcome(on_reject: &str, takes: u32) -> Val {
    let mut project = Project::new();
    let scores = [5, 9, 7];
    for take in 1..=takes {
        drew(&mut project, &format!("draw#{take}"));
        judged(
            &mut project,
            &format!("check#{take}"),
            json!({"verdict": "reject", "score": scores[take as usize - 1]}),
        );
    }
    let expansion = project.expand(&format!(
        "\
fx: workflow/v1
id: case
title: Outcomes
steps:
  draw:
    uses: fx/image.generate@1
    with: {{ prompt: a lantern }}
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: {{ subject: \"${{{{ steps.draw.outputs.image }}}}\", accept_take: 9 }}
    on_reject: {on_reject}
  after:
    uses: ./nodes/cases.py#shout
    with: {{ text: \"${{{{ steps.draw.take }}}}\" }}
"
    ));
    assert_eq!(
        expansion
            .instances
            .keys()
            .filter(|id| id.starts_with("draw#"))
            .count(),
        takes as usize,
        "{on_reject}"
    );
    get(&expansion, "after#1").with["text"].clone()
}

/// What downstream steps receive when every take is rejected, for each `on_reject`.
#[test]
fn rejection_outcomes() {
    assert_eq!(outcome("fail", 1), Val::Failed("draw#1".into()));
    assert_eq!(outcome("continue", 1), Val::Number(1.0));
    assert_eq!(outcome("skip", 1), Val::Missing);
    assert_eq!(
        outcome("{ regenerate: { max: 2, then: continue } }", 2),
        Val::Number(2.0)
    );
    assert_eq!(
        outcome("{ regenerate: { max: 2, then: skip } }", 2),
        Val::Missing
    );
    assert_eq!(
        outcome("{ regenerate: { max: 3 } }", 3),
        Val::Failed("draw#3".into())
    );
    assert_eq!(
        outcome(
            "{ regenerate: { max: 3, then: { keep_best: { by: score, order: highest } } } }",
            3
        ),
        Val::Number(2.0)
    );
}

const PICK_FIRST_ACCEPTED: &str = "\
fx: workflow/v1
id: case
title: Takes
steps:
  draw:
    uses: fx/image.generate@1
    takes: 3
    pick: first_accepted
    with: { prompt: a lantern }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 2 }
  after:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.draw.take }}\" }
";

#[test]
fn takes_with_pick_first_accepted() {
    let mut project = Project::new();
    let expansion = project.expand(PICK_FIRST_ACCEPTED);
    for take in 1..=3 {
        assert_eq!(state(&expansion, &format!("draw#{take}")).0, State::Planned);
        assert_eq!(
            state(&expansion, &format!("check#{take}")).0,
            State::Planned
        );
    }
    assert_eq!(
        get(&expansion, "after#1").waiting_on(),
        set(&[
            "check#1", "check#2", "check#3", "draw#1", "draw#2", "draw#3"
        ])
    );
    for take in 1..=3 {
        drew(&mut project, &format!("draw#{take}"));
    }
    judged(&mut project, "check#1", json!({"verdict": "reject"}));
    judged(&mut project, "check#2", json!({"verdict": "accept"}));
    let expansion = project.expand(PICK_FIRST_ACCEPTED);
    assert_eq!(get(&expansion, "after#1").with["text"], Val::Number(2.0));
}

#[test]
fn the_takes_file_shifts_takes() {
    // A plain step rerolled to take 3: only take 3 exists, and downstream reads it.
    let mut project = Project::new();
    project.take("draw", 3);
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Reroll
steps:
  draw:
    uses: fx/image.generate@1
    with: { prompt: a lantern }
  after:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.draw.take }}\" }
",
    );
    assert_eq!(ids(&expansion), vec!["draw#3", "after#1"]);
    assert_eq!(get(&expansion, "after#1").with["text"], Val::Number(3.0));

    // A judged sequence of max 3 starting at take 2.
    let mut project = Project::new();
    project.take("draw", 2);
    let expansion = project.expand(JUDGE_REGENERATE);
    assert_eq!(
        ids(&expansion),
        vec![
            "draw#2", "draw#3", "draw#4", "check#2", "check#3", "check#4", "after#1"
        ]
    );
    assert_eq!(state(&expansion, "draw#2").0, State::Planned);
    assert_eq!(state(&expansion, "draw#3"), (State::Maybe, ONLY_IF));

    // `takes: 3` with a pick of take 5 keeps five takes, all planned and priced.
    let mut project = Project::new();
    project.take("draw", 5);
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Picked
steps:
  draw:
    uses: fx/image.generate@1
    takes: 3
    with: { prompt: a lantern }
  after:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.draw.take }}\" }
",
    );
    for take in 1..=5 {
        let draw = get(&expansion, &format!("draw#{take}"));
        assert_eq!(draw.state, State::Planned);
        usd(10_000, 40_000, draw);
    }
    assert_eq!(get(&expansion, "after#1").with["text"], Val::Number(5.0));
}

#[test]
fn judge_feedback_reaches_the_next_take() {
    const FEEDBACK: &str = "\
fx: workflow/v1
id: case
title: Feedback
steps:
  draw:
    uses: fx/image.generate@1
    with: { prompt: a lantern, vars: { fb: \"${{ feedback }}\" } }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 2 }
    on_reject: { regenerate: { max: 2, feedback: true } }
";
    let mut project = Project::new();
    let expansion = project.expand(FEEDBACK);
    let Val::Object(vars) = &get(&expansion, "draw#1").with["vars"] else {
        panic!("vars")
    };
    assert_eq!(vars["fb"], Val::Missing);
    let draw2 = get(&expansion, "draw#2");
    assert_eq!(draw2.identity, None);
    assert_eq!(draw2.waiting_on(), set(&["check#1"]));
    drew(&mut project, "draw#1");
    judged(
        &mut project,
        "check#1",
        json!({"verdict": "reject", "note": "darker"}),
    );
    let expansion = project.expand(FEEDBACK);
    let Val::Object(vars) = &get(&expansion, "draw#2").with["vars"] else {
        panic!("vars")
    };
    let Val::Object(said) = &vars["fb"] else {
        panic!("feedback")
    };
    assert_eq!(said.keys().collect::<Vec<_>>(), vec!["check"]);
    assert!(get(&expansion, "draw#2").identity.is_some());
}

#[test]
fn a_judge_whose_type_is_no_judge() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Not a judge
steps:
  draw:
    uses: fx/image.generate@1
    with: { prompt: a lantern }
  loud:
    uses: ./nodes/cases.py#shout
    judges: draw
    with: { text: x }
  stray:
    uses: ./nodes/cases.py#verdict
    judges: elsewhere
    with: { subject: ./absent.png, accept_take: 1 }
",
    );
    let all = problems(&expansion);
    assert!(
        all.contains(
            &"loud.judges: ./nodes/cases.py#shout is not a judge: it reports no verdict".into()
        ),
        "{all:?}"
    );
    assert!(
        all.contains(&"stray.judges: judges: names a sibling, not elsewhere".into()),
        "{all:?}"
    );
    assert_eq!(get(&expansion, "loud#1").judges.as_deref(), Some("draw#1"));
    assert!(get(&expansion, "stray#1").judges.is_none());
}

// --- select -----------------------------------------------------------------------------------

const CONDITIONS_SELECT: &str = "\
fx: workflow/v1
id: case
title: Conditions and select
inputs:
  mode: { type: string, enum: [a, b], default: a }
steps:
  base:
    uses: fx/image.generate@1
    with: { prompt: base }
  check:
    uses: ./nodes/cases.py#verdict
    judges: base
    with: { subject: \"${{ steps.base.outputs.image }}\", accept_take: 1 }
    on_reject: continue
  only_b:
    if: ${{ inputs.mode == 'b' }}
    uses: ./nodes/cases.py#shout
    with: { text: b }
  fix:
    if: ${{ steps.check.facts.verdict == 'reject' }}
    uses: fx/image.generate@1
    with: { prompt: fixed }
  chosen:
    uses: fx/select@1
    with:
      first_of:
        - ${{ steps.fix.outputs.image }}
        - ${{ steps.base.outputs.image }}
";

fn first_of(expansion: &Expansion) -> Vec<Val> {
    match &get(expansion, "chosen#1").with["first_of"] {
        Val::List(items) => items.clone(),
        other => panic!("first_of is {other:?}"),
    }
}

#[test]
fn conditions_and_select() {
    let mut project = Project::new();
    project.inputs.insert("mode".into(), Val::Str("a".into()));
    // Nothing has run: fix is maybe, and select waits on both candidates.
    let expansion = project.expand(CONDITIONS_SELECT);
    assert_eq!(
        ids(&expansion),
        vec!["base#1", "check#1", "fix#1", "chosen#1"]
    );
    assert_eq!(state(&expansion, "fix#1").0, State::Maybe);
    assert!(get(&expansion, "fix#1").reads.is_empty());
    let chosen = get(&expansion, "chosen#1");
    assert_eq!(chosen.state, State::Planned);
    assert_eq!(chosen.waiting_on(), set(&["base#1", "check#1", "fix#1"]));
    assert!(chosen.prices.is_empty() && chosen.routes.is_empty());
    assert_eq!(chosen.ty.identity, "fx/select@1.1");

    // An accepted base: fix is left out, select reads the base.
    drew(&mut project, "base#1");
    judged(&mut project, "check#1", json!({"verdict": "accept"}));
    let expansion = project.expand(CONDITIONS_SELECT);
    assert_eq!(ids(&expansion), vec!["base#1", "check#1", "chosen#1"]);
    assert_eq!(first_of(&expansion), vec![Val::Missing, image("base#1")]);
    assert!(get(&expansion, "chosen#1").identity.is_some());
    assert_eq!(get(&expansion, "chosen#1").reads, set(&["base#1"]));

    // A rejected base is read as missing by select, though on_reject is continue.
    judged(&mut project, "check#1", json!({"verdict": "reject"}));
    let expansion = project.expand(CONDITIONS_SELECT);
    assert_eq!(state(&expansion, "fix#1").0, State::Planned);
    let items = first_of(&expansion);
    assert!(matches!(&items[0], Val::Pending(p) if p.refs == set(&["fix#1"])));
    assert_eq!(items[1], Val::Missing);
    assert_eq!(get(&expansion, "chosen#1").waiting_on(), set(&["fix#1"]));

    // A failed fix becomes missing too, and select stays planned.
    failed(&mut project, "fix#1");
    let expansion = project.expand(CONDITIONS_SELECT);
    assert_eq!(first_of(&expansion), vec![Val::Missing, Val::Missing]);
    assert_eq!(state(&expansion, "fix#1").0, State::Failed);
    assert_eq!(state(&expansion, "chosen#1").0, State::Planned);
    assert!(get(&expansion, "chosen#1").identity.is_some());
}

#[test]
fn reading_a_select_result_reads_its_value() {
    let mut project = Project::new();
    project.inputs.insert("mode".into(), Val::Str("a".into()));
    drew(&mut project, "base#1");
    judged(&mut project, "check#1", json!({"verdict": "accept"}));
    project.result(
        "chosen#1",
        made(&[("value", image("base#1"))], json!({"chosen": 1})),
    );
    let workflow = format!(
        "{CONDITIONS_SELECT}  after:\n    uses: fx/image.edit@1\n    route: img-a@acme\n    with: {{ image: \"${{{{ steps.chosen.outputs.anything }}}}\", prompt: x }}\n"
    );
    let expansion = project.expand(&workflow);
    assert_eq!(get(&expansion, "after#1").with["image"], image("base#1"));
}

// --- routes and prices ------------------------------------------------------------------------

#[test]
fn refusals_of_routes_requires_and_independence() {
    let mut project = Project::new();
    project.file("schema.json", b"{}");
    project.inputs.insert(
        "names".into(),
        Val::List(vec![Val::Str("ada".into()), Val::Str("bo".into())]),
    );
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Everything a plan refuses
inputs:
  names: { type: list, items: { type: string } }
steps:
  draw:
    uses: fx/image.generate@1
    requires: [mask]
    with: { prompt: x }
  write:
    uses: fx/structured.generate@1
    with: { prompt: x, schema: ./schema.json }
  review:
    uses: fx/vision.review@1
    route: llm-a@other
    independent_of: write
    judges: write
    with: { question: \"right?\" }
  lost:
    uses: fx/image.generate@1
    route: img-z@nowhere
    with: { prompt: x }
",
    );
    assert_eq!(
        problems(&expansion),
        vec![
            "draw.requires: img-a@acme does not support mask",
            "review.independent_of: shares the model llm-a with write; route one of them to a \
             different model",
            "lost.route: no route img-z@nowhere serves image.generate (known routes: img-a@acme)",
        ]
    );
    assert_eq!(
        ids(&expansion),
        vec!["draw#1", "write#1", "review#1", "lost#1"]
    );
    // A route that failed leaves no route, no price, and an identity over no routes.
    let lost = get(&expansion, "lost#1");
    assert!(lost.routes.is_empty() && lost.prices.is_empty());
    assert!(lost.identity.is_some());
    // A missing feature still binds the route.
    usd(10_000, 40_000, get(&expansion, "draw#1"));
    assert_eq!(
        get(&expansion, "review#1").routes["vision.review"].id(),
        "llm-a@other"
    );
    usd(1_000, 2_000, get(&expansion, "write#1"));
    usd(2_000, 4_000, get(&expansion, "review#1"));
}

#[test]
fn route_problems() {
    let mut project = Project::new();
    project.file("pic.png", b"png");
    project.file("schema.json", b"{}");
    // fx.yaml's routes are read as written; a bad id is refused where it is bound.
    project
        .defaults
        .insert("structured.generate".into(), "nonsense".into());
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Routes
steps:
  edit:
    uses: fx/image.edit@1
    with: { image: ./pic.png, prompt: x }
  odd:
    uses: fx/structured.generate@1
    with: { prompt: x, schema: ./schema.json }
  free:
    uses: ./nodes/cases.py#shout
    route: img-a@acme
    requires: [alpha]
    with: { text: x }
  early:
    uses: fx/image.generate@1
    at: plan
    with: { prompt: x }
",
    );
    let all = problems(&expansion);
    for want in [
        "edit.route: no route for image.edit: set route: on the step or routes.image.edit in \
         fx.yaml",
        "odd.route: a route is model@provider, not 'nonsense'",
        "free.route: shout makes no paid call",
        "free.requires: shout makes no paid call to check",
        "early.at: an at: plan step makes no paid call",
    ] {
        assert!(all.iter().any(|p| p == want), "{want} in {all:?}");
    }
    assert!(get(&expansion, "edit#1").routes.is_empty());
    assert!(get(&expansion, "early#1").at_plan);
}

#[test]
fn a_type_that_calls_two_capabilities() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Two capabilities
steps:
  a:
    uses: ./nodes/cases.py#paid
    route: vlm-c@other
    requires: [alpha]
  b:
    uses: ./nodes/cases.py#paid
    with: { max_steps: 7 }
",
    );
    // route: is ignored for a type that calls several capabilities; requires is checked
    // against each capability's route.
    assert_eq!(
        problems(&expansion),
        vec!["a.requires: llm-a@acme does not support alpha"]
    );
    let a = get(&expansion, "a#1");
    assert_eq!(
        a.routes.keys().collect::<Vec<_>>(),
        vec!["image.generate", "structured.generate"]
    );
    // 2 × (0.01–0.04) + 5 (the maximum) × (0.001–0.002).
    usd(25_000, 90_000, a);
    assert_eq!(
        a.prices.iter().map(|p| p.calls).collect::<Vec<_>>(),
        vec![2, 5]
    );
    // A given count is used as given, above its maximum too.
    let b = get(&expansion, "b#1");
    assert_eq!(
        b.prices.iter().map(|p| p.calls).collect::<Vec<_>>(),
        vec![2, 7]
    );
    usd(27_000, 94_000, b);
}

#[test]
fn tiered_prices() {
    let mut project = Project::new();
    project.routes = TIERED_ROUTES.into();
    project.defaults = [("video.generate".to_string(), "clip-a@acme".to_string())]
        .into_iter()
        .collect();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Tiered price
steps:
  small:
    uses: fx/video.generate@1
    requires: [first_last_frame]
    with:
      prompt: a lantern sways
      duration: 3
      resolution: 360p
  large:
    uses: fx/video.generate@1
    with:
      prompt: a lantern sways
      duration: 8
      resolution: 4k
  unknown:
    uses: fx/video.generate@1
    with:
      prompt: a lantern sways
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    usd(90_000, 112_500, get(&expansion, "small#1"));
    usd(2_400_000, 3_000_000, get(&expansion, "large#1"));
    usd(0, 3_750_000, get(&expansion, "unknown#1"));
}

#[test]
fn independent_of_a_group_never_conflicts() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Independence
steps:
  g:
    steps:
      inner:
        uses: fx/image.generate@1
        with: { prompt: x }
  solo:
    uses: fx/image.generate@1
    independent_of: [g, solo2, nowhere]
    with: { prompt: x }
  solo2:
    uses: fx/image.generate@1
    with: { prompt: x }
",
    );
    assert_eq!(
        problems(&expansion),
        vec![
            "solo.independent_of: shares the model img-a with solo2; route one of them to a \
             different model",
            "solo.independent_of: no step nowhere",
        ]
    );
    // Identical work in two places has one identity.
    assert_eq!(
        get(&expansion, "solo#1").identity,
        get(&expansion, "solo2#1").identity
    );
}

// --- needs and cycles -------------------------------------------------------------------------

#[test]
fn needs_and_cycles() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Needs
steps:
  a:
    uses: ./nodes/cases.py#shout
    needs: [b]
    with: { text: a }
  b:
    uses: ./nodes/cases.py#shout
    needs: [a, c, c, nowhere]
    with: { text: b }
  c:
    uses: ./nodes/cases.py#shout
    with: { text: c }
  e:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.e.outputs.text }}\" }
",
    );
    // A needs: cycle is silent; ids keep their first-seen order, deduplicated.
    assert_eq!(get(&expansion, "b#1").needs, vec!["c#1"]);
    assert_eq!(get(&expansion, "a#1").needs, vec!["b#1"]);
    let all = problems(&expansion);
    assert!(all.contains(&"b.needs: no step nowhere".into()), "{all:?}");
    // FX: a node reading itself is refused.
    assert!(
        all.contains(&"e.with.text: e refers back to itself".into()),
        "{all:?}"
    );
    assert_eq!(get(&expansion, "e#1").with["text"], Val::Missing);
}

#[test]
fn work_held_for_an_unfinished_step_reads_it_as_pending() {
    let project = Project::new();
    // `first` reads a1, a judge of draw, so a1 expands draw first; draw's other judge a2 reads
    // a1, which still waits for draw: a2 is held until a1 and `first` have their instances.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: A judge reads a judge
steps:
  first:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.a1.facts.take }}\" }
  draw:
    uses: fx/image.generate@1
    with: { prompt: x }
  a1:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 1 }
  a2:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: \"${{ steps.a1.facts.take }}\" }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    assert_eq!(ids(&expansion), ["draw#1", "a1#1", "first#1", "a2#1"]);
    assert_eq!(get(&expansion, "draw#1").judged_by, ["a1#1", "a2#1"]);
    assert_eq!(
        get(&expansion, "a2#1").waiting_on(),
        set(&["a1#1", "draw#1"])
    );
    assert_eq!(get(&expansion, "a2#1").identity, None);
    assert_eq!(get(&expansion, "first#1").waiting_on(), set(&["a1#1"]));

    // `independent_of` is a check, not a read: c reads b, which waits for a.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Independence
steps:
  b:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.a.outputs.image }}\" }
  a:
    uses: fx/image.generate@1
    independent_of: c
    with: { prompt: x }
  c:
    uses: fx/image.generate@1
    with: { prompt: \"${{ steps.b.outputs.text }}\" }
",
    );
    // The check still runs, once b exists.
    assert_eq!(
        problems(&expansion),
        ["a.independent_of: shares the model img-a with c; route one of them to a different model"]
    );
    assert_eq!(ids(&expansion), ["a#1", "b#1", "c#1"]);
    assert_eq!(get(&expansion, "c#1").waiting_on(), set(&["b#1"]));

    // A regenerating group's `until` reads one judge while another judge reads it.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Until
steps:
  build:
    steps:
      draw:
        uses: fx/image.generate@1
        with: { prompt: m }
      audit:
        uses: ./nodes/cases.py#verdict
        judges: draw
        with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 1 }
      audit2:
        uses: ./nodes/cases.py#verdict
        judges: draw
        with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: \"${{ steps.audit.facts.take }}\" }
        on_reject: continue
    regenerate: { max: 2, until: \"${{ steps.audit.facts.verdict == 'accept' }}\" }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    assert!(
        get(&expansion, "build.audit2#1.1")
            .waiting_on()
            .contains("build.audit#1.1")
    );

    // A `needs:` waits for the instances of what it names, not for their judges.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Needs
steps:
  b:
    uses: ./nodes/cases.py#shout
    needs: [a]
    with: { text: b }
  a:
    uses: fx/image.generate@1
    with: { prompt: x }
  j:
    uses: ./nodes/cases.py#verdict
    judges: a
    with: { subject: \"${{ steps.a.outputs.image }}\", accept_take: \"${{ steps.b.outputs.text }}\" }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    assert_eq!(get(&expansion, "b#1").needs, ["a#1"]);
    assert_eq!(get(&expansion, "j#1").waiting_on(), set(&["a#1", "b#1"]));
}

#[test]
fn reading_a_step_waits_for_its_held_judges() {
    let project = Project::new();
    // x reads a1 (which expands draw while x is unfinished, so draw's judges are held), then
    // draw's result, which waits for every judge of draw: they are expanded right then.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Settled
steps:
  x:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.a1.facts.take }} ${{ steps.draw.outputs.image }}\" }
  draw:
    uses: fx/image.generate@1
    with: { prompt: x }
  a1:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: 1 }
  a2:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: \"${{ steps.draw.outputs.image }}\", accept_take: \"${{ steps.a1.facts.take }}\" }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    assert_eq!(ids(&expansion), ["draw#1", "a1#1", "a2#1", "x#1"]);
    assert_eq!(
        get(&expansion, "x#1").waiting_on(),
        set(&["a1#1", "a2#1", "draw#1"])
    );
}

#[test]
fn a_judge_reading_a_reader_of_its_subject_is_a_cycle() {
    let project = Project::new();
    // e reads d's result, which waits for d's judge j1, which reads e.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: A real cycle
steps:
  e:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.d.outputs.image }}\" }
  d:
    uses: fx/image.generate@1
    with: { prompt: x }
  j1:
    uses: ./nodes/cases.py#verdict
    judges: d
    with: { subject: \"${{ steps.d.outputs.image }}\", accept_take: \"${{ steps.e.outputs.text }}\" }
",
    );
    assert_eq!(
        problems(&expansion),
        ["j1.with.accept_take: e refers back to itself"]
    );
    // The same through a regenerating group's `until`.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: A real cycle in a group
steps:
  build:
    steps:
      d:
        uses: fx/image.generate@1
        with: { prompt: x }
      j:
        uses: ./nodes/cases.py#verdict
        judges: d
        with: { subject: \"${{ steps.d.outputs.image }}\", accept_take: \"${{ steps.k.outputs.text }}\" }
      k:
        uses: ./nodes/cases.py#shout
        with: { text: \"${{ steps.d.outputs.image }}\" }
    regenerate: { max: 2, until: \"${{ steps.k.outputs.text == 'x' }}\" }
",
    );
    assert_eq!(
        problems(&expansion),
        ["build.j.with.accept_take: build.k refers back to itself"]
    );
    // The same when the judges are settled on trial: e reaches d through its judge k, so d's
    // judges are held while e is under way; n, begun after that, reads d and settles them; j
    // reads e, which waits for j through n. e began before the judges were held: a real cycle.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: A real cycle through a trial
steps:
  e:
    uses: ./nodes/cases.py#shout
    with: { text: \"${{ steps.k.take }} ${{ steps.n.outputs.text }}\" }
  d:
    uses: ./nodes/cases.py#shout
    with: { text: x }
  k:
    uses: ./nodes/cases.py#verdict
    judges: d
    with: { subject: \"${{ steps.d.outputs.text }}\", accept_take: 1 }
  n:
    uses: ./nodes/cases.py#join
    with: { parts: { t: \"${{ steps.d.outputs.text }}\" } }
  j:
    uses: ./nodes/cases.py#verdict
    judges: d
    with: { subject: \"${{ steps.d.outputs.text }}\", accept_take: \"${{ steps.e.outputs.text }}\" }
",
    );
    assert_eq!(
        problems(&expansion),
        ["j.with.accept_take: e refers back to itself"]
    );
    assert_eq!(ids(&expansion), ["d#1", "k#1", "j#1", "n#1", "e#1"]);
    assert_eq!(
        get(&expansion, "n#1").waiting_on(),
        set(&["d#1", "j#1", "k#1"])
    );
}

// --- a judge's evidence ----------------------------------------------------------------------
//
// A judge that judges the first step of a chain and reads its last reads its own evidence: the
// chain reads the judged take as it is, never waiting for the judge. When the judge expands first
// (a take whose `until` reads it) the chain is reached inside it. When a step outside reads the
// group while its `until` waits, the next take's members expand in declaration order instead: the
// judged step first, its judge held (the outside reader is unfinished), then the next member
// reads the judged step, which settles the judge, which reads that member back. Both must plan
// as stage-gen's engine plans them: no cycle, the same graph.

/// A step joining one text, read from `SOURCE`.
const JOIN: &str =
    "{ uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.SOURCE.outputs.text }}\" } } }";

/// A regenerating group: `STEPS`, then a judge of `one` reading `LAST`; `after` reads `LAST`.
const EVIDENCE: &str = "\
fx: workflow/v1
id: case
title: Evidence
steps:
  part:
    steps:
STEPS      review:
        uses: ./nodes/cases.py#verdict
        judges: one
        with: { subject: \"${{ steps.LAST.outputs.text }}\", accept_take: 1 }
    regenerate: { max: 2, until: \"${{ steps.review.facts.verdict == 'accept' }}\" }
  after:
    uses: ./nodes/cases.py#join
    with: { parts: { t: \"${{ steps.part.LAST.outputs.text }}\" } }
";

const CHAIN: [&str; 5] = ["one", "two", "three", "four", "five"];

/// [`EVIDENCE`] with a chain of `length` steps, each reading the one before.
fn evidence_chain(length: usize) -> String {
    let chain = &CHAIN[..length];
    let mut steps =
        String::from("      one: { uses: ./nodes/cases.py#shout, with: { text: x } }\n");
    for pair in chain.windows(2) {
        steps += &format!("      {}: {}\n", pair[1], JOIN.replace("SOURCE", pair[0]));
    }
    EVIDENCE
        .replace("STEPS", &steps)
        .replace("LAST", chain[length - 1])
}

/// The ids, as a set.
fn id_set(ids: &[String]) -> BTreeSet<String> {
    ids.iter().cloned().collect()
}

#[test]
fn a_judge_reads_its_evidence_in_every_take() {
    let project = Project::new();
    // Two steps is the rigged character's `assemble`, three its `part` (and the smallest case
    // reported), four and five longer chains.
    for length in 2..=5 {
        let expansion = project.expand(&evidence_chain(length));
        assert!(
            expansion.problems.is_empty(),
            "{length}: {:?}",
            problems(&expansion)
        );
        let chain = &CHAIN[..length];
        let mut expected = Vec::new();
        for take in 1..=2 {
            for name in chain.iter().chain(&["review"]) {
                expected.push(format!("part.{name}#{take}.1"));
            }
        }
        expected.push("after#1".to_string());
        assert_eq!(ids(&expansion), expected, "{length}");
        for take in 1..=2 {
            let id = |name: &str| format!("part.{name}#{take}.1");
            // Each link reads the one before as it is, the judged take included.
            for pair in chain.windows(2) {
                assert_eq!(
                    get(&expansion, &id(pair[1])).waiting_on(),
                    id_set(&[id(pair[0])]),
                    "{length}, take {take}"
                );
            }
            let review = get(&expansion, &id("review"));
            assert_eq!(review.judges, Some(id("one")), "{length}, take {take}");
            assert_eq!(review.waiting_on(), id_set(&[id(chain[length - 1])]));
            assert_eq!(get(&expansion, &id("one")).judged_by, [id("review")]);
        }
    }

    // While running: take 2's judged step has a result, and its evidence reads it.
    let mut project = Project::new();
    project.result("part.one#2.1", made(&[("text", image("one"))], json!({})));
    let expansion = project.expand(&evidence_chain(3));
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    let two = get(&expansion, "part.two#2.1");
    assert_eq!(two.reads, set(&["part.one#2.1"]));
    assert!(two.waiting_on().is_empty());
    assert!(two.identity.is_some());
    assert_eq!(
        get(&expansion, "part.review#2.1").waiting_on(),
        set(&["part.three#2.1"])
    );
}

#[test]
fn a_judge_reads_its_evidence_in_a_nested_regenerating_group() {
    let project = Project::new();
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Nested
steps:
  outer:
    steps:
      part:
        steps:
          one: { uses: ./nodes/cases.py#shout, with: { text: x } }
          two: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.one.outputs.text }}\" } } }
          three: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.two.outputs.text }}\" } } }
          review:
            uses: ./nodes/cases.py#verdict
            judges: one
            with: { subject: \"${{ steps.three.outputs.text }}\", accept_take: 1 }
        regenerate: { max: 2, until: \"${{ steps.review.facts.verdict == 'accept' }}\" }
      done: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.part.three.outputs.text }}\" } } }
      audit:
        uses: ./nodes/cases.py#verdict
        judges: done
        with: { subject: \"${{ steps.done.outputs.text }}\", accept_take: 1 }
    regenerate: { max: 2, until: \"${{ steps.audit.facts.verdict == 'accept' }}\" }
  after: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.outer.done.outputs.text }}\" } } }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    let mut expected = Vec::new();
    for outer in 1..=2 {
        for inner in 1..=2 {
            for name in ["one", "two", "three", "review"] {
                expected.push(format!("outer.part.{name}#{outer}.{inner}.1"));
            }
        }
        expected.push(format!("outer.done#{outer}.1"));
        expected.push(format!("outer.audit#{outer}.1"));
    }
    expected.push("after#1".to_string());
    assert_eq!(ids(&expansion), expected);
    for outer in 1..=2 {
        for inner in 1..=2 {
            let id = |name: &str| format!("outer.part.{name}#{outer}.{inner}.1");
            assert_eq!(
                get(&expansion, &id("two")).waiting_on(),
                id_set(&[id("one")])
            );
            assert_eq!(
                get(&expansion, &id("three")).waiting_on(),
                id_set(&[id("two")])
            );
            let review = get(&expansion, &id("review"));
            assert_eq!(review.judges, Some(id("one")));
            assert_eq!(review.waiting_on(), id_set(&[id("three")]));
        }
    }
}

#[test]
fn a_judge_reads_its_evidence_in_the_rigged_character_shape() {
    let project = Project::new();
    // The guide's rigged character (docs/guide/examples/rigged-character), its types made local:
    // parts that regenerate inside a build that regenerates, each part's review judging its mesh
    // by renders of the measured mesh, and an assembly whose review judges the oriented mesh by
    // its renders.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Rigged
steps:
  body:
    steps:
      part:
        for_each: [head, torso]
        key: ${{ item }}
        steps:
          mesh: { uses: ./nodes/cases.py#shout, with: { text: \"${{ item }}\" } }
          measure: { uses: ./nodes/cases.py#join, with: { parts: { m: \"${{ steps.mesh.outputs.text }}\" } } }
          renders: { uses: ./nodes/cases.py#join, with: { parts: { m: \"${{ steps.measure.outputs.text }}\" } } }
          review:
            uses: ./nodes/cases.py#verdict
            judges: mesh
            with: { subject: \"${{ steps.renders.outputs.text }}\", accept_take: 1 }
        regenerate: { max: 2, until: \"${{ steps.review.facts.verdict == 'accept' }}\" }
      assemble:
        steps:
          orient: { uses: ./nodes/cases.py#join, with: { parts: \"${{ steps.part.*.measure.outputs.text }}\" } }
          renders: { uses: ./nodes/cases.py#join, with: { parts: { o: \"${{ steps.orient.outputs.text }}\" } } }
          review:
            uses: ./nodes/cases.py#verdict
            judges: orient
            with: { subject: \"${{ steps.renders.outputs.text }}\", accept_take: 1 }
        regenerate: { max: 2, until: \"${{ steps.review.facts.verdict == 'accept' }}\" }
      rig: { uses: ./nodes/cases.py#join, with: { parts: { o: \"${{ steps.assemble.orient.outputs.text }}\" } } }
      audit:
        uses: ./nodes/cases.py#verdict
        judges: rig
        with: { subject: \"${{ steps.rig.outputs.text }}\", accept_take: 1 }
    regenerate: { max: 2, until: \"${{ steps.audit.facts.verdict == 'accept' }}\" }
  export: { uses: ./nodes/cases.py#join, with: { parts: { r: \"${{ steps.body.rig.outputs.text }}\" } } }
  admit:
    uses: ./nodes/cases.py#verdict
    judges: export
    with: { subject: \"${{ steps.export.outputs.text }}\", accept_take: 1 }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    let mut expected = Vec::new();
    for build in 1..=2 {
        for take in 1..=2 {
            for role in ["head", "torso"] {
                for name in ["mesh", "measure", "renders", "review"] {
                    expected.push(format!("body.part['{role}'].{name}#{build}.{take}.1"));
                }
            }
        }
        for take in 1..=2 {
            for name in ["orient", "renders", "review"] {
                expected.push(format!("body.assemble.{name}#{build}.{take}.1"));
            }
        }
        expected.push(format!("body.rig#{build}.1"));
        expected.push(format!("body.audit#{build}.1"));
    }
    expected.extend(["export#1".to_string(), "admit#1".to_string()]);
    assert_eq!(ids(&expansion), expected);
    for build in 1..=2 {
        for take in 1..=2 {
            for role in ["head", "torso"] {
                let id = |name: &str| format!("body.part['{role}'].{name}#{build}.{take}.1");
                assert_eq!(
                    get(&expansion, &id("measure")).waiting_on(),
                    id_set(&[id("mesh")])
                );
                assert_eq!(
                    get(&expansion, &id("renders")).waiting_on(),
                    id_set(&[id("measure")])
                );
                let review = get(&expansion, &id("review"));
                assert_eq!(review.judges, Some(id("mesh")));
                assert_eq!(review.waiting_on(), id_set(&[id("renders")]));
            }
            let id = |name: &str| format!("body.assemble.{name}#{build}.{take}.1");
            assert_eq!(
                get(&expansion, &id("renders")).waiting_on(),
                id_set(&[id("orient")])
            );
            let review = get(&expansion, &id("review"));
            assert_eq!(review.judges, Some(id("orient")));
            assert_eq!(review.waiting_on(), id_set(&[id("renders")]));
        }
    }
}

#[test]
fn a_later_reader_waits_for_a_judge_unless_it_is_the_judges_evidence() {
    let project = Project::new();
    // The judge reads only the take it judges: `two` is no evidence, so in take 2 too it waits
    // for the judge it settles.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Not evidence
steps:
  part:
    steps:
      one: { uses: ./nodes/cases.py#shout, with: { text: x } }
      two: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.one.outputs.text }}\" } } }
      review:
        uses: ./nodes/cases.py#verdict
        judges: one
        with: { subject: \"${{ steps.one.outputs.text }}\", accept_take: 1 }
    regenerate: { max: 2, until: \"${{ steps.review.facts.verdict == 'accept' }}\" }
  after: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.part.two.outputs.text }}\" } } }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    assert_eq!(
        ids(&expansion),
        [
            "part.one#1.1",
            "part.review#1.1",
            "part.two#1.1",
            "part.one#2.1",
            "part.review#2.1",
            "part.two#2.1",
            "after#1"
        ]
    );
    for take in 1..=2 {
        assert_eq!(
            get(&expansion, &format!("part.two#{take}.1")).waiting_on(),
            set(&[
                &format!("part.one#{take}.1"),
                &format!("part.review#{take}.1")
            ])
        );
    }

    // Evidence named by `needs:` counts too: the judge needs `two`, which reads the judged take
    // as it is, and needs all of it, not the instances made so far.
    let expansion = project.expand(
        "\
fx: workflow/v1
id: case
title: Needs
steps:
  part:
    steps:
      one: { uses: ./nodes/cases.py#shout, with: { text: x } }
      two: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.one.outputs.text }}\" } } }
      review:
        uses: ./nodes/cases.py#verdict
        judges: one
        needs: [two]
        with: { subject: \"${{ steps.one.outputs.text }}\", accept_take: 1 }
    regenerate: { max: 2, until: \"${{ steps.review.facts.verdict == 'accept' }}\" }
  after: { uses: ./nodes/cases.py#join, with: { parts: { t: \"${{ steps.part.two.outputs.text }}\" } } }
",
    );
    assert!(expansion.problems.is_empty(), "{:?}", problems(&expansion));
    assert_eq!(
        ids(&expansion),
        [
            "part.one#1.1",
            "part.two#1.1",
            "part.review#1.1",
            "part.one#2.1",
            "part.two#2.1",
            "part.review#2.1",
            "after#1"
        ]
    );
    for take in 1..=2 {
        assert_eq!(
            get(&expansion, &format!("part.two#{take}.1")).waiting_on(),
            set(&[&format!("part.one#{take}.1")])
        );
        assert_eq!(
            get(&expansion, &format!("part.review#{take}.1")).needs,
            [format!("part.two#{take}.1")]
        );
    }
}
