use super::*;
use axum::body::to_bytes;
use axum::http::Request;
use grida_fx_core::value::file_digest;
use serde_json::{Value, json};
use std::io::Write;
use tempfile::TempDir;
use tower::ServiceExt;

pub(super) struct Fixture {
    directory: TempDir,
    pub(super) digest: String,
    bytes: Vec<u8>,
}

impl Fixture {
    pub(super) fn new(kind: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let bytes = b"a synthetic offline artifact".to_vec();
        let digest = file_digest(&bytes);
        let mut plan = json!({
            "kind": "fx-graph-v1", "plan": "a".repeat(64),
            "workflow": {"id": "sample", "title": "Sample workflow"},
            "instances": [{"id":"draw#1", "path":"draw", "uses":"acme/draw@1", "state":"planned", "reads":[], "with":{}}],
            "steps": {"draw": {"title":"Draw the sample"}},
            "inputs": {"caption":"A synthetic sample"},
            "estimate": {"low_usd":0,"high_usd":0}, "stand_in":true
        });
        plan["pending"] = json!([]);
        plan["estimate"]["ceiling_usd"] = Value::Null;
        let instance = &mut plan["instances"][0];
        instance["step"] = json!("draw");
        instance["type"] = json!("acme/draw@1");
        instance["take"] = json!([1]);
        instance["key"] = Value::Null;
        instance["identity"] = Value::Null;
        instance["phase"] = json!(1);
        instance["price"] = json!({"low_usd":0,"high_usd":0});
        instance["routes"] = json!({});
        instance["waiting_on"] = json!([]);
        instance["judged_by"] = json!([]);
        instance["view"] = json!(false);
        instance["needs"] = json!([]);
        instance["judges"] = Value::Null;
        grida_fx_core::docs::validate(Schema::Graph, &plan, "fixture").unwrap();
        std::fs::write(directory.path().join("plan.json"), plan.to_string()).unwrap();
        let encoded =
            json!({"file":{"digest":digest,"kind":kind,"name":"sample","size":bytes.len()}});
        let events = [
            json!({"event":"run_started", "stand_in":true}),
            json!({"event":"node_started","id":"draw#1","path":"draw","uses":"acme/draw@1","with":{"input":{"file":{"digest":"b".repeat(64),"kind":"image/png","name":"input.png","size":8}}},"reads":[]}),
            json!({"event":"node_finished","id":"draw#1","path":"draw","outputs":{"image":encoded},"cache":"hit","duration_ms":3}),
            json!({"event":"node_started","id":"later#1","path":"later","uses":"acme/check@1","with":{},"reads":["draw#1"]}),
            json!({"event":"node_failed","id":"later#1","path":"later","error":"Synthetic failure","duration_ms":4}),
            json!({"event":"run_finished","ok":false,"charged_usd":0,"outputs":{"result":encoded}}),
        ];
        let mut file = FileForEvents::new(directory.path());
        for event in events {
            file.append(event);
        }
        let relative = grida_fx_runtime::folder::step_file_path("draw", "image", None, kind);
        let target = directory.path().join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, &bytes).unwrap();
        Self {
            directory,
            digest,
            bytes,
        }
    }

    pub(super) fn root(&self) -> PathBuf {
        self.directory.path().canonicalize().unwrap()
    }

    fn app(&self) -> Router {
        router(AppState {
            source: Source::Run(self.root()),
            authority: "127.0.0.1:43123".into(),
            artifact_prefix: String::new(),
        })
    }

    fn request(path: &str) -> axum::http::request::Builder {
        Request::builder()
            .uri(path)
            .header(header::HOST, "127.0.0.1:43123")
    }
}

struct FileForEvents(std::fs::File);
impl FileForEvents {
    fn new(root: &Path) -> Self {
        Self(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("events.jsonl"))
                .unwrap(),
        )
    }
    fn append(&mut self, mut event: Value) {
        event["kind"] = json!("fx-run-events-v1");
        event["plan"] = json!("a".repeat(64));
        event["invocation_id"] = json!("offline-fixture");
        event["offset_ms"] = json!(0);
        writeln!(self.0, "{event}").unwrap();
    }
}

fn imported_scope(nodes: &[&str]) -> Value {
    json!({
        "id":"scope:imported#", "parent":null, "kind":"workflow", "path":"imported",
        "step":"imported", "take":[], "title":"Imported sample", "source":"workflows/sample.yaml",
        "ports":{"inputs":{"input":{"type":"file","kind":"image"}},"outputs":["preview"]},
        "input_bindings":[], "output_bindings":[], "nodes":nodes, "pending":[]
    })
}

#[test]
fn scope_projection_uses_latest_snapshot_and_clears_old_interfaces() {
    let fixture = Fixture::new("image/png");
    let path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let bindings = json!([{"source":"scope:imported#","source_port":"input","target_port":"input","source_kind":"scope_input"}]);
    plan["scopes"] = json!([imported_scope(&["draw#1"])]);
    plan["instances"][0]["interface_bindings"] = bindings.clone();
    std::fs::write(&path, plan.to_string()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let first = read::read_inventory(&fixture.root()).unwrap().document;
    assert_eq!(first.scopes, Some(plan["scopes"].clone()));
    assert_eq!(first.nodes[0].interface_bindings, Some(bindings));

    let dynamic = "imported.dynamic#1";
    let mut events = FileForEvents::new(&fixture.root());
    let scope = imported_scope(&[dynamic]);
    events.append(json!({"event":"scopes_updated","scopes":[scope],"node_interface_bindings":{
        "draw#1":[], dynamic:[{"source":"scope:imported#","source_port":"input","target_port":"input","source_kind":"scope_input"}]
    }}));
    events.append(json!({"event":"node_skipped","id":dynamic,"path":"imported.dynamic","reason":"blocked","blocked":true}));
    drop(events);
    let document = read::read_inventory(&fixture.root()).unwrap().document;
    assert!(document.warnings.is_empty(), "{:?}", document.warnings);
    assert_eq!(document.scopes, Some(json!([scope])));
    assert_eq!(document.nodes[0].interface_bindings, Some(json!([])));
    let skipped = document.nodes.iter().find(|n| n.id == dynamic).unwrap();
    assert_eq!(skipped.state, "skipped");
    assert_eq!(
        skipped.interface_bindings.as_ref().unwrap()[0]["source_kind"],
        "scope_input"
    );
    assert_eq!(std::fs::read(path).unwrap(), before);

    let mut events = FileForEvents::new(&fixture.root());
    events.append(
        json!({"event":"scopes_updated","scopes":[],"node_interface_bindings":{"draw#1":[]}}),
    );
    drop(events);
    let document = read::read_inventory(&fixture.root()).unwrap().document;
    assert_eq!(document.scopes, Some(json!([])));
    assert!(
        document
            .nodes
            .iter()
            .find(|n| n.id == dynamic)
            .unwrap()
            .interface_bindings
            .is_none()
    );
}

#[test]
fn invalid_scope_metadata_cannot_hide_nodes_or_trigger_source_access() {
    for change in ["cycle", "parent", "member", "source", "port", "shape"] {
        let fixture = Fixture::new("image/png");
        let mut scope = imported_scope(&["draw#1"]);
        let mut bindings = json!({"draw#1":[]});
        match change {
            "cycle" => scope["parent"] = json!("scope:imported#"),
            "parent" => scope["parent"] = json!("scope:missing#"),
            "member" => scope["nodes"] = json!(["missing#1"]),
            "source" => scope["source"] = json!("/private/source.yaml"),
            "port" => {
                bindings["draw#1"] = json!([{"source":"scope:imported#","source_port":"missing","target_port":"input","source_kind":"scope_input"}])
            }
            _ => bindings = Value::Null,
        }
        let mut events = FileForEvents::new(&fixture.root());
        events.append(
            json!({"event":"scopes_updated","scopes":[scope],"node_interface_bindings":bindings}),
        );
        drop(events);
        let document = read::read_inventory(&fixture.root()).unwrap().document;
        assert!(document.scopes.is_none(), "{change}");
        assert_eq!(document.nodes.len(), 2, "{change}");
        assert_eq!(document.nodes[0].state, "succeeded");
        assert!(
            document
                .nodes
                .iter()
                .all(|n| n.interface_bindings.is_none())
        );
        assert!(
            document
                .warnings
                .iter()
                .any(|w| w.contains("scope metadata")),
            "{change}"
        );
    }
}

#[test]
fn legacy_runs_do_not_acquire_guessed_scopes() {
    let fixture = Fixture::new("image/png");
    let document =
        serde_json::to_value(read::read_inventory(&fixture.root()).unwrap().document).unwrap();
    assert!(document.get("scopes").is_none());
    assert!(
        document["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.get("interface_bindings").is_none())
    );
}

#[test]
fn static_scope_forests_are_validated_and_preserved_verbatim() {
    let mut graph = static_plan();
    let id = graph["instances"][0]["id"].as_str().unwrap().to_string();
    graph["scopes"] = json!([imported_scope(&[&id])]);
    graph["instances"][0]["interface_bindings"] = json!([{
        "source":"scope:imported#","source_port":"input","target_port":"input","source_kind":"scope_input"
    }]);
    let Source::Plan(saved) = plan_source(graph.clone()).unwrap() else {
        panic!("expected a plan")
    };
    assert_eq!(*saved, graph);
    for field in ["parent", "nodes", "source"] {
        let mut invalid = graph.clone();
        invalid["scopes"][0][field] = match field {
            "parent" => json!("scope:imported#"),
            "nodes" => json!(["unknown#1"]),
            _ => json!("/private/workflow.yaml"),
        };
        assert!(plan_source(invalid).is_err(), "{field}");
    }
}

#[test]
fn run_projection_keeps_plan_ports_and_bindings_without_loading_author_code() {
    let fixture = Fixture::new("image/png");
    let plan_path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
    let ports = json!({"inputs":{"input":"image"},"outputs":{"image":"image/png","report":"json"},"params":{"enabled":{"type":"boolean"}}});
    let bindings = json!([{"source":"origin#1","source_port":"picture","target_port":"input","source_kind":"output"}]);
    plan["types"] = json!({"acme/draw@1":{"ports":ports}});
    plan["instances"][0]["bindings"] = bindings.clone();
    plan["instances"][0]["needs"] = json!(["barrier#1"]);
    plan["instances"][0]["judges"] = Value::Null;
    std::fs::write(&plan_path, plan.to_string()).unwrap();
    let before = std::fs::read(&plan_path).unwrap();
    let snapshot = read::read_inventory(&fixture.root()).unwrap();
    let document = serde_json::to_value(snapshot.document).unwrap();
    let draw = &document["nodes"][0];
    assert_eq!(draw["ports"], ports);
    assert_eq!(draw["bindings"], bindings);
    assert_eq!(draw["needs"], json!(["barrier#1"]));
    assert_eq!(draw["judges"], Value::Null);
    assert_eq!(std::fs::read(&plan_path).unwrap(), before);
    assert!(document["nodes"][1].get("ports").is_none());
    assert!(document["nodes"][1].get("bindings").is_none());
}

#[test]
fn run_projection_uses_runtime_ports_and_replaces_stale_planned_bindings() {
    let fixture = Fixture::new("image/png");
    let plan_path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
    plan["instances"][0]["bindings"] = json!([{"source":"old#1","source_port":"image","target_port":"input","source_kind":"output"}]);
    std::fs::write(&plan_path, plan.to_string()).unwrap();
    let ports = json!({"inputs":{"input":"image?"},"outputs":{"image":"image/png"},"params":{}});
    let mut events = FileForEvents::new(&fixture.root());
    events.append(json!({"event":"node_started","id":"draw#1","path":"draw","uses":"acme/draw@1","reads":[],"with":{},"ports":ports,"bindings":[],"needs":[],"judges":null}));
    let dynamic_ports = json!({"inputs":{"image":"image"},"outputs":{"report":"json"},"params":{}});
    let dynamic_bindings = json!([{"source":"draw#1","source_port":"image","target_port":"image","source_kind":"output"}]);
    events.append(json!({"event":"node_started","id":"dynamic['one']#1","path":"dynamic['one']","uses":"acme/dynamic@1","reads":["draw#1"],"with":{},"ports":dynamic_ports,"bindings":dynamic_bindings,"needs":["barrier#1"],"judges":"draw#1"}));
    drop(events);
    let document =
        serde_json::to_value(read::read_inventory(&fixture.root()).unwrap().document).unwrap();
    assert_eq!(document["nodes"][0]["ports"], ports);
    assert_eq!(document["nodes"][0]["bindings"], json!([]));
    let dynamic = document["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "dynamic['one']#1")
        .unwrap();
    assert_eq!(dynamic["ports"], dynamic_ports);
    assert_eq!(dynamic["bindings"], dynamic_bindings);
    assert_eq!(dynamic["needs"], json!(["barrier#1"]));
    assert_eq!(dynamic["judges"], "draw#1");
}

#[test]
fn dynamic_nodes_terminated_before_start_keep_recorded_ports() {
    let fixture = Fixture::new("image/png");
    let ports = json!({"inputs":{"image":"image"},"outputs":{"report":"json"},"params":{}});
    let bindings = json!([{"source":"draw#1","source_port":"image","target_port":"image","source_kind":"output"}]);
    let mut events = FileForEvents::new(&fixture.root());
    for (event, id) in [
        ("node_skipped", "dynamic['one'].blocked#1"),
        ("node_failed", "dynamic['one'].failed#1"),
    ] {
        events.append(json!({"event":event,"id":id,"path":id.split('#').next().unwrap(),"uses":"acme/inspect@1","reads":["draw#1"],"with":{},"ports":ports,"bindings":bindings,"needs":[],"judges":null,"reason":"Synthetic upstream failure","error":"Synthetic failure","blocked":true}));
    }
    drop(events);
    let document =
        serde_json::to_value(read::read_inventory(&fixture.root()).unwrap().document).unwrap();
    for (id, state) in [
        ("dynamic['one'].blocked#1", "skipped"),
        ("dynamic['one'].failed#1", "failed"),
    ] {
        let entry = document["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap();
        assert_eq!(entry["ports"], ports);
        assert_eq!(entry["bindings"], bindings);
        assert_eq!(entry["uses"], "acme/inspect@1");
        assert_eq!(entry["state"], state);
        assert!(entry["duration_ms"].is_null());
    }
}

pub(super) fn static_plan() -> Value {
    let instances: Vec<Value> = ["planned", "maybe", "absent", "blocked", "failed", "done"]
        .into_iter()
        .enumerate()
        .map(|(index, state)| {
            let path = format!("step_{index}");
            json!({
                "id":format!("{path}#1"), "path":path, "step":path,
                "take":[1], "uses":"acme/example@1", "type":null,
                "with":{"input":{"pending":["source#1"]},"file":{"file":"b".repeat(64)}},
                "routes":{}, "state":state, "identity":null, "phase":1, "key":null,
                "judges":null, "judged_by":[], "waiting_on":["source#1"], "needs":[],
                "reads":["source#1"], "price":{"low_usd":0,"high_usd":0.1},
                "view":false, "reason":"Recorded planning evidence"
            })
        })
        .collect();
    json!({
        "kind":"fx-graph-v1",
        "workflow":{"id":"static-case","title":"Static plan","file":"example.py:build"},
        "instances":instances,
        "pending":[{"path":"future_items","max":3,"phase":2,"high_usd":0.3}],
        "estimate":{"low_usd":0,"high_usd":0.9,"ceiling_usd":null},
        "problems":[{"where":"future_items","message":"An example planning problem"}]
    })
}

#[tokio::test]
async fn static_plan_api_preserves_all_planner_evidence_without_mutation() {
    let graph = static_plan();
    let original = graph.clone();
    let source = plan_source(graph).unwrap();
    let app = router(AppState {
        source,
        authority: "127.0.0.1:43123".into(),
        artifact_prefix: String::new(),
    });
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(Fixture::request("/api/view").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let document: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap())
                .unwrap();
        assert_eq!(document, original);
        assert_eq!(document["kind"], "fx-graph-v1");
        assert_eq!(document["instances"].as_array().unwrap().len(), 6);
        assert!(
            document.get("nodes").is_none(),
            "a plan must not be projected as runtime state"
        );
    }
    for path in [
        "/api/run".to_string(),
        format!("/api/artifacts/{}", "b".repeat(64)),
    ] {
        for method in [Method::GET, Method::HEAD] {
            let response = app
                .clone()
                .oneshot(
                    Fixture::request(&path)
                        .method(method)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }
}

#[tokio::test]
async fn plan_bind_rejects_invalid_graphs_before_hosting() {
    let mut invalid_state = static_plan();
    invalid_state["instances"][0]["state"] = json!("running");
    let mut invalid_pending = static_plan();
    invalid_pending["pending"][0]["max"] = json!(0);
    for graph in [
        json!(null),
        json!({"kind":"fx-graph-v1"}),
        invalid_state,
        invalid_pending,
    ] {
        let error = bind_plan(graph, 0)
            .await
            .err()
            .expect("an invalid plan must be refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[tokio::test]
async fn unified_view_api_keeps_run_api_compatible() {
    let fixture = Fixture::new("json");
    let mut documents = Vec::new();
    for path in ["/api/run", "/api/view"] {
        let response = fixture
            .app()
            .oneshot(Fixture::request(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        documents.push(
            serde_json::from_slice::<Value>(
                &to_bytes(response.into_body(), 1_000_000).await.unwrap(),
            )
            .unwrap(),
        );
    }
    assert_eq!(documents[0], documents[1]);
    assert_eq!(documents[0]["kind"], "fx-viewer-run-v1");
}

#[test]
fn projects_dynamic_nodes_and_reports_missing_inputs_without_mutation() {
    let fixture = Fixture::new("image/png");
    let events_path = fixture.root().join("events.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&events_path)
        .unwrap()
        .write_all(b"{\"event\":")
        .unwrap();
    let before = std::fs::read(&events_path).unwrap();
    let snapshot = read::read_run(&fixture.root()).unwrap();
    let run = snapshot.document;
    assert_eq!(run.state, "failed");
    assert!(run.stand_in);
    assert_eq!(run.charged_usd, Some(0.0));
    assert_eq!(run.nodes.len(), 2);
    assert_eq!(run.nodes[0].title, "Draw the sample");
    assert_eq!(run.nodes[0].cache.as_deref(), Some("hit"));
    assert_eq!(run.nodes[1].reads, vec!["draw#1"]);
    assert_eq!(run.nodes[1].error.as_deref(), Some("Synthetic failure"));
    assert_eq!(run.artifacts.iter().filter(|a| a.available).count(), 1);
    assert_eq!(run.artifacts.iter().filter(|a| !a.available).count(), 1);
    assert!(
        run.warnings
            .iter()
            .any(|warning| warning.contains("unfinished event tail"))
    );
    assert_eq!(std::fs::read(events_path).unwrap(), before);
}

#[test]
fn cancellation_and_resume_follow_invocation_order() {
    let fixture = Fixture::new("json");
    assert!(
        read::read_run(&fixture.root())
            .unwrap()
            .document
            .outputs
            .get("result")
            .is_some()
    );
    let mut events = FileForEvents::new(&fixture.root());
    events.append(json!({"event":"run_started"}));
    assert_eq!(
        read::read_run(&fixture.root()).unwrap().document.outputs,
        json!({})
    );
    events.append(json!({"event":"node_started","id":"draw#1","path":"draw","reads":[],"with":{}}));
    events.append(json!({"event":"run_cancelled","charged_usd":0.02}));
    let snapshot = read::read_run(&fixture.root()).unwrap();
    assert_eq!(snapshot.document.state, "cancelled");
    assert_eq!(snapshot.document.nodes[0].state, "failed");
    assert_eq!(snapshot.document.nodes[0].cache, None);
    assert_eq!(snapshot.document.nodes[0].outputs, json!({}));
    assert_eq!(snapshot.document.outputs, json!({}));
    events.append(json!({"event":"run_started"}));
    events.append(json!({"event":"node_started","id":"draw#1","path":"draw","reads":[],"with":{}}));
    let snapshot = read::read_run(&fixture.root()).unwrap();
    assert_eq!(snapshot.document.state, "unfinished");
    assert_eq!(snapshot.document.charged_usd, None);
    assert_eq!(snapshot.document.nodes[0].state, "running");
    assert_eq!(snapshot.document.outputs, json!({}));
}

#[test]
fn failure_or_skip_after_success_does_not_keep_successful_outputs_or_cache() {
    for terminal in ["node_failed", "node_skipped"] {
        let fixture = Fixture::new("json");
        let before = read::read_run(&fixture.root()).unwrap();
        assert!(before.document.nodes[0].outputs.get("image").is_some());
        assert_eq!(before.document.nodes[0].cache.as_deref(), Some("hit"));
        FileForEvents::new(&fixture.root()).append(json!({
            "event":terminal,"id":"draw#1","path":"draw","error":"The result failed a later check"
        }));
        let after = read::read_run(&fixture.root()).unwrap();
        let node = &after.document.nodes[0];
        assert_eq!(
            node.state,
            if terminal == "node_failed" {
                "failed"
            } else {
                "skipped"
            }
        );
        assert_eq!(node.outputs, json!({}));
        assert_eq!(node.cache, None);
        assert_eq!(
            node.error.as_deref(),
            Some("The result failed a later check")
        );
    }
}

#[test]
fn artifact_inventory_and_selected_lookup_do_not_verify_unrelated_bytes() {
    let fixture = Fixture::new("image/png");
    let other = b"another recorded artifact";
    let other_digest = file_digest(other);
    let other_path = grida_fx_runtime::folder::step_file_path("other", "text", None, "text/plain");
    let placed = fixture.root().join(&other_path);
    std::fs::create_dir_all(placed.parent().unwrap()).unwrap();
    std::fs::write(placed, other).unwrap();
    FileForEvents::new(&fixture.root()).append(json!({
        "event":"node_finished","id":"other#1","path":"other","outputs":{
            "text":{"file":{"digest":other_digest,"kind":"text/plain","name":"other.txt","size":other.len()}}
        },"cache":"miss"
    }));
    read::take_verified_paths();
    let inventory = read::read_inventory(&fixture.root()).unwrap();
    assert!(
        read::take_verified_paths().is_empty(),
        "reading records must not open artifact bytes"
    );
    let (mut file, _) = inventory
        .open_artifact(&fixture.root(), &fixture.digest)
        .unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, fixture.bytes);
    assert_eq!(read::take_verified_paths(), vec!["files/draw/image.png"]);
    let snapshot = read::read_run(&fixture.root()).unwrap();
    assert_eq!(
        snapshot
            .document
            .artifacts
            .iter()
            .filter(|artifact| artifact.available)
            .count(),
        2
    );
    assert!(
        read::take_verified_paths().contains(&other_path),
        "the run response must still check full availability"
    );
}

#[test]
fn a_filename_collision_never_serves_wrong_bytes() {
    let fixture = Fixture::new("image/png");
    std::fs::write(
        fixture.root().join("files/draw/image.png"),
        vec![b'x'; fixture.bytes.len()],
    )
    .unwrap();
    let snapshot = read::read_run(&fixture.root()).unwrap();
    assert!(snapshot.document.artifacts.iter().all(|a| !a.available));
    assert!(
        snapshot
            .open_artifact(&fixture.root(), &fixture.digest)
            .is_err()
    );
    assert!(
        snapshot
            .open_artifact(&fixture.root(), &"c".repeat(64))
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn symlinks_and_traversal_are_not_read() {
    let fixture = Fixture::new("image/png");
    let foreign = tempfile::tempdir().unwrap();
    let target = foreign.path().join("image.png");
    std::fs::write(&target, &fixture.bytes).unwrap();
    let placed = fixture.root().join("files/draw/image.png");
    std::fs::remove_file(&placed).unwrap();
    std::os::unix::fs::symlink(&target, &placed).unwrap();
    let snapshot = read::read_run(&fixture.root()).unwrap();
    assert!(
        snapshot
            .open_artifact(&fixture.root(), &fixture.digest)
            .is_err()
    );
    assert!(read::confined_file(&fixture.root(), "../plan.json").is_err());
    assert!(read::confined_file(&fixture.root(), "/etc/passwd").is_err());
    std::fs::remove_file(fixture.root().join("plan.json")).unwrap();
    std::os::unix::fs::symlink(&target, fixture.root().join("plan.json")).unwrap();
    assert!(read::read_run(&fixture.root()).is_err());
}

#[tokio::test]
async fn serves_json_verified_bytes_head_and_ranges() {
    let fixture = Fixture::new("video/mp4");
    let response = fixture
        .app()
        .oneshot(Fixture::request("/api/run").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap();
    assert_eq!(value["kind"], "fx-viewer-run-v1");
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-viewer-run-v1.schema.json"
    ))
    .unwrap();
    assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
    let url = format!("/api/artifacts/{}", fixture.digest);
    let response = fixture
        .app()
        .oneshot(
            Fixture::request(&url)
                .header(header::RANGE, "bytes=2-6")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()[header::CONTENT_RANGE],
        format!("bytes 2-6/{}", fixture.bytes.len())
    );
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap(),
        &fixture.bytes[2..7]
    );
    let response = fixture
        .app()
        .oneshot(
            Fixture::request(&url)
                .method(Method::HEAD)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        fixture.bytes.len().to_string()
    );
    assert!(
        to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty()
    );
    let response = fixture
        .app()
        .oneshot(
            Fixture::request(&url)
                .header(header::RANGE, "bytes=900-")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test]
async fn rejects_remote_origin_host_and_unknown_digests() {
    let fixture = Fixture::new("image/png");
    for request in [
        Request::builder()
            .uri("/api/run")
            .header(header::HOST, "foreign.example"),
        Fixture::request("/api/run").header(header::ORIGIN, "https://foreign.example"),
        Fixture::request("/api/run").header("sec-fetch-site", "cross-site"),
    ] {
        assert_eq!(
            fixture
                .app()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let path = format!("/api/artifacts/{}", "c".repeat(64));
    let response = fixture
        .app()
        .oneshot(Fixture::request(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn active_artifacts_are_downloads_and_missing_assets_are_not_html() {
    let fixture = Fixture::new("text/html");
    let path = format!("/api/artifacts/{}", fixture.digest);
    let response = fixture
        .app()
        .oneshot(Fixture::request(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment"
    );
    assert!(
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    let response = fixture
        .app()
        .oneshot(Fixture::request("/missing.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = fixture
        .app()
        .oneshot(Fixture::request("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
}

#[test]
fn byte_ranges_cover_suffixes_and_refuse_multipart() {
    assert_eq!(byte_range("bytes=-4", 10), Some((6, 9)));
    assert_eq!(byte_range("bytes=3-", 10), Some((3, 9)));
    assert_eq!(byte_range("bytes=0-999", 10), Some((0, 9)));
    assert_eq!(byte_range("bytes=-0", 10), None);
    assert_eq!(byte_range("bytes=2-1", 10), None);
    assert_eq!(byte_range("bytes=0-1,3-4", 10), None);
    assert_eq!(byte_range("bytes=0-", 0), None);
}

#[tokio::test]
async fn observation_snapshot_and_batches_share_one_record_frontier() {
    let fixture = Fixture::new("image/png");
    let response = fixture
        .app()
        .oneshot(
            Fixture::request("/api/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let captured: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(captured["kind"], "fx-run-snapshot-v1");
    assert_eq!(captured["view"]["state"], "failed");
    let cursor = captured["cursor"].as_str().unwrap();
    let mut log = FileForEvents::new(&fixture.root());
    log.append(json!({"event":"run_started"}));
    log.append(
        json!({"event":"node_started", "id":"draw#1", "path":"draw", "with":{}, "reads":[]}),
    );
    let response = fixture
        .app()
        .oneshot(
            Fixture::request(&format!("/api/events?after={cursor}&limit=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let batch: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(batch["events"].as_array().unwrap().len(), 1);
    assert_eq!(batch["events"][0]["event"], "run_started");
    assert_eq!(batch["has_more"], true);
    // The initial projection never includes events appended after its capture.
    let observed = grida_fx_runtime::observation::snapshot(&fixture.root()).unwrap();
    let view = read::read_observed(&fixture.root(), &observed)
        .unwrap()
        .document;
    log.append(json!({"event":"run_finished", "ok":true, "outputs":{}}));
    assert_eq!(view.state, "unfinished");
    assert_eq!(view.nodes[0].state, "running");
}

#[tokio::test]
async fn observation_errors_are_structured_and_do_not_leak_source_paths() {
    let fixture = Fixture::new("image/png");
    for (path, status, code) in [
        (
            "/api/events?after=broken",
            StatusCode::CONFLICT,
            "invalid_cursor",
        ),
        (
            "/api/events?limit=0",
            StatusCode::BAD_REQUEST,
            "invalid_limit",
        ),
        (
            "/api/events?limit=many",
            StatusCode::BAD_REQUEST,
            "invalid_limit",
        ),
        (
            "/api/events?limit=1&limit=2",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            "/api/events?after=a&after=b",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
    ] {
        let response = fixture
            .app()
            .oneshot(Fixture::request(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["kind"], "fx-run-observation-error-v1");
        assert_eq!(value["code"], code);
        assert!(!String::from_utf8_lossy(&body).contains(fixture.root().to_str().unwrap()));
    }
}

#[test]
fn running_cost_follows_settlements_and_resume_baseline() {
    let fixture = Fixture::new("image/png");
    let mut log = FileForEvents::new(&fixture.root());
    log.append(json!({"event":"run_started", "charged_usd":0.4}));
    log.append(
        json!({"event":"budget_settled", "charged_usd":0.1, "node_id":"draw#1", "reported":true}),
    );
    let current = read::read_run(&fixture.root()).unwrap().document;
    assert_eq!(current.state, "unfinished");
    assert!((current.charged_usd.unwrap() - 0.5).abs() < 0.0000001);
    log.append(json!({"event":"run_cancelled", "charged_usd":0.6}));
    assert_eq!(
        read::read_run(&fixture.root())
            .unwrap()
            .document
            .charged_usd,
        Some(0.6)
    );
    log.append(json!({"event":"run_started"}));
    log.append(
        json!({"event":"budget_settled", "charged_usd":0.2, "node_id":"draw#1", "reported":true}),
    );
    assert_eq!(
        read::read_run(&fixture.root())
            .unwrap()
            .document
            .charged_usd,
        None
    );
}

#[test]
fn run_projection_uses_first_recorded_name_across_resume() {
    let fixture = Fixture::new("image/png");
    let path = fixture.root().join("events.jsonl");
    let events = std::fs::read_to_string(&path).unwrap();
    let (first, rest) = events.split_once('\n').unwrap();
    let mut first: Value = serde_json::from_str(first).unwrap();
    first["name"] = json!("character_1_rig_ready");
    std::fs::write(&path, format!("{first}\n{rest}")).unwrap();
    FileForEvents::new(&fixture.root())
        .append(json!({"event":"run_started", "name":"different_name"}));
    assert_eq!(
        read::read_inventory(&fixture.root())
            .unwrap()
            .document
            .run_name,
        "character_1_rig_ready"
    );
}

#[test]
fn run_projection_keeps_legacy_basename_for_missing_or_invalid_initial_names() {
    for name in [
        Value::Null,
        json!("two words"),
        json!("_leading"),
        json!("café"),
        json!("a".repeat(65)),
        json!("/private/invalid"),
    ] {
        let fixture = Fixture::new("image/png");
        let root = fixture.root();
        let path = root.join("events.jsonl");
        let events = std::fs::read_to_string(&path).unwrap();
        let (first, rest) = events.split_once('\n').unwrap();
        let mut first: Value = serde_json::from_str(first).unwrap();
        if !name.is_null() {
            first["name"] = name;
        }
        std::fs::write(&path, format!("{first}\n{rest}")).unwrap();
        FileForEvents::new(&root).append(json!({"event":"run_started", "name":"later_valid_name"}));
        assert_eq!(
            read::read_inventory(&root).unwrap().document.run_name,
            root.file_name().unwrap().to_string_lossy()
        );
    }
}

async fn layout_of(app: Router) -> Value {
    let response = app
        .oneshot(Fixture::request("/api/layout").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers().get(header::ETAG).is_none());
    let report: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-layout-report-v1.schema.json"
    ))
    .unwrap();
    assert!(
        jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&report)
    );
    report
}

#[tokio::test]
async fn the_layout_route_serves_automatic_cells_for_a_plan_and_a_run() {
    let app = router(AppState {
        source: plan_source(static_plan()).unwrap(),
        authority: "127.0.0.1:43123".into(),
        artifact_prefix: String::new(),
    });
    let report = layout_of(app).await;
    assert_eq!(report["state"], "none");
    assert_eq!(report["cursor"], Value::Null);
    assert_eq!(report["cells"]["step_0"]["column"], 0);
    assert_eq!(report["cells"]["future_items"]["source"], "automatic");

    let fixture = Fixture::new("image/png");
    let report = layout_of(fixture.app()).await;
    assert!(report["cursor"].is_string());
    assert!(!report["cells"].as_object().unwrap().is_empty());
    // A run's snapshot carries the report of the same prefix.
    let response = fixture
        .app()
        .oneshot(
            Fixture::request("/api/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let captured: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(captured["layout"], report);
    assert_eq!(captured["layout"]["cursor"], captured["cursor"]);
}
