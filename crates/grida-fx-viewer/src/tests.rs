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
            project_root: self.root(),
        })
    }

    fn request(path: &str) -> axum::http::request::Builder {
        Request::builder()
            .uri(path)
            .header(header::HOST, "127.0.0.1:43123")
    }
}

#[tokio::test]
async fn answers_are_exact_run_same_origin_and_share_the_runtime_winner() {
    use grida_fx_runtime::waiting::{self, RequestSpec, ResponseChannel};

    let fixture = Fixture::new("image/png");
    let root = fixture.root();
    let path = root.join("events.jsonl");
    let history = std::fs::read_to_string(&path).unwrap();
    std::fs::write(path, history.replace("offline-fixture", "0123456789abcdef")).unwrap();
    let opened = waiting::ensure_request(
        &root,
        RequestSpec {
            step: "choose#1".into(),
            message: None,
            schema: json!({"type":"boolean"}),
            show: Value::Null,
            choices: None,
            choice_field: None,
            timeout_seconds: None,
            on_timeout: Default::default(),
        },
    )
    .unwrap();
    let endpoint = format!("/api/requests/{}/respond", opened.id);
    let app = fixture.app();
    for (origin, marker, mime, expected) in [
        (None, true, "application/json", StatusCode::FORBIDDEN),
        (
            Some("http://unrelated.invalid"),
            true,
            "application/json",
            StatusCode::FORBIDDEN,
        ),
        (
            Some("http://127.0.0.1:43123"),
            false,
            "application/json",
            StatusCode::FORBIDDEN,
        ),
        (
            Some("http://127.0.0.1:43123"),
            true,
            "text/plain",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
    ] {
        let mut req = Fixture::request(&endpoint)
            .method(Method::POST)
            .header(header::CONTENT_TYPE, mime);
        if let Some(origin) = origin {
            req = req.header(header::ORIGIN, origin);
        }
        if marker {
            req = req.header("x-fx-action", "respond");
        }
        let response = app
            .clone()
            .oneshot(req.body(Body::from(r#"{"answer":true}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let submit = |json: &'static str| {
        Fixture::request(&endpoint)
            .method(Method::POST)
            .header(header::ORIGIN, "http://127.0.0.1:43123")
            .header("sec-fetch-site", "same-origin")
            .header("x-fx-action", "respond")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(submit(r#"{"answer":9007199254740993}"#))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = app
        .clone()
        .oneshot(submit(r#"{"answer":"invalid"}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let winner = waiting::respond(&root, &opened.id, json!(false), ResponseChannel::Cli).unwrap();
    let response = app
        .clone()
        .oneshot(submit(r#"{"answer":true}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(body["request"], json!(winner));
    let response = app.oneshot(submit(r#"{"answer":false}"#)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(body, json!(winner));
}

#[test]
fn an_open_request_can_preview_its_files_before_the_runner_logs_the_wait() {
    use grida_fx_runtime::waiting::{self, RequestSpec};
    use std::fs;

    let fixture = Fixture::new("image/png");
    let root = fixture.root();
    fs::write(root.join("events.jsonl"), "").unwrap();
    let mut log = FileForEvents::new(&root);
    let invocation = "0123456789abcdef";
    log.append(json!({"event":"run_started","invocation_id":invocation}));
    let before_step = observation::snapshot(&root).unwrap();
    log.append(json!({"event":"node_started","invocation_id":invocation,"id":"draw#1","path":"draw","uses":"fx/await@1"}));

    let bytes = b"synthetic review image";
    let digest = file_digest(bytes);
    fs::create_dir(root.join("values")).unwrap();
    fs::write(root.join("values").join(&digest), bytes).unwrap();
    let image =
        json!({"file":{"digest":digest,"kind":"image/png","name":"review.png","size":bytes.len()}});
    waiting::ensure_request(
        &root,
        RequestSpec {
            step: "draw#1".into(),
            message: None,
            schema: json!({"type":"string"}),
            show: Value::Null,
            choices: Some([("swatch".into(), image)].into()),
            choice_field: None,
            timeout_seconds: None,
            on_timeout: Default::default(),
        },
    )
    .unwrap();

    let current = observation::snapshot(&root).unwrap();
    assert!(
        read::read_observed(&root, &current)
            .unwrap()
            .document
            .artifacts
            .iter()
            .any(|a| a.digest == digest && a.available)
    );
    assert!(
        read::read_inventory(&root)
            .unwrap()
            .open_artifact(&root, &digest)
            .is_ok()
    );
    assert!(
        read::read_observed(&root, &before_step)
            .unwrap()
            .open_artifact(&root, &digest)
            .is_err()
    );
    assert!(
        read::project_export_records(&root, &current)
            .unwrap()
            .open_artifact(&root, &digest)
            .is_err()
    );
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
        for (name, default) in [
            ("invocation_id", json!("offline-fixture")),
            ("offset_ms", json!(0)),
        ] {
            event
                .as_object_mut()
                .unwrap()
                .entry(name)
                .or_insert(default);
        }
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
fn review_outcome_uses_the_terminal_wait_fact_and_survives_export_without_answers() {
    let fixture = Fixture::new("image/png");
    let root = fixture.root();
    let mut log = FileForEvents::new(&root);
    let outcome = |id: &str| {
        read::read_inventory(&root)
            .unwrap()
            .document
            .nodes
            .into_iter()
            .find(|node| node.id == id)
            .unwrap()
            .review_outcome
    };
    log.append(json!({"event":"run_started"}));
    log.append(json!({"event":"node_started","id":"draw#1","path":"draw","uses":"fx/await@01"}));
    log.append(json!({"event":"waiting_request","id":"draw#1","request_status":"answered"}));
    assert_eq!(outcome("draw#1"), None);
    log.append(json!({"event":"node_finished","id":"draw#1","facts":{"outcome":"answered"},"outputs":{"answer":{"value":false}}}));
    assert_eq!(outcome("draw#1"), Some("answered"));
    log.append(json!({"event":"node_finished","id":"later#1","facts":{"outcome":"answered"}}));
    assert_eq!(outcome("later#1"), None); // A different builtin or project node can publish an identically named fact.
    log.append(json!({"event":"run_finished","ok":true,"outputs":{}}));
    std::fs::File::create(root.join("run.lock")).unwrap();
    let package = tempfile::tempdir().unwrap();
    let target = package.path().join("export");
    export::run_export(&root, &target, "export", export::ExportOptions::default()).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(target.join("manifest.json")).unwrap()).unwrap();
    let draw = manifest["view"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "draw#1")
        .unwrap();
    assert_eq!(draw["review_outcome"], "answered");
    assert_eq!(draw["outputs"]["answer"], json!({"withheld":true}));

    log.append(json!({"event":"run_started"}));
    assert_eq!(outcome("draw#1"), Some("answered")); // A resume can preserve a completed checkpoint.
    log.append(json!({"event":"node_started","id":"draw#1","path":"draw","uses":"fx/await@1"}));
    assert_eq!(outcome("draw#1"), None);
    log.append(json!({"event":"node_finished","id":"draw#1","facts":{"outcome":"expired"}}));
    assert_eq!(outcome("draw#1"), Some("expired"));
    log.append(json!({"event":"node_retry","id":"draw#1"}));
    assert_eq!(outcome("draw#1"), None);
    log.append(
        json!({"event":"node_failed","id":"draw#1","error":"the response deadline expired"}),
    );
    assert_eq!(outcome("draw#1"), None);
    log.append(json!({"event":"node_failed","id":"draw#1","facts":{"outcome":"expired"}}));
    assert_eq!(outcome("draw#1"), Some("expired"));
}

#[test]
fn video_preview_metadata_survives_plan_and_dynamic_event_projection() {
    let fixture = Fixture::new("image/png");
    let plan_path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
    let selected = json!({"video":"video", "poster":"poster"});
    plan["steps"]["draw"]["preview"] = selected.clone();
    std::fs::write(&plan_path, plan.to_string()).unwrap();
    let projected = read::read_inventory(&fixture.root()).unwrap().document;
    assert_eq!(projected.nodes[0].preview, Some(selected.clone()));
    assert!(projected.nodes[1].preview.is_none());

    // The runtime descriptor belongs to this repeated instance, including nodes absent from
    // the original static plan. It does not reuse another take's output or a file location.
    let mut events = FileForEvents::new(&fixture.root());
    events.append(
        json!({"event":"node_started", "id":"clip[item]#2", "path":"clip[item]",
        "step":"clip", "take":[2], "uses":"acme/clip@1", "reads":[], "with":{},
        "preview":selected}),
    );
    let projected = read::read_inventory(&fixture.root()).unwrap().document;
    let dynamic = projected
        .nodes
        .iter()
        .find(|node| node.id == "clip[item]#2")
        .unwrap();
    assert_eq!(
        dynamic.preview,
        Some(json!({"video":"video", "poster":"poster"}))
    );
    let encoded = serde_json::to_value(projected).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-viewer-run-v1.schema.json"
    ))
    .unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&encoded)
        .unwrap();
}

#[test]
fn malformed_preview_metadata_never_becomes_a_location_or_expression() {
    for descriptor in [
        json!(null),
        json!({}),
        json!({"video":"steps.clip.video"}),
        json!({"video":"video", "poster":"${{ steps.cover.image }}"}),
        json!({"video":"video", "poster":"/tmp/cover.png"}),
        json!({"video":"video", "poster":null}),
        json!({"video":"video", "url":"https://example.com/cover.png"}),
    ] {
        assert!(read::preview(Some(&descriptor)).is_none(), "{descriptor}");
    }
    assert_eq!(
        read::preview(Some(&json!({"video":"video"}))),
        Some(json!({"video":"video"}))
    );
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

#[test]
fn a_start_is_dated_from_its_invocation_start() {
    let fixture = Fixture::new("image/png");
    let mut events = FileForEvents::new(&fixture.root());
    // The fixture's own invocation records no start, so its steps stay undated.
    for event in [
        json!({"event":"run_started","invocation_id":"resumed","started_at":"2026-10-11T09:00:00.000Z","offset_ms":0}),
        json!({"event":"node_started","id":"draw#1","path":"draw","invocation_id":"resumed","offset_ms":61500}),
    ] {
        events.append(event);
    }
    drop(events);
    let document =
        serde_json::to_value(read::read_inventory(&fixture.root()).unwrap().document).unwrap();
    let node = |id: &str| {
        document["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(node("draw#1")["state"], "running");
    assert_eq!(node("draw#1")["started_at"], "2026-10-11T09:01:01.500Z");
    assert!(node("later#1").get("started_at").is_none());
}

#[test]
fn a_run_is_timed_by_its_invocations_and_dated_by_its_last_record() {
    let fixture = Fixture::new("image/png");
    let read =
        || serde_json::to_value(read::read_inventory(&fixture.root()).unwrap().document).unwrap();
    // The fixture's own invocation records no start: it is timed, but its last record is undated.
    let before = read();
    let earlier = before["elapsed_ms"].as_u64().unwrap();
    assert!(before.get("recorded_at").is_none());
    let mut events = FileForEvents::new(&fixture.root());
    for event in [
        json!({"event":"run_started","invocation_id":"resumed","started_at":"2026-10-11T09:00:00.000Z","offset_ms":0}),
        json!({"event":"node_started","id":"draw#1","path":"draw","invocation_id":"resumed","offset_ms":61500}),
    ] {
        events.append(event);
    }
    drop(events);
    let after = read();
    assert_eq!(after["elapsed_ms"], earlier + 61500);
    assert_eq!(after["recorded_at"], "2026-10-11T09:01:01.500Z");
}

#[test]
fn a_step_waiting_for_its_route_is_waiting_and_its_waits_are_timed() {
    let fixture = Fixture::new("image/png");
    let mut log = FileForEvents::new(&fixture.root());
    let at = |invocation: &str, event: Value, offset: u64| {
        let mut event = event;
        event["invocation_id"] = json!(invocation);
        event["offset_ms"] = json!(offset);
        event
    };
    let first = |event: Value, offset: u64| at("first", event, offset);
    let second = |event: Value, offset: u64| at("second", event, offset);
    let call = |name: &str, key: char, route: &str| {
        json!({"event":name,"id":"draw#1","capability":"image.generate","route":route,
               "call":key.to_string().repeat(64)})
    };
    let read = || {
        let document =
            serde_json::to_value(read::read_inventory(&fixture.root()).unwrap().document).unwrap();
        document["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "draw#1")
            .unwrap()
            .clone()
    };
    let started = json!({"event":"node_started","id":"draw#1","path":"draw","with":{},"reads":[]});
    for event in [
        first(
            json!({"event":"run_started","started_at":"2026-10-11T09:00:00.000Z"}),
            0,
        ),
        first(started.clone(), 1_000),
        first(call("call_waiting", 'a', "img-a@acme"), 1_500),
        // A second call of the same body waits too; the earliest open wait is shown.
        first(call("call_waiting", 'b', "img-b@acme"), 3_000),
    ] {
        log.append(event);
    }
    let waiting = read();
    assert_eq!(waiting["state"], "waiting");
    assert_eq!(waiting["waiting_for"], "img-a@acme");
    assert_eq!(waiting["waiting_since"], "2026-10-11T09:00:01.500Z");
    assert!(waiting.get("waited_ms").is_none());
    // One admitted, the step still waits, for the other call's route since that call began.
    log.append(first(call("call_admitted", 'a', "img-a@acme"), 41_500));
    let still = read();
    assert_eq!(still["state"], "waiting");
    assert_eq!(still["waiting_for"], "img-b@acme");
    assert_eq!(still["waiting_since"], "2026-10-11T09:00:03.000Z");
    assert_eq!(still["waited_ms"], 40_000);
    // Both admitted, it runs; the overlapping waits count once (1.5 s to 43 s). An admission
    // with no open wait changes nothing; a later wait of the same start adds to the time.
    for event in [
        first(call("call_admitted", 'b', "img-b@acme"), 43_000),
        first(call("call_admitted", 'z', "img-a@acme"), 44_000),
        first(call("call_waiting", 'c', "img-a@acme"), 50_000),
        first(call("call_admitted", 'c', "img-a@acme"), 52_000),
    ] {
        log.append(event);
    }
    let running = read();
    assert_eq!(running["state"], "running");
    assert_eq!(running["waited_ms"], 43_500);
    assert!(running.get("waiting_for").is_none() && running.get("waiting_since").is_none());
    // A new attempt leaves its wait behind, untimed, even once that call is admitted.
    log.append(first(call("call_waiting", 'd', "img-a@acme"), 55_000));
    assert_eq!(read()["state"], "waiting");
    log.append(first(
        json!({"event":"node_retry","id":"draw#1","attempt":2,"error":"e"}),
        56_000,
    ));
    let retried = read();
    assert_eq!(retried["state"], "running");
    assert!(retried.get("waiting_for").is_none());
    log.append(first(call("call_admitted", 'd', "img-a@acme"), 57_000));
    assert_eq!(read()["waited_ms"], 43_500);
    // Waiting again, then the run ends: the step ended unfinished and waits for nothing.
    log.append(first(call("call_waiting", 'e', "img-a@acme"), 60_000));
    assert_eq!(read()["state"], "waiting");
    log.append(first(
        json!({"event":"run_cancelled","reason":"interrupted","charged_usd":0}),
        61_000,
    ));
    let ended = read();
    assert_eq!(ended["state"], "failed");
    assert_eq!(ended["interrupted"], true);
    assert_eq!(ended["waited_ms"], 43_500);
    assert!(ended.get("waiting_for").is_none() && ended.get("waiting_since").is_none());
    // A new invocation, its offsets from 0 again, leaves the last one's wait behind.
    log.append(second(
        json!({"event":"run_started","started_at":"2026-10-11T09:01:09.000Z"}),
        0,
    ));
    log.append(second(call("call_admitted", 'e', "img-a@acme"), 500));
    let resumed = read();
    assert_eq!(resumed["state"], "running");
    assert_eq!(resumed["waited_ms"], 43_500);
    assert!(resumed.get("waiting_for").is_none());
    // A new start forgets the last one's waits and times its own.
    log.append(second(started, 1_000));
    assert!(read().get("waited_ms").is_none());
    log.append(second(call("call_waiting", 'f', "img-a@acme"), 2_000));
    assert_eq!(read()["waiting_since"], "2026-10-11T09:01:11.000Z");
    log.append(second(call("call_admitted", 'f', "img-a@acme"), 5_000));
    let restarted = read();
    assert_eq!(restarted["state"], "running");
    assert_eq!(restarted["waited_ms"], 3_000);
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
        project_root: PathBuf::new(),
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

#[tokio::test]
async fn current_viewer_configuration_reloads_without_changing_recorded_data() {
    let fixture = Fixture::new("image/png");
    let original_plan = std::fs::read(fixture.root().join("plan.json")).unwrap();
    let original_events = std::fs::read(fixture.root().join("events.jsonl")).unwrap();
    let app = fixture.app();
    let read = || async {
        app.clone()
            .oneshot(
                Fixture::request("/api/viewer_config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    let response = read().await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let defaults: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        defaults,
        json!({"display":{"workflow_inputs":true,"workflow_outputs":true,"notes":true}})
    );
    std::fs::write(
        fixture.root().join("fx.yaml"),
        "fx: project/v1\nviewer: {display: {workflow_inputs: false, notes: false}}\n",
    )
    .unwrap();
    let config: Value = serde_json::from_slice(
        &to_bytes(read().await.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        config,
        json!({"display":{"workflow_inputs":false,"workflow_outputs":true,"notes":false}})
    );
    std::fs::write(
        fixture.root().join("fx.yaml"),
        "fx: project/v1\nviewer: {display: {notes: private_invalid_value}}\n",
    )
    .unwrap();
    let refused = read().await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = to_bytes(refused.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.contains("private_invalid_value"));
    assert!(!text.contains(&fixture.root().to_string_lossy().to_string()));
    std::fs::remove_file(fixture.root().join("fx.yaml")).unwrap();
    let reset: Value = serde_json::from_slice(
        &to_bytes(read().await.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reset, defaults);
    assert_eq!(
        std::fs::read(fixture.root().join("plan.json")).unwrap(),
        original_plan
    );
    assert_eq!(
        std::fs::read(fixture.root().join("events.jsonl")).unwrap(),
        original_events
    );
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn confined_open_traverses_search_only_directories() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let root = base.join("ancestor/root");
    let parent = root.join("nested");
    std::fs::create_dir_all(&parent).unwrap();
    std::fs::write(parent.join("source.txt"), b"searchable source").unwrap();
    let ancestors = [base.join("ancestor"), root.clone(), parent];
    for ancestor in &ancestors {
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(0o111)).unwrap();
    }
    let opened = read::open_confined_file(&root, "nested/source.txt");
    for ancestor in &ancestors {
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (mut file, _) = opened.unwrap();
    let mut bytes = String::new();
    std::io::Read::read_to_string(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, "searchable source");
}

#[cfg(unix)]
#[test]
fn confined_open_rejects_path_replacements_after_validation() {
    use std::os::unix::fs::symlink;

    for replacement in ["file_link", "parent_link", "root_parent_link", "fifo"] {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let root = base.join("container/root");
        let relative = "nested/source.txt";
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join(relative), b"confined source").unwrap();
        let foreign = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(foreign.path().join("root/nested")).unwrap();
        std::fs::write(foreign.path().join("source.txt"), b"outside source").unwrap();
        std::fs::write(
            foreign.path().join("root/nested/source.txt"),
            b"outside source",
        )
        .unwrap();
        let path = read::confined_file(&root, relative).unwrap();
        match replacement {
            "file_link" => {
                std::fs::remove_file(&path).unwrap();
                symlink(foreign.path().join("source.txt"), &path).unwrap();
            }
            "parent_link" => {
                std::fs::rename(root.join("nested"), root.join("previous")).unwrap();
                symlink(foreign.path(), root.join("nested")).unwrap();
            }
            "root_parent_link" => {
                std::fs::rename(base.join("container"), base.join("previous")).unwrap();
                symlink(foreign.path(), base.join("container")).unwrap();
            }
            "fifo" => {
                std::fs::remove_file(&path).unwrap();
                assert!(
                    std::process::Command::new("mkfifo")
                        .arg(&path)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            _ => unreachable!(),
        }
        // A FIFO without a writer must be refused without waiting for one.
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            sent.send(read::open_confined_file(&root, relative).is_err())
                .unwrap();
        });
        assert!(
            received
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("confined opening must not block"),
            "accepted {replacement}"
        );
        reader.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn artifact_open_rejects_replaced_parent_directory() {
    let fixture = Fixture::new("image/png");
    let snapshot = read::read_run(&fixture.root()).unwrap();
    let foreign = tempfile::tempdir().unwrap();
    std::fs::write(foreign.path().join("image.png"), &fixture.bytes).unwrap();
    std::fs::rename(
        fixture.root().join("files/draw"),
        fixture.root().join("files/previous"),
    )
    .unwrap();
    std::os::unix::fs::symlink(foreign.path(), fixture.root().join("files/draw")).unwrap();
    assert!(
        snapshot
            .open_artifact(&fixture.root(), &fixture.digest)
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
    assert!(read::open_confined_file(&fixture.root(), "../plan.json").is_err());
    assert!(read::open_confined_file(&fixture.root(), "/etc/passwd").is_err());
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
async fn serves_recorded_matroska_inline_with_head_and_ranges() {
    let fixture = Fixture::new("video/x-matroska");
    let url = format!("/api/artifacts/{}", fixture.digest);
    for method in [Method::GET, Method::HEAD] {
        let response = fixture
            .app()
            .oneshot(
                Fixture::request(&url)
                    .method(method.clone())
                    .header(header::RANGE, "bytes=2-6")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/x-matroska");
        assert_eq!(response.headers()[header::CONTENT_DISPOSITION], "inline");
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "5");
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            format!("bytes 2-6/{}", fixture.bytes.len())
        );
        let body = to_bytes(response.into_body(), 100).await.unwrap();
        if method == Method::HEAD {
            assert!(body.is_empty());
        } else {
            assert_eq!(body, &fixture.bytes[2..7]);
        }
    }
}

#[test]
fn inline_video_types_follow_the_recorded_kind_grammar() {
    for kind in [
        "video/mp4",
        "video/webm",
        "video/x-matroska",
        "video/vnd.acme+clip",
    ] {
        assert_eq!(artifact_mime(kind), (kind, true));
    }
    for kind in [
        "video",
        "video/",
        "video/*",
        "video/MP4",
        "video/mp4; codecs=avc1",
        "video/mp4\r\nx-injected: yes",
        "video/mp4/extra",
        "video/mp4[]",
        "video/mp4?",
        "video/ mp4",
        "video/\u{00e9}",
        "text/html",
        "image/svg+xml",
    ] {
        assert_eq!(artifact_mime(kind), ("application/octet-stream", false));
    }
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
async fn viewer_assets_allow_local_wasm_workers_and_verified_model_blobs_only() {
    let fixture = Fixture::new("model/gltf-binary");
    let response = fixture
        .app()
        .oneshot(Fixture::request("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(policy.contains("script-src 'self' 'wasm-unsafe-eval';"));
    assert!(policy.contains("worker-src 'self' blob:;"));
    assert!(policy.contains("connect-src 'self' data: blob:;"));
    assert!(!policy.contains("'unsafe-eval'"));
    assert!(!policy.contains("https:"));
    for (path, content_type) in [
        (
            "/assets/model-decoders/draco_decoder.wasm",
            "application/wasm",
        ),
        (
            "/assets/model-decoders/basis_transcoder.wasm",
            "application/wasm",
        ),
        (
            "/assets/model-decoders/draco_wasm_wrapper.js",
            "text/javascript",
        ),
        (
            "/assets/model-decoders/basis_transcoder.js",
            "text/javascript",
        ),
        ("/assets/licenses.md", "text/markdown"),
    ] {
        let response = fixture
            .app()
            .oneshot(Fixture::request(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            content_type,
            "{path}"
        );
    }
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

#[cfg(unix)]
#[tokio::test]
async fn observation_reports_active_owner_and_clears_it_without_new_events() {
    use grida_fx_runtime::{engine::Cancel, folder::RunFolder, run_control::RunControl};

    let fixture = Fixture::new("image/png");
    let root = fixture.root();
    let plan = "a".repeat(64);
    let invocation = "0123456789abcdef";
    let get = |path: &str| {
        fixture
            .app()
            .oneshot(Fixture::request(path).body(Body::empty()).unwrap())
    };
    let mut log = FileForEvents::new(&root);
    let lock = RunFolder::lock_existing(&root, "test run").unwrap();
    log.append(json!({"event":"run_started","invocation_id":invocation}));
    assert!(
        !get("/api/snapshot")
            .await
            .unwrap()
            .headers()
            .contains_key(RUN_ACTIVE_HEADER)
    );

    let owner = control::ControlServer::start(
        &root,
        &plan,
        invocation,
        RunControl::unrecorded(Cancel::new()),
        &tokio::runtime::Handle::current(),
    )
    .unwrap();
    log.append(json!({"event":"pause_requested","source":"cli","invocation_id":invocation}));
    let snapshot = get("/api/snapshot").await.unwrap();
    assert_eq!(snapshot.headers()[RUN_ACTIVE_HEADER], "1");
    let snapshot: Value =
        serde_json::from_slice(&to_bytes(snapshot.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(snapshot["view"]["state"], "pausing");
    let after = snapshot["cursor"].as_str().unwrap();
    let batch = format!("/api/events?after={after}&plan={plan}&invocation_id={invocation}");
    assert_eq!(get(&batch).await.unwrap().headers()[RUN_ACTIVE_HEADER], "1");
    let mismatched = batch.replace(invocation, "fedcba9876543210");
    assert!(
        !get(&mismatched)
            .await
            .unwrap()
            .headers()
            .contains_key(RUN_ACTIVE_HEADER)
    );

    drop(owner);
    drop(lock);
    let response = get(&batch).await.unwrap();
    assert!(!response.headers().contains_key(RUN_ACTIVE_HEADER));
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(response["events"], json!([]));
    assert_eq!(response["cursor"], after);
    let snapshot = get("/api/snapshot").await.unwrap();
    assert!(!snapshot.headers().contains_key(RUN_ACTIVE_HEADER));
    let snapshot: Value =
        serde_json::from_slice(&to_bytes(snapshot.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(snapshot["view"]["state"], "unfinished");
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
fn pause_retains_known_outputs_and_waiting_requests_for_resume() {
    let fixture = Fixture::new("image/png");
    let mut log = FileForEvents::new(&fixture.root());
    log.append(json!({"event":"run_started", "invocation_id":"pause-owner"}));
    log.append(
        json!({"event":"node_started", "id":"draw#1", "path":"draw", "with":{}, "reads":[]}),
    );
    log.append(json!({"event":"waiting_request", "id":"draw#1", "request_id":"a".repeat(64), "request_status":"pending"}));
    log.append(json!({
        "event": "run_paused", "invocation_id": "pause-owner", "charged_usd": 0.6,
        "outputs": {"early": {"value": "ready"}},
    }));
    let view = read::read_run(&fixture.root()).unwrap().document;
    assert_eq!(view.state, "paused");
    assert_eq!(view.charged_usd, Some(0.6));
    assert_eq!(view.outputs["early"], json!({"value": "ready"}));
    assert_eq!(view.nodes[0].state, "awaiting");
    log.append(json!({"event":"run_started", "invocation_id":"resumed"}));
    let view = read::read_run(&fixture.root()).unwrap().document;
    assert_eq!(view.state, "unfinished");
    assert_eq!(view.nodes[0].state, "awaiting");
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
        project_root: PathBuf::new(),
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

#[test]
fn static_unknown_forecast_uses_version_two_and_preserves_partial_totals() {
    let mut graph = static_plan();
    graph["kind"] = json!("fx-graph-v2");
    let price = json!({"low_usd":null,"high_usd":null,"known_low_usd":0.12,"known_high_usd":0.12,"unpriced_calls":1});
    graph["instances"][0]["price"] = price.clone();
    graph["estimate"] = price;
    graph["estimate"]["ceiling_usd"] = json!(3);
    assert!(plan_source(graph.clone()).is_ok());
    graph["kind"] = json!("fx-graph-v1");
    assert!(
        plan_source(graph).is_err(),
        "old format cannot silently misread a subtotal as total"
    );
}
