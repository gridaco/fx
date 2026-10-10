//! The cell rule and the order key (spec/layout.md §4), on hand-written fx-graph-v1 plans.

use super::plan_report;
use serde_json::{Value, json};
use std::time::Instant;

/// One instance: `(id, step, key, take, reads)`.
type Planned<'a> = (&'a str, &'a str, Option<&'a str>, u64, &'a [&'a str]);

/// A plan whose `steps` are declared in the given order.
fn plan(steps: &[&str], instances: &[Planned]) -> Value {
    let declared: serde_json::Map<String, Value> = steps
        .iter()
        .enumerate()
        .map(|(order, step)| (step.to_string(), json!({ "order": order })))
        .collect();
    let instances: Vec<Value> = instances
        .iter()
        .map(|(id, step, key, take, reads)| {
            json!({
                "id": id, "path": id, "step": step, "key": key, "take": [take],
                "reads": reads, "state": "planned"
            })
        })
        .collect();
    json!({ "steps": declared, "instances": instances, "pending": [] })
}

fn cell(report: &Value, address: &str) -> (u64, u64) {
    let cell = &report["cells"][address];
    (
        cell["column"].as_u64().expect("column"),
        cell["row"].as_u64().expect("row"),
    )
}

fn report(plan: &Value) -> Value {
    serde_json::to_value(plan_report(plan)).expect("serializable")
}

#[test]
fn section_4_example() {
    let variants = ["warm", "cool", "mono", "dusk", "night"];
    let mut steps = vec!["seed", "propose"];
    steps.extend(variants);
    steps.extend(["sheet", "notes", "index"]);
    let mut instances = vec![
        ("seed#1", "seed", None, 1, &[][..]),
        ("propose#1", "propose", None, 1, &["seed#1"][..]),
        ("propose#2", "propose", None, 2, &["seed#1"][..]),
    ];
    let ids: Vec<String> = variants.iter().map(|name| format!("{name}#1")).collect();
    for (name, id) in variants.iter().zip(&ids) {
        instances.push((id, name, None, 1, &["propose#1"][..]));
    }
    let reads: Vec<&str> = ids.iter().map(String::as_str).collect();
    instances.push(("sheet#1", "sheet", None, 1, &reads));
    instances.push(("notes#1", "notes", None, 1, &[]));
    instances.push(("index#1", "index", None, 1, &["notes#1"]));
    let report = report(&plan(&steps, &instances));
    let expected = [
        ("seed", (0, 0)),
        ("propose", (1, 0)),
        ("warm", (2, 0)),
        ("cool", (2, 1)),
        ("mono", (2, 2)),
        ("dusk", (3, 0)),
        ("night", (3, 1)),
        ("sheet", (4, 0)),
        ("notes", (5, 0)),
        ("index", (6, 0)),
    ];
    for (address, at) in expected {
        assert_eq!(cell(&report, address), at, "{address}");
    }
    assert_eq!(report["kind"], "fx-layout-report-v1");
    assert_eq!(report["state"], "none");
    assert_eq!(report["cursor"], Value::Null);
}

#[test]
fn three_readers_keep_one_column_and_take_the_next_free_row() {
    let report = report(&plan(
        &["seed", "a", "b", "c", "after"],
        &[
            ("seed#1", "seed", None, 1, &[]),
            ("a#1", "a", None, 1, &["seed#1"]),
            ("b#1", "b", None, 1, &["seed#1"]),
            ("c#1", "c", None, 1, &["seed#1"]),
            ("after#1", "after", None, 1, &["c#1"]),
        ],
    ));
    assert_eq!(cell(&report, "a"), (1, 0));
    assert_eq!(cell(&report, "b"), (1, 1));
    assert_eq!(cell(&report, "c"), (1, 2));
    // A successor takes its predecessor's row.
    assert_eq!(cell(&report, "after"), (2, 2));
}

#[test]
fn a_long_chain_never_snakes() {
    let steps: Vec<String> = (0..30).map(|index| format!("s{index}")).collect();
    let ids: Vec<String> = steps.iter().map(|step| format!("{step}#1")).collect();
    let instances: Vec<_> = (0..30)
        .map(|index| {
            let reads: &[&str] = if index == 0 { &[] } else { &[""] };
            (ids[index].as_str(), steps[index].as_str(), None, 1, reads)
        })
        .collect();
    let mut plan = plan(
        &steps.iter().map(String::as_str).collect::<Vec<_>>(),
        &instances,
    );
    for index in 1..30 {
        plan["instances"][index]["reads"] = json!([ids[index - 1]]);
    }
    let report = report(&plan);
    for (index, step) in steps.iter().enumerate() {
        assert_eq!(cell(&report, step), (index as u64, 0), "{step}");
    }
}

#[test]
fn an_edge_back_to_an_earlier_slot_in_a_cycle_is_ignored_for_depth() {
    let report = report(&plan(
        &["a", "b"],
        &[
            ("a#1", "a", None, 1, &["b#1"]),
            ("b#1", "b", None, 1, &["a#1"]),
        ],
    ));
    assert_eq!(cell(&report, "a"), (0, 0));
    assert_eq!(cell(&report, "b"), (1, 0));
}

#[test]
fn group_frames_share_one_cell_set_and_edges_lift_to_the_common_container() {
    let mut plan = plan(
        &["seed", "badge", "badge.draw", "badge.check", "close"],
        &[
            ("seed#1", "seed", None, 1, &[]),
            ("sun.draw#1", "badge.draw", None, 1, &["seed#1"]),
            ("sun.check#1", "badge.check", None, 1, &["sun.draw#1"]),
            ("stone.draw#1", "badge.draw", None, 1, &["seed#1"]),
            ("stone.check#1", "badge.check", None, 1, &["stone.draw#1"]),
            (
                "close#1",
                "close",
                None,
                1,
                &["sun.check#1", "stone.check#1"],
            ),
        ],
    );
    plan["scopes"] = json!([]);
    let report = report(&plan);
    assert_eq!(cell(&report, "seed"), (0, 0));
    assert_eq!(cell(&report, "badge"), (1, 0));
    assert_eq!(cell(&report, "close"), (2, 0));
    assert_eq!(cell(&report, "badge.draw"), (0, 0));
    assert_eq!(cell(&report, "badge.check"), (1, 0));
    let addresses: Vec<&str> = report["cells"]
        .as_object()
        .expect("cells")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        addresses,
        ["seed", "badge", "close", "badge.draw", "badge.check"],
        "container by container from the root"
    );
}

#[test]
fn a_pending_repeat_has_a_slot_after_what_it_waits_on() {
    let mut plan = plan(&["seed", "dynamic"], &[("seed#1", "seed", None, 1, &[])]);
    plan["pending"] = json!([{ "path": "dynamic", "step": "dynamic", "waiting_on": ["seed#1"], "phase": 2, "max": 8 }]);
    let report = report(&plan);
    assert_eq!(cell(&report, "dynamic"), (1, 0));
}

#[test]
fn members_follow_the_expansion_order_and_absent_ones_are_left_out() {
    let mut plan = plan(
        &["draw"],
        &[
            ("draw['b']#1", "draw", Some("b"), 1, &[]),
            ("draw['a']#1", "draw", Some("a"), 1, &[]),
            ("draw['a']#2", "draw", Some("a"), 2, &[]),
        ],
    );
    plan["instances"][2]["state"] = json!("absent");
    let report = report(&plan);
    assert_eq!(report["order"], json!(["draw['b']#1", "draw['a']#1"]));
}

#[test]
fn a_record_without_declared_steps_orders_slots_by_plan_position() {
    let mut plan = plan(
        &[],
        &[
            ("late#1", "late", None, 1, &[]),
            ("early#1", "early", None, 1, &[]),
        ],
    );
    plan.as_object_mut().expect("plan").remove("steps");
    let report = report(&plan);
    // Two islands side by side, the first planned first.
    assert_eq!(cell(&report, "late"), (0, 0));
    assert_eq!(cell(&report, "early"), (1, 0));
}

#[test]
fn the_same_records_give_the_same_report_within_budget() {
    // 500 instances: 50 chains of 10 that all read one seed.
    let mut steps = vec!["seed".to_string()];
    let mut instances = vec![json!({"id": "seed#1", "path": "seed", "step": "seed", "take": [1]})];
    for chain in 0..50 {
        for link in 0..10 {
            let step = format!("c{chain}_{link}");
            let reads = if link == 0 {
                json!(["seed#1"])
            } else {
                json!([format!("c{chain}_{}#1", link - 1)])
            };
            instances.push(json!({"id": format!("{step}#1"), "path": step, "step": step, "take": [1], "reads": reads}));
            steps.push(step);
        }
    }
    let declared: serde_json::Map<String, Value> = steps
        .iter()
        .enumerate()
        .map(|(order, step)| (step.clone(), json!({ "order": order })))
        .collect();
    let plan = json!({ "steps": declared, "instances": instances });
    let started = Instant::now();
    let first = report(&plan);
    let elapsed = started.elapsed();
    assert_eq!(first, report(&plan));
    let budget = if cfg!(debug_assertions) { 160 } else { 16 };
    assert!(elapsed.as_millis() < budget, "{elapsed:?}");
    // The seed's 50 readers wrap into columns of three.
    assert_eq!(cell(&first, "c0_0"), (1, 0));
    assert_eq!(cell(&first, "c3_0"), (2, 0));
}

#[test]
fn an_empty_step_is_one_root_slot() {
    let report = report(&plan(&[], &[("a#1", "", None, 1, &[])]));
    assert_eq!(cell(&report, ""), (0, 0));
}

#[test]
fn an_expanded_repeat_keeps_its_plan_cell_in_the_run_view() {
    let steps = ["seed", "split", "a", "b", "c", "draw"];
    let mut plan = plan(
        &steps,
        &[
            ("seed#1", "seed", None, 1, &[]),
            ("split#1", "split", None, 1, &["seed#1"]),
            ("a#1", "a", None, 1, &["seed#1"]),
            ("b#1", "b", None, 1, &["a#1"]),
            ("c#1", "c", None, 1, &["b#1"]),
        ],
    );
    let repeat = json!({
        "path": "draw", "step": "draw", "max": 8, "phase": 2, "high_usd": 0,
        "waiting_on": ["split#1"]
    });
    plan["kind"] = json!("fx-graph-v1");
    plan["workflow"] = json!({ "id": "demo", "title": "Demo" });
    plan["pending"] = json!([repeat]);
    let planned = cell(&report(&plan), "draw");
    // The repeat expands to no item: the newest snapshot records no pending entry.
    let listed: Vec<Value> = steps[..5]
        .iter()
        .map(|step| json!({ "id": format!("{step}#1"), "step": step, "take": [1], "key": null }))
        .collect();
    let updated = |pending: Value| {
        json!({
            "kind": "fx-run-events-v1", "event": "scopes_updated", "invocation_id": "run-1",
            "offset_ms": 0, "scopes": [], "node_interface_bindings": {},
            "instances": listed, "pending": pending
        })
    };
    let folder = tempfile::tempdir().expect("folder");
    let root = folder.path().canonicalize().expect("canonical folder");
    std::fs::write(root.join("plan.json"), plan.to_string()).expect("plan");
    let log = format!("{}\n{}\n", updated(json!([repeat])), updated(json!([])));
    std::fs::write(root.join("events.jsonl"), log).expect("events");
    let snapshot = crate::read::read_inventory(&root).expect("projection");
    let run = serde_json::to_value(super::run_report(&plan, &snapshot, "cursor".into()))
        .expect("serializable");
    assert_eq!(cell(&run, "draw"), planned);
}
