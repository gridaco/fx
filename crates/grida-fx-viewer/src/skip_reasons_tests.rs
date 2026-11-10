use crate::read;
use serde_json::{Value, json};

fn project(mut events: Vec<Value>) -> read::RunDocument {
    let root = tempfile::tempdir().unwrap();
    let plan = json!({
        "kind":"fx-graph-v1", "plan":"a".repeat(64),
        "workflow":{"id":"acme", "title":"Candidate checks"},
        "steps":{}, "inputs":{}, "pending":[],
        "estimate":{"low_usd":0,"high_usd":0},
        "instances":[
            {"id":"draw#1","path":"draw","step":"draw","take":[1],"key":null,"uses":"acme/draw@1","state":"planned","reads":[],"with":{}},
            {"id":"draw#2","path":"draw","step":"draw","take":[2],"key":null,"uses":"acme/draw@1","state":"planned","reads":[],"with":{}},
            {"id":"draw#3","path":"draw","step":"draw","take":[3],"key":null,"uses":"acme/draw@1","state":"maybe","reads":[],"with":{},"reason":"only if the take before it is rejected"},
            {"id":"review#3","path":"review","step":"review","take":[3],"key":null,"uses":"acme/review@1","state":"maybe","reads":["draw#3"],"with":{}}
        ]
    });
    for event in &mut events {
        event["kind"] = json!("fx-run-events-v1");
        event["plan"] = plan["plan"].clone();
        if event.get("invocation_id").is_none() {
            event["invocation_id"] = json!("first");
        }
        event["offset_ms"] = json!(0);
    }
    std::fs::write(root.path().join("plan.json"), plan.to_string()).unwrap();
    std::fs::write(
        root.path().join("events.jsonl"),
        events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>(),
    )
    .unwrap();
    read::read_inventory(&root.path().canonicalize().unwrap())
        .unwrap()
        .document
}

fn cause(code: &str, sources: &[&str]) -> Value {
    json!({"code":code,"sources":sources})
}

fn snapshot(reason: Value) -> Value {
    json!({"event":"scopes_updated","scopes":[],"skip_reasons":{"draw#3":reason}})
}

fn finished() -> Value {
    json!({"event":"run_finished","ok":true,"charged_usd":0,"outputs":{}})
}

fn reason(document: &read::RunDocument, id: &str) -> Option<Value> {
    document
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap()
        .skip_reason
        .clone()
}

#[test]
fn terminal_unused_takes_show_recorded_causes_without_creating_members() {
    let accepted = cause("earlier_take_accepted", &["draw#2"]);
    let document = project(vec![
        json!({"event":"run_started"}),
        json!({"event":"scopes_updated","scopes":[],"skip_reasons":{"draw#3":accepted,"review#3":accepted}}),
        finished(),
    ]);
    assert_eq!(document.nodes.len(), 4);
    assert_eq!(reason(&document, "draw#3"), Some(accepted.clone()));
    assert_eq!(reason(&document, "review#3"), Some(accepted));
    assert!(
        document
            .nodes
            .iter()
            .find(|node| node.id == "draw#3")
            .unwrap()
            .error
            .is_none()
    );
    let value = serde_json::to_value(document).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-viewer-run-v1.schema.json"
    ))
    .unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&value)
        .unwrap();
}

#[test]
fn pending_instances_do_not_get_a_final_reason_and_new_snapshots_replace_prior_causes() {
    let events = vec![
        json!({"event":"run_started"}),
        snapshot(cause("condition_false", &[])),
    ];
    let unfinished = project(events.clone());
    assert_eq!(unfinished.state, "unfinished");
    assert!(reason(&unfinished, "draw#3").is_none());
    let mut replaced = events;
    replaced.push(json!({"event":"scopes_updated","scopes":[],"skip_reasons":{}}));
    replaced.push(finished());
    assert!(reason(&project(replaced), "draw#3").is_none());
}

#[test]
fn resume_clears_previous_causes_and_native_skip_causes_survive_empty_snapshots() {
    let mut events = vec![
        json!({"event":"run_started"}),
        snapshot(cause("earlier_take_accepted", &["draw#2"])),
        finished(),
        json!({"event":"run_started","invocation_id":"resumed"}),
    ];
    assert!(reason(&project(events.clone()), "draw#3").is_none());
    events.extend([
        json!({"event":"node_started","id":"draw#3","path":"draw","with":{},"reads":[]}),
        json!({"event":"node_skipped","id":"draw#3","path":"draw","skip_reason":cause("no_candidate", &[]),"duration_ms":0}),
        json!({"event":"scopes_updated","scopes":[],"skip_reasons":{}}),
    ]);
    assert_eq!(
        reason(&project(events.clone()), "draw#3"),
        Some(cause("no_candidate", &[]))
    );
    events.push(json!({"event":"node_started","id":"draw#3","path":"draw","with":{},"reads":[]}));
    assert!(reason(&project(events.clone()), "draw#3").is_none());
    events.extend([
        json!({"event":"node_finished","id":"draw#3","path":"draw","outputs":{},"cache":"miss","duration_ms":0}),
        finished(),
    ]);
    assert!(reason(&project(events), "draw#3").is_none());
}

#[test]
fn causes_require_known_related_instances_and_do_not_preserve_unknown_payloads() {
    for malformed in [
        cause("future_cause", &[]),
        cause("earlier_take_accepted", &["missing#1"]),
        cause("upstream_failed", &["draw#1", "draw#1"]),
        json!({"code":"condition_false"}),
        json!({"code":"condition_false","sources":[3]}),
    ] {
        assert!(reason(&project(vec![snapshot(malformed), finished()]), "draw#3").is_none());
    }
    let mut recorded = cause("condition_false", &[]);
    recorded["private_text"] = json!("withheld-author-value");
    let document = project(vec![snapshot(recorded), finished()]);
    assert_eq!(
        reason(&document, "draw#3"),
        Some(cause("condition_false", &[]))
    );
    assert!(
        !serde_json::to_string(&document)
            .unwrap()
            .contains("withheld-author-value")
    );
}

#[test]
fn older_records_keep_the_status_without_an_invented_cause() {
    let document = project(vec![finished()]);
    assert!(document.nodes.iter().all(|node| node.state == "skipped"));
    assert!(document.nodes.iter().all(|node| node.skip_reason.is_none()));
}
