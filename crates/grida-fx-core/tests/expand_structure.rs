//! Expansion structure: listing order, repeats, matrices, pending repeats,
//! groups, workflows used as steps, cycles, `needs:`, reads and phases.
//!
//! Each case writes a small project into a temporary folder (`fx.yaml`, `nodes/cases.py`, the
//! workflows), describes `nodes/cases.py` through a [`FakeHost`], and expands with no route table:
//! the node types are free.

use grida_fx_core::docs::lock::LockFile;
use grida_fx_core::docs::takes::Takes;
use grida_fx_core::docs::workflow::load_workflow;
use grida_fx_core::expand::{
    ExpandEnv, Expansion, Instance, NodeResult, ResultStatus, State, expand,
};
use grida_fx_core::host::FakeHost;
use grida_fx_core::registry::Registry;
use grida_fx_core::routes::RouteTable;
use grida_fx_core::val::Val;
use grida_fx_protocol::{ClosureEntry, DescribedType, ModuleDescription, TypeSpec};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

/// A node type of `nodes/cases.py`.
fn spec(name: &str, inputs: Value, params: Value, outputs: Value, judge: bool) -> TypeSpec {
    let ports = |value: Value| -> IndexMap<String, String> {
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect()
    };
    TypeSpec {
        name: name.to_string(),
        description: None,
        inputs: ports(inputs),
        params: params
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        outputs: ports(outputs),
        judge,
        calls: IndexMap::new(),
        resources: Vec::new(),
        tools: Vec::new(),
        view: None,
        version: None,
        retry: Default::default(),
    }
}

/// The free node types the cases use (unversioned, so no fx.lock is involved).
fn cases_module(path: PathBuf) -> ModuleDescription {
    let text = json!({"type": "string"});
    let types = vec![
        (
            "shout",
            spec(
                "shout",
                json!({}),
                json!({ "text": text }),
                json!({"text": "text"}),
                false,
            ),
        ),
        (
            "join",
            spec(
                "join",
                json!({"parts": "text{}"}),
                json!({}),
                json!({"text": "text"}),
                false,
            ),
        ),
        (
            "verdict",
            spec(
                "verdict",
                json!({"subject": "file"}),
                json!({"accept_take": {"type": "integer"}}),
                json!({}),
                true,
            ),
        ),
    ];
    ModuleDescription::Described {
        path: "nodes/cases.py".to_string(),
        types: types
            .into_iter()
            .map(|(attribute, spec)| DescribedType {
                attribute: attribute.to_string(),
                spec,
            })
            .collect(),
        closure: vec![ClosureEntry {
            label: "nodes/cases.py".to_string(),
            path: path.to_string_lossy().into_owned(),
        }],
    }
}

/// A project in a temporary folder.
struct Case {
    _dir: tempfile::TempDir,
    root: PathBuf,
    inputs: IndexMap<String, Val>,
    results: IndexMap<String, NodeResult>,
    /// An `fx: routes/v1` table, and the project's default route per capability.
    routes: Option<Value>,
    route_defaults: IndexMap<String, String>,
}

impl Case {
    /// A project holding `workflows/<name>.yaml` for each `(name, text)`.
    fn new(workflows: &[(&str, &str)]) -> Case {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        std::fs::create_dir_all(root.join("nodes")).unwrap();
        std::fs::write(root.join("nodes/cases.py"), "# node types of the cases\n").unwrap();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        for (name, text) in workflows {
            std::fs::write(root.join(format!("workflows/{name}.yaml")), text).unwrap();
        }
        Case {
            _dir: dir,
            root,
            inputs: IndexMap::new(),
            results: IndexMap::new(),
            routes: None,
            route_defaults: IndexMap::new(),
        }
    }

    /// One image route, `img-a@acme` at 0.01 to 0.04 a call, the default for its capability.
    fn with_image_route(mut self) -> Case {
        self.routes = Some(json!({
            "fx": "routes/v1",
            "routes": [{
                "capability": "image.generate",
                "route": "img-a@acme",
                "price": {"low_usd": 0.01, "high_usd": 0.04}
            }]
        }));
        self.route_defaults
            .insert("image.generate".into(), "img-a@acme".into());
        self
    }

    fn input(mut self, name: &str, value: Val) -> Case {
        self.inputs.insert(name.to_string(), value);
        self
    }

    /// A successful result with text outputs.
    fn done(&mut self, id: &str, outputs: &[(&str, Val)]) {
        self.results.insert(
            id.to_string(),
            NodeResult {
                status: ResultStatus::Succeeded,
                outputs: outputs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
                facts: IndexMap::new(),
                error: None,
            },
        );
    }

    /// Expands `workflows/<name>.yaml`.
    fn expand(&self, name: &str) -> Expansion {
        let source = format!("workflows/{name}.yaml");
        let workflow = Rc::new(load_workflow(&self.root.join(&source), &source, &source).unwrap());
        let mut registry = Registry::new(
            self.root.clone(),
            Vec::new(),
            LockFile::default(),
            self.route_defaults.clone(),
        );
        let mut host = FakeHost::new().with_module(cases_module(self.root.join("nodes/cases.py")));
        let routes = match &self.routes {
            Some(table) => RouteTable::from_document(table, "routes.yaml").unwrap(),
            None => RouteTable::new(),
        };
        let takes = Takes::new();
        expand(ExpandEnv {
            workflow: &workflow,
            inputs: &self.inputs,
            registry: &mut registry,
            host: &mut host,
            routes: &routes,
            takes: &takes,
            results: &self.results,
        })
        .unwrap()
    }
}

fn strings(values: &[&str]) -> Val {
    Val::List(values.iter().map(|v| Val::Str((*v).into())).collect())
}

fn ids(expansion: &Expansion) -> Vec<&str> {
    expansion.instances.keys().map(String::as_str).collect()
}

fn problems(expansion: &Expansion) -> Vec<(&str, &str)> {
    expansion
        .problems
        .iter()
        .map(|p| (p.where_.as_str(), p.message.as_str()))
        .collect()
}

fn instance<'e>(expansion: &'e Expansion, id: &str) -> &'e Instance {
    expansion
        .instances
        .get(id)
        .unwrap_or_else(|| panic!("no instance {id}: {:?}", ids(expansion)))
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

const ADD_MAX: &str = "the list comes from a step, so the plan cannot count it: add max: \
                       (the most items this repeat may run)";

#[test]
fn a_step_is_listed_after_what_it_reads() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Order
steps:
  a:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.c.outputs.text }}" }
  b:
    uses: ./nodes/cases.py#shout
    needs: [d]
    with: { text: b }
  c:
    uses: ./nodes/cases.py#shout
    with: { text: c }
  d:
    uses: ./nodes/cases.py#shout
    with: { text: d }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(ids(&expansion), ["c#1", "a#1", "d#1", "b#1"]);
    assert_eq!(instance(&expansion, "a#1").waiting_on(), set(&["c#1"]));
    assert_eq!(instance(&expansion, "b#1").needs, ["d#1"]);
}

#[test]
fn an_unreferenced_group_member_comes_last() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Groups
steps:
  x:
    uses: ./nodes/cases.py#shout
    with: { text: x }
  g:
    steps:
      m1: { uses: ./nodes/cases.py#shout, with: { text: m1 } }
      m2: { uses: ./nodes/cases.py#shout, with: { text: "${{ steps.m1.outputs.text }}" } }
      lonely: { uses: ./nodes/cases.py#shout, with: { text: lonely } }
  one:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.g.m1.outputs.text }}${{ steps.g.m2.outputs.text }}" }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(
        ids(&expansion),
        ["x#1", "g.m1#1", "g.m2#1", "one#1", "g.lonely#1"]
    );
    let m2 = instance(&expansion, "g.m2#1");
    assert_eq!(m2.step, "g.m2");
    assert_eq!(m2.path, "g.m2");
    assert_eq!(m2.key, None);
    assert_eq!(
        instance(&expansion, "one#1").waiting_on(),
        set(&["g.m1#1", "g.m2#1"])
    );
}

#[test]
fn used_workflow_scopes_are_swept_breadth_first() {
    let case = Case::new(&[
        (
            "outer",
            r#"fx: workflow/v1
id: outer
title: Outer
steps:
  w1:
    uses: ./workflows/mid.yaml
    with: { name: a }
  user:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.w1.deeper.loud.outputs.text }}" }
  w2:
    uses: ./workflows/mid.yaml
    with: { name: b }
"#,
        ),
        (
            "mid",
            r#"fx: workflow/v1
id: mid
title: Mid
inputs:
  name: { type: string }
steps:
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ inputs.name }}" }
  deeper:
    uses: ./workflows/deep.yaml
    with: { name: "${{ inputs.name }}" }
  read:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.loud.outputs.text }}" }
"#,
        ),
        (
            "deep",
            r#"fx: workflow/v1
id: deep
title: Deep
inputs:
  name: { type: string }
steps:
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ inputs.name }}!" }
"#,
        ),
    ]);
    let expansion = case.expand("outer");
    assert_eq!(problems(&expansion), []);
    assert_eq!(
        ids(&expansion),
        [
            "w1.deeper.loud#1",
            "user#1",
            "w1.loud#1",
            "w1.read#1",
            "w2.loud#1",
            "w2.read#1",
            "w2.deeper.loud#1",
        ]
    );
    let deep = instance(&expansion, "w2.deeper.loud#1");
    assert_eq!(deep.step, "w2.deeper.loud");
    assert_eq!(deep.with["text"], Val::Str("b!".into()));
    // Every workflow the plan used, the root first.
    assert_eq!(
        expansion.workflows.keys().collect::<Vec<_>>(),
        [
            "workflows/outer.yaml",
            "workflows/mid.yaml",
            "workflows/deep.yaml"
        ]
    );
}

#[test]
fn for_each_keys_duplicates_and_max() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Keys
inputs:
  names: { type: list, items: { type: string } }
steps:
  x:
    for_each: ${{ inputs.names }}
    key: ${{ item }}
    max: 2
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
  plain:
    for_each: [p, q]
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
"#,
    )])
    .input("names", strings(&["ada", "bo", "ada"]));
    let expansion = case.expand("case");
    assert_eq!(
        problems(&expansion),
        [
            ("x.for_each", "3 items exceed max: 2"),
            ("x.key", "key 'ada' names two items"),
        ]
    );
    assert_eq!(
        ids(&expansion),
        ["x['ada']#1", "x['bo']#1", "plain['0']#1", "plain['1']#1"]
    );
    let ada = instance(&expansion, "x['ada']#1");
    assert_eq!(ada.path, "x['ada']");
    assert_eq!(ada.step, "x");
    assert_eq!(ada.key.as_deref(), Some("ada"));
    assert_eq!(ada.concurrency_group.as_deref(), Some("x"));
    assert_eq!(ada.with["text"], Val::Str("ada".into()));
    assert_eq!(
        instance(&expansion, "plain['1']#1").key.as_deref(),
        Some("1")
    );
}

#[test]
fn a_key_that_cannot_be_read_leaves_no_item() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Bad key
steps:
  x:
    for_each: [ada, bo]
    key: ${{ item.zz }}
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
  y:
    for_each: [[1], [2]]
    key: ${{ item }}
    uses: ./nodes/cases.py#shout
    with: { text: y }
"#,
    )]);
    let expansion = case.expand("case");
    let found = problems(&expansion);
    assert_eq!(found.len(), 3, "{found:?}");
    assert_eq!(found[0].0, "x.key");
    assert_eq!(
        found[1],
        ("x", "an instance key is text or a number, not _Missing")
    );
    assert_eq!(
        found[2],
        ("y", "an instance key is text or a number, not list")
    );
    assert!(expansion.instances.is_empty());
}

#[test]
fn keys_of_numbers_booleans_and_quotes() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Key text
steps:
  u:
    for_each: [1, 2.5, true, 3.0, "it's", 'a\b', "x]y"]
    key: ${{ item }}
    uses: ./nodes/cases.py#shout
    with: { text: u }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(
        ids(&expansion),
        [
            "u['1']#1",
            "u['2.5']#1",
            "u['true']#1",
            "u['3']#1",
            r"u['it\'s']#1",
            r"u['a\\b']#1",
            "u['x]y']#1",
        ]
    );
}

#[test]
fn matrix_keys_and_edges() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Matrix
steps:
  combo:
    matrix: { eye: [open, shut], mouth: [smile, flat] }
    uses: ./nodes/cases.py#shout
    with: { text: "${{ matrix.eye }} ${{ matrix.mouth }}" }
  empty:
    matrix: { eye: [], mouth: [smile] }
    uses: ./nodes/cases.py#shout
    with: { text: empty }
  whole:
    matrix: {}
    uses: ./nodes/cases.py#shout
    with: { text: whole }
  twice:
    matrix: { n: [1, 1] }
    uses: ./nodes/cases.py#shout
    with: { text: twice }
  bad:
    matrix: { n: "${{ 'x' }}" }
    uses: ./nodes/cases.py#shout
    with: { text: bad }
outputs:
  count: ${{ len(steps.twice) }}
  texts: ${{ steps.combo.*.outputs.text }}
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), [("bad", "a matrix axis is a list")]);
    assert_eq!(
        ids(&expansion),
        [
            "combo['open']['smile']#1",
            "combo['open']['flat']#1",
            "combo['shut']['smile']#1",
            "combo['shut']['flat']#1",
            "whole#1",
            "twice['1']#1",
        ]
    );
    let first = instance(&expansion, "combo['open']['smile']#1");
    assert_eq!(first.key.as_deref(), Some("open.smile"));
    assert_eq!(first.path, "combo['open']['smile']");
    assert_eq!(first.with["text"], Val::Str("open smile".into()));
    assert_eq!(instance(&expansion, "whole#1").key.as_deref(), Some(""));
    // Two children with the same key, one instance.
    assert_eq!(expansion.outputs["count"], Val::Number(2.0));
    match &expansion.outputs["texts"] {
        Val::Collection(collection) => {
            let keys: Vec<&str> = collection.items.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["open.smile", "open.flat", "shut.smile", "shut.flat"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn pending_repeats_with_and_without_max() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Pending repeats
steps:
  split:
    uses: ./nodes/cases.py#shout
    with: { text: a }
  draw:
    for_each: ${{ steps.split.outputs.items }}
    max: 6
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item.text }}" }
  loose:
    for_each: ${{ steps.split.outputs.items }}
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}", bogus: 1 }
  late:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ len(steps.draw) }} ${{ steps.draw['a'].outputs.text }}" }
"#,
    )]);
    let expansion = case.expand("case");
    // The shadow item's own problems (`bogus`) are discarded while planning.
    assert_eq!(problems(&expansion), [("loose", ADD_MAX)]);
    assert_eq!(ids(&expansion), ["split#1", "late#1"]);
    assert_eq!(expansion.pending.len(), 2);
    let draw = &expansion.pending[0];
    assert_eq!(draw.path, "draw");
    assert_eq!(draw.max, 6);
    assert_eq!(draw.phase, 2);
    assert_eq!(draw.waiting_on, set(&["split#1"]));
    let loose = &expansion.pending[1];
    assert_eq!(loose.path, "loose");
    assert_eq!(loose.max, 1);
    // Whatever reads a pending repeat waits on what its list waits on.
    let late = instance(&expansion, "late#1");
    assert_eq!(late.waiting_on(), set(&["split#1"]));
    assert_eq!(late.identity, None);
}

#[test]
fn workflow_step_paths() {
    let case = Case::new(&[
        (
            "case",
            r#"fx: workflow/v1
id: case
title: A workflow used as a step
inputs:
  names: { type: list, items: { type: string } }
steps:
  each:
    for_each: ${{ inputs.names }}
    key: ${{ item }}
    uses: ./workflows/inner.yaml
    with: { name: "${{ item }}" }
outputs:
  loud: ${{ steps.each.*.outputs.loud }}
"#,
        ),
        (
            "inner",
            r#"fx: workflow/v1
id: inner
title: Inner
inputs:
  name: { type: string }
  suffix: { type: string, default: "!" }
steps:
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ inputs.name }}${{ inputs.suffix }}" }
outputs:
  loud: ${{ steps.loud.outputs.text }}
"#,
        ),
    ])
    .input("names", strings(&["ada", "bo"]));
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(ids(&expansion), ["each['ada'].loud#1", "each['bo'].loud#1"]);
    let ada = instance(&expansion, "each['ada'].loud#1");
    assert_eq!(ada.path, "each['ada'].loud");
    assert_eq!(ada.step, "each.loud");
    // Members of a repeated workflow have no key and no concurrency group.
    assert_eq!(ada.key, None);
    assert_eq!(ada.concurrency_group, None);
    assert_eq!(ada.with["text"], Val::Str("ada!".into()));
    match &expansion.outputs["loud"] {
        Val::Collection(collection) => {
            let keys: Vec<&str> = collection.items.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["ada", "bo"]);
            assert!(matches!(collection.items[0].1, Val::Pending(_)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_used_workflow_reports_unknown_inputs_and_output_problems_when_read() {
    let case = Case::new(&[
        (
            "case",
            r#"fx: workflow/v1
id: case
title: Used
steps:
  quiet:
    uses: ./workflows/inner.yaml
    with: { name: a }
  w:
    uses: ./workflows/inner.yaml
    with: { name: a, zeta: 1, alpha: 2 }
  reader:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.w.outputs.loud }}" }
"#,
        ),
        (
            "inner",
            r#"fx: workflow/v1
id: inner
title: Inner
inputs:
  name: { type: string }
steps:
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ inputs.name }}" }
outputs:
  loud: ${{ steps.loud.outputs.text }}
  bad: ${{ inputs.zz }}
"#,
        ),
    ]);
    let expansion = case.expand("case");
    assert_eq!(
        problems(&expansion),
        [
            ("w.with.alpha", "./workflows/inner.yaml has no input alpha"),
            ("w.with.zeta", "./workflows/inner.yaml has no input zeta"),
            // Only `w`'s outputs are read; the key has no step prefix.
            ("outputs.bad", "no field 'zz'"),
        ]
    );
    assert_eq!(ids(&expansion), ["w.loud#1", "reader#1", "quiet.loud#1"]);
}

#[test]
fn the_cycles_gnode_reports() {
    let case = Case::new(&[
        (
            "selfif",
            r#"fx: workflow/v1
id: selfif
title: Cycles
steps:
  x:
    if: ${{ steps.x.outputs.text == 'a' }}
    uses: ./nodes/cases.py#shout
    with: { text: x }
  y:
    for_each: ${{ steps.y.outputs.text }}
    uses: ./nodes/cases.py#shout
    with: { text: y }
  p:
    if: ${{ steps.q.outputs.text }}
    uses: ./nodes/cases.py#shout
    with: { text: p }
  q:
    if: ${{ steps.p.outputs.text }}
    uses: ./nodes/cases.py#shout
    with: { text: q }
  c:
    for_each: [a, b, c]
    key: ${{ len(steps.c) }}
    uses: ./nodes/cases.py#shout
    with: { text: c }
  r:
    for_each: [a, b]
    if: ${{ steps.r['0'].outputs.text == 'A' }}
    uses: ./nodes/cases.py#shout
    with: { text: r }
"#,
        ),
        (
            "selfwith",
            r#"fx: workflow/v1
id: selfwith
title: A used workflow reading itself
steps:
  w:
    uses: ./workflows/inner.yaml
    with: { name: "${{ steps.w.outputs.loud }}" }
"#,
        ),
        (
            "inner",
            r#"fx: workflow/v1
id: inner
title: Inner
inputs:
  name: { type: string }
steps:
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ inputs.name }}" }
outputs:
  loud: ${{ steps.loud.outputs.text }}
"#,
        ),
    ]);
    let expansion = case.expand("selfif");
    assert_eq!(
        problems(&expansion),
        [
            ("x.if", "x refers back to itself"),
            ("y.for_each", "y refers back to itself"),
            ("q.if", "p refers back to itself"),
            ("c.key", "c refers back to itself"),
            ("c", "an instance key is text or a number, not _Missing"),
            ("r.if", "a step refers back to itself"),
        ]
    );
    assert!(expansion.instances.is_empty());

    let expansion = case.expand("selfwith");
    let found = problems(&expansion);
    assert_eq!(found[0], ("w.with", "w refers back to itself"));
    // Every given value is lost, so the required input is reported too.
    assert_eq!(found[1].0, "w.with");
}

#[test]
fn fx_refuses_a_node_reading_itself() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Node cycles
steps:
  e:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.e.outputs.text }}" }
  f:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.g.outputs.text }}" }
  g:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.f.outputs.text }}" }
  draw:
    uses: ./nodes/cases.py#shout
    with: { text: draw }
  check:
    uses: ./nodes/cases.py#verdict
    judges: draw
    with: { subject: "${{ steps.draw.outputs.text }}", accept_take: 1 }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(
        problems(&expansion),
        [
            ("e.with.text", "e refers back to itself"),
            ("g.with.text", "f refers back to itself"),
        ]
    );
    assert_eq!(ids(&expansion), ["e#1", "g#1", "f#1", "draw#1", "check#1"]);
    assert_eq!(instance(&expansion, "e#1").with["text"], Val::Missing);
    assert_eq!(instance(&expansion, "f#1").waiting_on(), set(&["g#1"]));
    // A judge reads the step it judges: no cycle.
    assert_eq!(
        instance(&expansion, "check#1").waiting_on(),
        set(&["draw#1"])
    );
}

#[test]
fn a_recursive_let_is_refused() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Lets
let:
  x: ${{ let.x }}
  y: ${{ let.z }}
  z: ${{ let.y }}
  ok: ${{ item }}
steps:
  a:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ let.x }}" }
  b:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ let.y }}" }
  c:
    for_each: [one]
    uses: ./nodes/cases.py#shout
    with: { text: "${{ let.ok }}" }
  d:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ let.ok }}" }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(
        problems(&expansion),
        [
            ("a.with.text", "let.x refers back to itself"),
            ("b.with.text", "let.y refers back to itself"),
            // A let is evaluated where it is referenced.
            ("d.with.text", "unknown name 'item'"),
        ]
    );
    assert_eq!(
        instance(&expansion, "c['0']#1").with["text"],
        Val::Str("one".into())
    );
}

#[test]
fn needs_lists_ids_in_first_seen_order_and_cycles_stay_silent() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Needs
steps:
  g:
    steps:
      m: { uses: ./nodes/cases.py#shout, with: { text: m } }
  rep:
    for_each: [a, b]
    key: ${{ item }}
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
  grep:
    for_each: [a]
    key: ${{ item }}
    steps:
      m: { uses: ./nodes/cases.py#shout, with: { text: m } }
  x:
    uses: ./nodes/cases.py#shout
    with: { text: x }
  t:
    uses: ./nodes/cases.py#shout
    needs: [g, rep, grep, x, x]
    with: { text: t }
  a:
    uses: ./nodes/cases.py#shout
    needs: [b]
    with: { text: a }
  b:
    uses: ./nodes/cases.py#shout
    needs: [a]
    with: { text: b }
  self:
    uses: ./nodes/cases.py#shout
    needs: [self, zz]
    with: { text: s }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), [("self.needs", "no step zz")]);
    assert_eq!(
        instance(&expansion, "t#1").needs,
        ["g.m#1", "rep['a']#1", "rep['b']#1", "grep['a'].m#1", "x#1"]
    );
    assert_eq!(instance(&expansion, "a#1").needs, ["b#1"]);
    assert!(instance(&expansion, "b#1").needs.is_empty());
    assert!(instance(&expansion, "self#1").needs.is_empty());
}

#[test]
fn reads_leak_through_on_demand_expansion() {
    let mut case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Reads
steps:
  a:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.b.outputs.text }}" }
  b:
    if: ${{ steps.d.outputs.text == 'x' }}
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.c.outputs.text }}" }
  c:
    uses: ./nodes/cases.py#shout
    with: { text: c }
  d:
    uses: ./nodes/cases.py#shout
    with: { text: d }
  e:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.b.outputs.text }}" }
"#,
    )]);
    case.done("c#1", &[("text", Val::Str("x".into()))]);
    case.done("d#1", &[("text", Val::Str("x".into()))]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(instance(&expansion, "a#1").reads, set(&["c#1", "d#1"]));
    assert!(instance(&expansion, "e#1").reads.is_empty());
    assert_eq!(instance(&expansion, "b#1").reads, set(&["c#1"]));
    assert_eq!(instance(&expansion, "c#1").state, State::Done);
}

#[test]
fn the_phase_of_a_repeat_moves_between_expansions() {
    let workflow = r#"fx: workflow/v1
id: case
title: Phases
steps:
  split:
    uses: ./nodes/cases.py#shout
    with: { text: a }
  draw:
    for_each: ${{ steps.split.outputs.items }}
    max: 6
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
  again:
    for_each: ${{ steps.draw.*.outputs.text }}
    max: 6
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
  after:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.split.outputs.text }}" }
"#;
    let mut case = Case::new(&[("case", workflow)]);
    // While planning: both repeats wait on split#1.
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    let phases: Vec<(&str, u32)> = expansion
        .pending
        .iter()
        .map(|p| (p.path.as_str(), p.phase))
        .collect();
    assert_eq!(phases, [("draw", 2), ("again", 2)]);
    // An ordinary step stays in its frame's phase.
    assert_eq!(instance(&expansion, "after#1").phase, 1);

    // split ran: draw's items exist but have not run, so again's list is a known list of
    // pending values: its items are phase 1 (no result was read).
    case.done(
        "split#1",
        &[
            ("items", strings(&["a", "b"])),
            ("text", Val::Str("a".into())),
        ],
    );
    let expansion = case.expand("case");
    assert!(expansion.pending.is_empty());
    assert_eq!(instance(&expansion, "draw['0']#1").phase, 2);
    assert_eq!(instance(&expansion, "again['0']#1").phase, 1);

    // draw ran: again's items read its results.
    case.done("draw['0']#1", &[("text", Val::Str("A".into()))]);
    case.done("draw['1']#1", &[("text", Val::Str("B".into()))]);
    let expansion = case.expand("case");
    assert_eq!(instance(&expansion, "again['0']#1").phase, 3);
    assert_eq!(
        instance(&expansion, "again['1']#1").with["text"],
        Val::Str("B".into())
    );
}

#[test]
fn conditions_leave_steps_out_or_make_them_maybe() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Conditions
steps:
  split:
    uses: ./nodes/cases.py#shout
    with: { text: a }
  skipped:
    if: false
    uses: ./nodes/cases.py#shout
    with: { text: skipped }
  quoted:
    if: "false"
    uses: ./nodes/cases.py#shout
    with: { text: quoted }
  reader:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.skipped.outputs.text ?? 'none' }}" }
  gate:
    if: ${{ steps.split.outputs.ok }}
    steps:
      m:
        if: true
        uses: ./nodes/cases.py#shout
        with: { text: m }
  items:
    for_each: [a, b]
    if: ${{ item == 'b' }}
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
outputs:
  count: ${{ len(steps.items) }}
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(
        ids(&expansion),
        [
            "split#1",
            "quoted#1",
            "reader#1",
            "items['1']#1",
            "gate.m#1"
        ]
    );
    assert_eq!(
        instance(&expansion, "reader#1").with["text"],
        Val::Str("none".into())
    );
    // A pending condition: every instance under it is maybe, and the condition is no edge.
    let m = instance(&expansion, "gate.m#1");
    assert_eq!(m.state, State::Maybe);
    assert!(m.inputs_from().is_empty());
    // Items left out by `if:` still count.
    assert_eq!(expansion.outputs["count"], Val::Number(2.0));
}

#[test]
fn workflow_assertions_over_plan_values_refuse_the_plan() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Assertions
inputs:
  names: { type: list, items: { type: string } }
assert:
  - check: ${{ len(inputs.names) < 2 }}
    message: "at most one name, not ${{ len(inputs.names) }}"
steps:
  x:
    assert:
      - check: false
        message: never
    uses: ./nodes/cases.py#shout
    with: { text: x }
"#,
    )])
    .input("names", strings(&["ada", "bo"]));
    let expansion = case.expand("case");
    // A check over plan values refuses the plan; the step itself is still expanded (and its
    // instance checks it again: planning drops the repeat).
    let found = grida_fx_core::error::unique(expansion.problems.clone());
    assert_eq!(
        found
            .iter()
            .map(|p| (p.where_.as_str(), p.message.as_str()))
            .collect::<Vec<_>>(),
        [
            ("workflow.assert[0]", "at most one name, not 2"),
            ("x.assert[0]", "never"),
        ]
    );
    assert_eq!(ids(&expansion), ["x#1"]);
}

#[test]
fn a_regenerating_group_lists_its_takes() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: A group that regenerates
steps:
  build:
    steps:
      draw: { uses: ./nodes/cases.py#shout, with: { text: a mesh } }
      audit:
        uses: ./nodes/cases.py#verdict
        judges: draw
        with: { subject: "${{ steps.draw.outputs.text }}", accept_take: 1 }
        on_reject: continue
    regenerate: { max: 3, until: "${{ steps.audit.facts.verdict == 'accept' }}" }
  export:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.build.draw.take }}" }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(
        ids(&expansion),
        [
            "build.draw#1.1",
            "build.audit#1.1",
            // Reading the undecided group names every take's instances, which expands them.
            "build.draw#2.1",
            "build.audit#2.1",
            "build.draw#3.1",
            "build.audit#3.1",
            "export#1",
        ]
    );
    let states: Vec<State> = ["build.draw#1.1", "build.draw#2.1", "build.draw#3.1"]
        .iter()
        .map(|id| instance(&expansion, id).state)
        .collect();
    assert_eq!(states, [State::Planned, State::Maybe, State::Maybe]);
    assert_eq!(instance(&expansion, "build.draw#2.1").step, "build.draw");
    let export = instance(&expansion, "export#1");
    assert_eq!(
        export.waiting_on(),
        set(&[
            "build.draw#1.1",
            "build.audit#1.1",
            "build.draw#2.1",
            "build.audit#2.1",
            "build.draw#3.1",
            "build.audit#3.1",
        ])
    );
}

#[test]
fn a_workflow_that_uses_itself_is_refused() {
    let case = Case::new(&[
        (
            "case",
            r#"fx: workflow/v1
id: case
title: Recursion
steps:
  w:
    uses: ./workflows/loop.yaml
"#,
        ),
        (
            "loop",
            r#"fx: workflow/v1
id: loop
title: Loop
steps:
  again:
    uses: ./workflows/loop.yaml
  loud:
    uses: ./nodes/cases.py#shout
    with: { text: x }
"#,
        ),
    ]);
    let expansion = case.expand("case");
    assert_eq!(
        problems(&expansion),
        [("w.again", "./workflows/loop.yaml refers back to itself")]
    );
    assert_eq!(ids(&expansion), ["w.loud#1"]);
}

#[test]
fn an_unresolvable_uses_leaves_the_whole_repeat_absent() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Unresolvable
steps:
  rep:
    for_each: [a, b]
    uses: ./nodes/missing.py#shout
    with: { text: "${{ item }}" }
outputs:
  rep: ${{ steps.rep ?? 'gone' }}
"#,
    )]);
    let expansion = case.expand("case");
    let found = problems(&expansion);
    // One problem, at the step; the step reads as missing.
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "rep");
    assert!(expansion.instances.is_empty());
    assert_eq!(expansion.outputs["rep"], Val::Str("gone".into()));
}

#[test]
fn shadow_prices_count_nested_pending_repeats_and_budgets_scope_each_item() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Prices
steps:
  split:
    uses: ./nodes/cases.py#shout
    with: { text: a }
  outer:
    for_each: ${{ steps.split.outputs.items }}
    max: 3
    steps:
      pic:
        uses: fx/image.generate@1
        with: { prompt: "${{ item }}" }
      inner:
        for_each: ${{ steps.pic.outputs.image.parts }}
        max: 2
        uses: fx/image.generate@1
        with: { prompt: inner }
  grid:
    matrix: { a: "${{ steps.split.outputs.list }}", b: [1, 2] }
    max: 4
    uses: fx/image.generate@1
    with: { prompt: "${{ matrix.a }}" }
  each:
    for_each: [x, y]
    budget: { max_usd: 0.1 }
    uses: fx/image.generate@1
    with: { prompt: "${{ item }}" }
"#,
    )])
    .with_image_route();
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    // Only the listed repeats are pending: the nested one priced inside its shadow is not.
    let pending: Vec<(&str, u32)> = expansion
        .pending
        .iter()
        .map(|p| (p.path.as_str(), p.max))
        .collect();
    assert_eq!(pending, [("outer", 3), ("grid", 4)]);
    let outer = &expansion.pending[0];
    // One item: pic (0.01 to 0.04) and up to 2 inner draws (0.04 each at worst).
    assert_eq!(outer.per_instance_low.0, 10_000);
    assert_eq!(outer.per_instance_high.0, 120_000);
    assert_eq!(outer.high().0, 360_000);
    assert_eq!(expansion.pending[1].per_instance_high.0, 40_000);
    assert_eq!(ids(&expansion), ["split#1", "each['0']#1", "each['1']#1"]);
    let first = instance(&expansion, "each['0']#1");
    assert_eq!(
        first
            .budget
            .as_ref()
            .map(|(owner, ceiling)| (owner.as_str(), ceiling.0)),
        Some(("each['0']", 100_000))
    );
    assert_eq!(first.high().0, 40_000);
}

#[test]
fn a_repeat_inside_a_repeat_flattens_its_keys() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: Nested repeats
steps:
  ent:
    for_each: [ada, bo]
    key: ${{ item }}
    steps:
      shot:
        for_each: [x, y]
        key: ${{ item }}
        uses: ./nodes/cases.py#shout
        with: { text: "${{ item }}" }
  pick:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.ent['bo'].shot['y'].outputs.text }}" }
  count:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ len(steps.ent) }}" }
outputs:
  all: ${{ steps.ent.*.shot.*.outputs.text }}
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    assert_eq!(
        ids(&expansion),
        [
            "ent['bo'].shot['x']#1",
            "ent['bo'].shot['y']#1",
            "pick#1",
            "count#1",
            "ent['ada'].shot['x']#1",
            "ent['ada'].shot['y']#1",
        ]
    );
    let shot = instance(&expansion, "ent['ada'].shot['x']#1");
    assert_eq!(shot.step, "ent.shot");
    // The inner repeat is a node repeat: its items have keys and a concurrency group.
    assert_eq!(shot.key.as_deref(), Some("x"));
    assert_eq!(shot.concurrency_group.as_deref(), Some("ent['ada'].shot"));
    assert_eq!(
        instance(&expansion, "pick#1").waiting_on(),
        set(&["ent['bo'].shot['y']#1"])
    );
    // A whole `${{ }}` keeps the value's type.
    assert_eq!(
        instance(&expansion, "count#1").with["text"],
        Val::Number(2.0)
    );
    match &expansion.outputs["all"] {
        Val::Collection(collection) => {
            let keys: Vec<&str> = collection.items.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["ada.x", "ada.y", "bo.x", "bo.y"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_repeat_indexed_by_a_key_a_run_gives_waits_on_every_instance() {
    let case = Case::new(&[(
        "case",
        r#"fx: workflow/v1
id: case
title: A key only a run knows
steps:
  pick:
    uses: ./nodes/cases.py#shout
    with: { text: a }
  rep:
    for_each: [a, b]
    key: ${{ item }}
    uses: ./nodes/cases.py#shout
    with: { text: "${{ item }}" }
  chosen:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.rep[steps.pick.outputs.text].outputs.text }}" }
  member:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.ent[steps.pick.outputs.text].shot.outputs.text }}" }
  ent:
    for_each: [ada, bo]
    key: ${{ item }}
    steps:
      shot:
        uses: ./nodes/cases.py#shout
        with: { text: "${{ item }}" }
  every:
    uses: ./nodes/cases.py#shout
    with: { text: "${{ steps.rep.*.outputs.text[steps.pick.outputs.text] }}" }
"#,
    )]);
    let expansion = case.expand("case");
    assert_eq!(problems(&expansion), []);
    // Any instance may be the one picked: the value waits on the key and on all of them.
    let chosen = instance(&expansion, "chosen#1");
    assert_eq!(
        chosen.waiting_on(),
        set(&["pick#1", "rep['a']#1", "rep['b']#1"])
    );
    assert_eq!(chosen.identity, None);
    assert!(matches!(chosen.with["text"], Val::Pending(_)));
    // A repeated group: every member instance of every item.
    assert_eq!(
        instance(&expansion, "member#1").waiting_on(),
        set(&["ent['ada'].shot#1", "ent['bo'].shot#1", "pick#1"])
    );
    // A `.*` result is finished into its collection first, as before: the key and its items.
    assert_eq!(
        instance(&expansion, "every#1").waiting_on(),
        set(&["pick#1", "rep['a']#1", "rep['b']#1"])
    );
}

/// A workflow whose first step, `top`, reads the last of `n` steps `s0`…, each reading the one
/// before it: expanding `top` expands the whole chain inside it.
fn chain(n: usize) -> String {
    let mut text = String::from("fx: workflow/v1\nid: case\ntitle: A chain\nsteps:\n");
    let read = |i: usize| format!("\"${{{{ steps.s{i}.outputs.text }}}}\"");
    text += &format!(
        "  top:\n    uses: ./nodes/cases.py#shout\n    with: {{ text: {} }}\n",
        read(n - 1)
    );
    text += "  s0:\n    uses: ./nodes/cases.py#shout\n    with: { text: a }\n";
    for i in 1..n {
        text += &format!(
            "  s{i}:\n    uses: ./nodes/cases.py#shout\n    with: {{ text: {} }}\n",
            read(i - 1)
        );
    }
    text
}

/// Expands `chain(n)` on a thread with a stack of `stack` bytes: the instance count and the
/// problems.
fn expand_chain(n: usize, stack: usize) -> (usize, Vec<(String, String)>) {
    let case = Case::new(&[("case", chain(n).as_str())]);
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let expansion = case.expand("case");
            let problems = expansion
                .problems
                .iter()
                .map(|p| (p.where_.clone(), p.message.clone()))
                .collect();
            (expansion.instances.len(), problems)
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn a_chain_of_a_thousand_steps_plans_on_an_8_mb_stack() {
    // Every step a step reads expands inside it: the frames between two links of the chain are
    // kept small enough for a thousand links on an 8 MB stack, unoptimized.
    let (count, problems) = expand_chain(1000, 8 << 20);
    assert_eq!(problems, []);
    assert_eq!(count, 1001);
}

#[test]
fn a_chain_longer_than_the_bound_is_refused() {
    // `top` expands s2099, which expands s2098, …: s100 would be the 2001st expansion under way.
    let (count, problems) = expand_chain(2100, 256 << 20);
    assert_eq!(
        problems,
        [(
            "s100".to_string(),
            "the chain of steps reading each other is longer than 2000".to_string()
        )]
    );
    // s100 stays absent; every other step plans.
    assert_eq!(count, 2100);
}
