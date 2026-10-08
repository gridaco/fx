//! The machine documents of the planning verbs, and the plan digest.
//!
//! - [`graph_document`]: `grida-fx expand` and `plan --json` print fx-graph-v1
//!   (spec/schemas/fx-graph-v1.schema.json): `kind`, `workflow` `{id, title, description?,
//!   file}`, `types` (each distinct uses: `{identity, source?}`), `instances`, `pending`
//!   `{path, max, phase, high_usd}`, `estimate` `{low_usd, high_usd, ceiling_usd}`, `problems`.
//! - [`instance_document`]: one instance: `id`, `path`, `step`, `take`, `uses`, `type` (the type
//!   identity), `with` (each value's `shown` form, pending as `{"pending": …}`), `routes`
//!   `{cap: {route, fingerprint}}`, `state`, `identity`, `phase`, `key`, `judges`, `judged_by`
//!   (link order), `waiting_on` (sorted), `needs` (first-seen order), `reads` (the sorted union
//!   of what it read, waits on and needs, without itself), `price` `{low_usd, high_usd}`,
//!   `view`, and `reason` only when set.
//! - [`identity_document`]: `{id: identity or null}` for planned, maybe and done instances.
//! - [`price_document`]: `{"phases": [{phase, steps, calls: [lo, hi], low_usd, high_usd,
//!   then}], "estimate": {low_usd, high_usd}, "ceiling_usd"}`.
//! - [`plan_digest`]: identity.md §10 (`fx-plan-v1`): workflows by source as authored, inputs
//!   as given (plain), takes `{step: take}`, types `{uses: identity}`, routes = sorted distinct
//!   fingerprints of every capability-route pair bound. `None` while an input is pending (never in
//!   practice).
//!
//! Every document holds FX values only; the CLI prints them with
//! [`crate::value::write_json`] and a newline. No absolute path appears in any of them. Money is
//! written as a JSON number of dollars (identity.md §12).

use super::{Estimate, PhaseSummary, Plan};
use crate::error::Problem;
use crate::expand::{Instance, PendingRepeat, State};
use crate::money::Usd;
use crate::project::Planner;
use crate::registry::ResolvedType;
use crate::value;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[cfg(test)]
pub(crate) mod test_support;

/// The fx-graph-v1 document, with `problems`.
pub fn graph_document(plan: &Plan, planner: &Planner) -> Value {
    let loaded = &planner.workflow;
    let declared = &loaded.workflow;
    let mut workflow = Map::new();
    workflow.insert("id".into(), Value::String(declared.id.clone()));
    workflow.insert("title".into(), Value::String(declared.title.clone()));
    if let Some(description) = &declared.description {
        workflow.insert("description".into(), Value::String(description.clone()));
    }
    workflow.insert("file".into(), Value::String(loaded.source.clone()));
    let types: Map<String, Value> = plan
        .expansion
        .types
        .iter()
        .map(|(uses, resolved)| (uses.clone(), type_entry(resolved)))
        .collect();
    let estimate = plan.estimate();
    json!({
        "kind": "fx-graph-v1",
        "workflow": workflow,
        "types": types,
        "scopes": plan.expansion.scopes,
        "instances": plan.expansion.ordered().map(instance_document).collect::<Vec<_>>(),
        "pending": plan.expansion.pending.iter().map(pending_document).collect::<Vec<_>>(),
        "estimate": {
            "low_usd": usd(estimate.low),
            "high_usd": usd(estimate.high),
            "ceiling_usd": ceiling(plan.ceiling),
        },
        "problems": problems(&plan.problems),
    })
}

/// One instance of the graph.
pub fn instance_document(instance: &Instance) -> Value {
    let with: Map<String, Value> = instance
        .with
        .iter()
        .map(|(name, value)| (name.clone(), value.shown()))
        .collect();
    let routes: Map<String, Value> = instance
        .routes
        .iter()
        .map(|(capability, route)| {
            (
                capability.clone(),
                json!({"route": route.id(), "fingerprint": route.fingerprint()}),
            )
        })
        .collect();
    let type_identity = if instance.ty.identity.is_empty() {
        Value::Null
    } else {
        Value::String(instance.ty.identity.clone())
    };
    let mut document = json!({
        "id": instance.id,
        "path": instance.path,
        "step": instance.step,
        "take": instance.takes,
        "uses": instance.uses,
        "type": type_identity,
        "with": with,
        "routes": routes,
        "state": instance.state.as_str(),
        "identity": instance.identity,
        "phase": instance.phase,
        "key": instance.key,
        "judges": instance.judges,
        "judged_by": instance.judged_by,
        "waiting_on": instance.waiting_on().into_iter().collect::<Vec<_>>(),
        "needs": instance.needs,
        "reads": instance.inputs_from().into_iter().collect::<Vec<_>>(),
        "bindings": instance.bindings,
        "interface_bindings": instance.interface_bindings,
        "price": {"low_usd": usd(instance.low()), "high_usd": usd(instance.high())},
        "view": instance.view,
    });
    if let (Some(reason), Value::Object(map)) = (&instance.reason, &mut document)
        && !reason.is_empty()
    {
        map.insert("reason".into(), Value::String(reason.clone()));
    }
    document
}

/// `grida-fx identity`.
pub fn identity_document(plan: &Plan) -> Value {
    let identities: Map<String, Value> = plan
        .expansion
        .ordered()
        .filter(|i| matches!(i.state, State::Planned | State::Maybe | State::Done))
        .map(|i| {
            let identity = i.identity.clone().map_or(Value::Null, Value::String);
            (i.id.clone(), identity)
        })
        .collect();
    Value::Object(identities)
}

/// `grida-fx price`.
pub fn price_document(plan: &Plan) -> Value {
    price_of(&plan.phases(), plan.estimate(), plan.ceiling)
}

/// The plan digest (identity.md §10).
pub fn plan_digest(plan: &Plan, planner: &Planner) -> Option<String> {
    let root = &planner.workflow;
    let mut workflows = Map::new();
    if !plan.expansion.workflows.contains_key(&root.source) {
        workflows.insert(root.source.clone(), root.document.clone());
    }
    for (source, document) in &plan.expansion.workflows {
        workflows.insert(source.clone(), document.clone());
    }
    let mut inputs = Map::new();
    for (name, given) in &planner.inputs.given {
        inputs.insert(name.clone(), given.plain()?);
    }
    let takes: Map<String, Value> = planner
        .takes
        .iter()
        .map(|(step, choice)| (step.clone(), Value::from(choice.take)))
        .collect();
    let types: Map<String, Value> = plan
        .expansion
        .types
        .iter()
        .map(|(uses, resolved)| (uses.clone(), Value::String(resolved.identity.clone())))
        .collect();
    let routes: BTreeSet<String> = plan
        .expansion
        .ordered()
        .flat_map(|i| i.routes.values().map(|route| route.fingerprint()))
        .collect();
    Some(value::digest(&plan_object(
        workflows, inputs, takes, types, routes,
    )))
}

/// The `fx-plan-v1` object of identity.md §10 from its parts.
fn plan_object(
    workflows: Map<String, Value>,
    inputs: Map<String, Value>,
    takes: Map<String, Value>,
    types: Map<String, Value>,
    routes: BTreeSet<String>,
) -> Value {
    json!({
        "kind": "fx-plan-v1",
        "workflows": workflows,
        "inputs": inputs,
        "takes": takes,
        "types": types,
        "routes": routes.into_iter().collect::<Vec<_>>(),
    })
}

/// `types.<uses>` of the graph: the identity, and a project type's source digests.
fn type_entry(resolved: &ResolvedType) -> Value {
    let mut entry = Map::new();
    entry.insert("identity".into(), Value::String(resolved.identity.clone()));
    entry.insert("ports".into(), ports_document(&resolved.spec));
    if let Some(source) = &resolved.source {
        entry.insert("source".into(), source.to_value());
    }
    Value::Object(entry)
}

/// Display declarations, kept separate from type and step identity documents.
pub fn ports_document(spec: &crate::spec::NodeSpec) -> Value {
    let inputs: Map<String, Value> = spec
        .inputs
        .iter()
        .map(|(name, port)| (name.clone(), Value::from(port.notation())))
        .collect();
    let outputs: Map<String, Value> = spec
        .outputs
        .iter()
        .map(|(name, port)| (name.clone(), Value::from(port.notation())))
        .collect();
    json!({"inputs": inputs, "outputs": outputs, "params": spec.params})
}

fn pending_document(repeat: &PendingRepeat) -> Value {
    json!({
        "path": repeat.path,
        "max": repeat.max,
        "phase": repeat.phase,
        "high_usd": usd(repeat.high()),
    })
}

fn problems(problems: &[Problem]) -> Value {
    Value::Array(
        problems
            .iter()
            .map(|p| json!({"where": p.where_, "message": p.message}))
            .collect(),
    )
}

/// The price document from computed phases.
fn price_of(phases: &[PhaseSummary], estimate: Estimate, ceiling_usd: Option<Usd>) -> Value {
    let phases: Vec<Value> = phases
        .iter()
        .map(|phase| {
            json!({
                "phase": phase.phase,
                "steps": phase.steps,
                "calls": [phase.calls_low, phase.calls_high],
                "low_usd": usd(phase.low),
                "high_usd": usd(phase.high),
                "then": phase.pending,
            })
        })
        .collect();
    json!({
        "phases": phases,
        "estimate": {"low_usd": usd(estimate.low), "high_usd": usd(estimate.high)},
        "ceiling_usd": ceiling(ceiling_usd),
    })
}

/// An amount as an FX number of dollars: the micro-dollars divided by a million, as the nearest
/// double, through [`value::number`] (identity.md §12: `0` and `0.0` are one value).
fn usd(amount: Usd) -> Value {
    value::number(amount.0 as f64 / 1_000_000.0).expect("micro-dollars are finite")
}

fn ceiling(amount: Option<Usd>) -> Value {
    amount.map_or(Value::Null, usd)
}

#[cfg(test)]
mod tests {
    //! Tests on hand-built instances and plans. Tests whose name starts with `integrated_` read
    //! values only other modules compute (with-values' shown forms, route fingerprints, source
    //! digests, the plan's pricing), so they hold once those modules are in place.

    use super::test_support::*;
    use super::*;
    use crate::docs::Schema;
    use crate::val::Val;

    fn graph_schema() -> jsonschema::Validator {
        let schema: Value = serde_json::from_str(Schema::Graph.text()).unwrap();
        jsonschema::validator_for(&schema).unwrap()
    }

    fn assert_valid_graph(document: &Value) {
        let validator = graph_schema();
        let errors: Vec<String> = validator
            .iter_errors(document)
            .map(|e| format!("{}: {e}", e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{errors:#?}\n{}",
            value::write_json(document)
        );
    }

    #[test]
    fn money_is_a_number_of_dollars() {
        assert_eq!(value::canon(&usd(Usd(0))), "0");
        assert_eq!(value::canon(&usd(Usd(46_000))), "0.046");
        assert_eq!(value::canon(&usd(Usd(6_862_500))), "6.8625");
        assert_eq!(value::canon(&usd(Usd(2_000_000))), "2");
        assert_eq!(value::canon(&usd(Usd(1))), "0.000001");
        assert_eq!(ceiling(None), Value::Null);
    }

    #[test]
    fn an_instance_keeps_the_predecessors_keys() {
        let mut draw = instance("draw#1", State::Planned, 1, &[(1, 10_000, 40_000)], None);
        draw.reads.insert("count#1".into());
        draw.reads.insert("draw#1".into());
        draw.needs = vec!["zeta#1".into(), "alpha#1".into()];
        draw.judged_by = vec!["check#1".into()];
        let document = instance_document(&draw);
        let keys: BTreeSet<&str> = document
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "id",
                "path",
                "step",
                "take",
                "uses",
                "type",
                "with",
                "routes",
                "state",
                "identity",
                "phase",
                "key",
                "judges",
                "judged_by",
                "waiting_on",
                "needs",
                "reads",
                "bindings",
                "interface_bindings",
                "price",
                "view",
            ])
        );
        assert_eq!(document["id"], "draw#1");
        assert_eq!(document["path"], "draw");
        assert_eq!(document["step"], "draw");
        assert_eq!(document["take"], json!([1]));
        assert_eq!(document["uses"], "fx/image.generate@1");
        assert_eq!(document["type"], "fx/image.generate@1.1");
        assert_eq!(document["state"], "planned");
        assert_eq!(document["identity"], Value::Null);
        assert_eq!(document["phase"], 1);
        assert_eq!(document["key"], Value::Null);
        assert_eq!(document["judges"], Value::Null);
        assert_eq!(document["judged_by"], json!(["check#1"]));
        assert_eq!(document["needs"], json!(["zeta#1", "alpha#1"]));
        assert_eq!(
            document["reads"],
            json!(["alpha#1", "count#1", "zeta#1"]),
            "sorted, without itself"
        );
        assert_eq!(
            document["price"],
            json!({"low_usd": 0.01, "high_usd": 0.04})
        );
        assert_eq!(document["view"], false);
    }

    #[test]
    fn a_reason_appears_only_when_set() {
        let mut later = instance("draw#2", State::Maybe, 1, &[], Some(&hex('a')));
        later.takes = vec![2];
        assert!(instance_document(&later).get("reason").is_none());
        later.reason = Some(String::new());
        assert!(instance_document(&later).get("reason").is_none());
        later.reason = Some("only if the take before it is rejected".into());
        let document = instance_document(&later);
        assert_eq!(document["reason"], "only if the take before it is rejected");
        assert_eq!(document["take"], json!([2]));
        assert_eq!(document["identity"], hex('a'));
        assert_eq!(document["price"], json!({"low_usd": 0, "high_usd": 0}));
    }

    #[test]
    fn with_and_routes_come_from_their_owners() {
        let mut draw = instance("draw#1", State::Planned, 1, &[(1, 10_000, 40_000)], None);
        draw.with
            .insert("prompt".into(), Val::Str("A picture".into()));
        let route = route("image.generate", "img-a", "acme");
        draw.routes.insert("image.generate".into(), route.clone());
        let document = instance_document(&draw);
        assert_eq!(
            document["with"],
            json!({"prompt": Val::Str("A picture".into()).shown()})
        );
        assert_eq!(
            document["routes"],
            json!({"image.generate": {"route": "img-a@acme", "fingerprint": route.fingerprint()}})
        );
    }

    #[test]
    fn identities_of_planned_maybe_and_done_instances() {
        let (mut plan, _) = linear_plan();
        let mut gone = instance("gone#1", State::Absent, 1, &[], Some(&hex('b')));
        gone.reason = Some("its condition is false".into());
        plan.expansion.instances.insert(gone.id.clone(), gone);
        let blocked = instance("stuck#1", State::Blocked, 1, &[], None);
        plan.expansion.instances.insert(blocked.id.clone(), blocked);
        let done = instance("ready#1", State::Done, 1, &[], Some(&hex('c')));
        plan.expansion.instances.insert(done.id.clone(), done);
        assert_eq!(
            identity_document(&plan),
            json!({
                "count#1": hex('d'),
                "draw#1": null,
                "ready#1": hex('c'),
            })
        );
    }

    #[test]
    fn price_of_the_captured_refusals() {
        let phases = [
            PhaseSummary {
                phase: 1,
                steps: 4,
                calls_low: 3,
                calls_high: 3,
                low: Usd(13_000),
                high: Usd(46_000),
                pending: vec![],
            },
            PhaseSummary {
                phase: 2,
                steps: 0,
                calls_low: 0,
                calls_high: 0,
                low: Usd(0),
                high: Usd(0),
                pending: vec!["unbounded (up to 1)".into()],
            },
        ];
        let estimate = Estimate {
            low: Usd(13_000),
            high: Usd(46_000),
        };
        let document = price_of(&phases, estimate, None);
        assert_eq!(
            value::write_json(&document),
            r#"{
 "ceiling_usd": null,
 "estimate": {
  "high_usd": 0.046,
  "low_usd": 0.013
 },
 "phases": [
  {
   "calls": [
    3,
    3
   ],
   "high_usd": 0.046,
   "low_usd": 0.013,
   "phase": 1,
   "steps": 4,
   "then": []
  },
  {
   "calls": [
    0,
    0
   ],
   "high_usd": 0,
   "low_usd": 0,
   "phase": 2,
   "steps": 0,
   "then": [
    "unbounded (up to 1)"
   ]
  }
 ]
}"#
        );
        let capped = price_of(&phases, estimate, Some(Usd(1_000_000)));
        assert_eq!(capped["ceiling_usd"], 1);
    }

    #[test]
    fn graph_of_built_in_steps_fits_its_schema() {
        let (plan, planner) = builtin_plan();
        let document = graph_document(&plan, &planner);
        assert_valid_graph(&document);
        assert_eq!(document["kind"], "fx-graph-v1");
        assert_eq!(
            document["workflow"],
            json!({"id": "case", "title": "Case", "file": "workflows/case.yaml"})
        );
        assert_eq!(
            document["types"],
            json!({
                "fx/files.copy@1": {"identity": "fx/files.copy@1.1", "ports": {"inputs": {}, "outputs": {}, "params": {}}},
                "fx/image.generate@1": {"identity": "fx/image.generate@1.1", "ports": {"inputs": {}, "outputs": {}, "params": {}}},
            })
        );
        let ids: Vec<&Value> = document["instances"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| &i["id"])
            .collect();
        assert_eq!(ids, ["copy#1", "draw#1"], "listing order");
        assert_eq!(document["instances"][1]["reads"], json!(["copy#1"]));
        assert_eq!(document["pending"], json!([]));
        assert_eq!(
            document["problems"],
            json!([{
                "where": "draw.route",
                "message": "no route img-a@acme serves image.generate (known routes: none)",
            }])
        );
        assert_eq!(document["estimate"]["ceiling_usd"], Value::Null);
    }

    #[test]
    fn graph_names_a_project_type_by_its_identity() {
        let (plan, planner) = linear_plan();
        let document = graph_document(&plan, &planner);
        assert_eq!(
            document["types"]["./nodes/cases.py#lines"]["identity"],
            "nodes/cases.py#lines@1"
        );
        assert!(
            document["types"]["./nodes/cases.py#lines"]
                .get("source")
                .is_some()
        );
        assert!(
            document["types"]["fx/image.generate@1"]
                .get("source")
                .is_none()
        );
        assert_eq!(document["instances"][0]["type"], "nodes/cases.py#lines@1");
        assert_eq!(document["instances"][0]["uses"], "./nodes/cases.py#lines");
    }

    #[test]
    fn graph_keeps_pending_repeats_problems_and_a_description() {
        let (mut plan, mut planner) = phase_plan();
        let mut declared = (*planner.workflow.workflow).clone();
        declared.description = Some("Draws each line.".into());
        let mut loaded = (*planner.workflow).clone();
        loaded.workflow = std::rc::Rc::new(declared);
        planner.workflow = std::rc::Rc::new(loaded);
        plan.problems = vec![Problem::new("draw.route", "no route for image.generate")];
        let document = graph_document(&plan, &planner);
        assert_valid_graph(&document);
        assert_eq!(document["workflow"]["description"], "Draws each line.");
        assert_eq!(
            document["pending"],
            json!([{"path": "draw", "max": 6, "phase": 2, "high_usd": 0.24}])
        );
        assert_eq!(
            document["problems"],
            json!([{"where": "draw.route", "message": "no route for image.generate"}])
        );
        assert_eq!(document["estimate"]["ceiling_usd"], 1);
    }

    #[test]
    fn the_plan_object_of_identity_md() {
        // identity.md §14, "A plan": the parts as the planner and the expansion hold them.
        let (workflows, inputs, takes, types, routes) = tiny_plan_parts();
        let object = plan_object(workflows, inputs, takes, types, routes);
        assert_eq!(
            value::canon(&object),
            r#"{"inputs":{"brief":{"file":"c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8"}},"kind":"fx-plan-v1","routes":["4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4","5d727add4c417db9508eb6c7f81ff19f7d90005adf080c0702acb9077a330d6b"],"takes":{"draw":2},"types":{"fx/image.edit@1":"fx/image.edit@1.1","fx/image.generate@1":"fx/image.generate@1.1"},"workflows":{"workflows/tiny.yaml":{"fx":"workflow/v1","id":"tiny","inputs":{"brief":{"kind":"text/plain","type":"file"}},"steps":{"draw":{"route":"img-a@acme","uses":"fx/image.generate@1","with":{"prompt":"${{ inputs.brief }}"}},"tint":{"route":"img-a@acme","uses":"fx/image.edit@1","with":{"image":"${{ steps.draw.outputs.image }}","prompt":"Make it blue."}}},"title":"Tiny"}}}"#
        );
        assert_eq!(
            value::digest(&object),
            "89d7ffdb27cd591501b572fcc8ea6260e794b5af27baa6a2aae0f9a86542a3c2"
        );
    }

    #[test]
    fn integrated_plan_digest_of_identity_md() {
        let (plan, planner) = tiny_plan();
        assert_eq!(
            plan_digest(&plan, &planner).as_deref(),
            Some("89d7ffdb27cd591501b572fcc8ea6260e794b5af27baa6a2aae0f9a86542a3c2")
        );
    }

    #[test]
    fn integrated_plan_digest_keeps_the_root_workflow() {
        // The expansion records the workflows it used; the root counts even when it is not
        // among them, so an expansion that stopped early still names its workflow.
        let (mut plan, planner) = tiny_plan();
        plan.expansion.workflows.clear();
        assert_eq!(
            plan_digest(&plan, &planner).as_deref(),
            Some("89d7ffdb27cd591501b572fcc8ea6260e794b5af27baa6a2aae0f9a86542a3c2")
        );
    }

    #[test]
    fn integrated_plan_digest_waits_on_a_pending_input() {
        let (plan, mut planner) = tiny_plan();
        planner.inputs.given.insert(
            "brief".into(),
            Val::Pending(Box::new(crate::val::Pending::of("count#1", None))),
        );
        assert_eq!(plan_digest(&plan, &planner), None);
    }

    #[test]
    fn integrated_graph_with_routes_and_project_sources_fits_its_schema() {
        let (plan, planner) = tiny_plan();
        let document = graph_document(&plan, &planner);
        assert_valid_graph(&document);
        let (plan, planner) = local_identity_plan();
        let document = graph_document(&plan, &planner);
        assert_valid_graph(&document);
        assert_eq!(
            document["types"]["./nodes/n.py#echo"],
            json!({
                "identity": "nodes/n.py#echo@source:f80b608fa143ba177212d039ea7c96232c844aefb446375b523c6108774327b3",
                "ports": {"inputs": {}, "outputs": {}, "params": {}},
                "source": {
                    "files": {"nodes/n.py": "9e26bf369911c45c243c684147b23fc9e1dcfcf257d299a1c632016a6fcd33f4"},
                    "resources": {"prompts/r.md": "66a045b452102c59d840ec097d59d9467e13a3f34f6494e539ffd32c1bb35f18"},
                },
            })
        );
    }

    #[test]
    fn integrated_price_of_a_plan() {
        let (plan, _) = refusals_plan();
        let document = price_document(&plan);
        assert_eq!(
            document["estimate"],
            json!({"low_usd": 0.013, "high_usd": 0.046})
        );
        assert_eq!(document["phases"][0]["calls"], json!([3, 3]));
        assert_eq!(
            document["phases"][1]["then"],
            json!(["unbounded (up to 1)"])
        );
        let (plan, _) = tiered_plan();
        let document = price_document(&plan);
        assert_eq!(
            document["estimate"],
            json!({"low_usd": 2.49, "high_usd": 6.8625})
        );
    }

    #[test]
    fn display_bindings_do_not_change_identities_prices_or_the_plan_digest() {
        let (mut plan, planner) = tiny_plan();
        let digest = plan_digest(&plan, &planner);
        let identities = identity_document(&plan);
        let prices = price_document(&plan);
        let instance = plan.expansion.instances.values_mut().next().unwrap();
        instance.bindings.push(crate::expand::wiring::Binding {
            source: "display_only#1".into(),
            source_port: "image".into(),
            target_port: "prompt".into(),
            source_kind: crate::expand::wiring::SourceKind::Output,
        });
        instance.interface_bindings = instance.bindings.clone();
        instance.display_scope = Some("scope:display_only#".into());
        assert_eq!(
            graph_document(&plan, &planner)["instances"][0]["bindings"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        plan.expansion.scopes.push(crate::expand::display::Scope {
            id: "scope:display_only#".into(),
            parent: None,
            kind: "workflow",
            path: "display_only".into(),
            step: "display_only".into(),
            take: Vec::new(),
            title: "Display only".into(),
            source: Some("workflows/display.yaml".into()),
            ports: Default::default(),
            input_bindings: Vec::new(),
            output_bindings: Vec::new(),
            nodes: Vec::new(),
            pending: Vec::new(),
        });
        assert_eq!(plan_digest(&plan, &planner), digest);
        assert_eq!(identity_document(&plan), identities);
        assert_eq!(price_document(&plan), prices);
    }
}
