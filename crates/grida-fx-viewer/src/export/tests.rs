use super::*;
use crate::tests::Fixture;

fn source(kind: &str) -> Fixture {
    let fixture = Fixture::new(kind);
    File::create(fixture.root().join("run.lock")).unwrap();
    fixture
}
fn export(
    fixture: &Fixture,
    parent: &tempfile::TempDir,
    options: ExportOptions,
) -> Result<ExportResult, ExportError> {
    run_export(
        &fixture.root(),
        &parent.path().join("export"),
        "export",
        options,
    )
}
fn manifest(parent: &tempfile::TempDir) -> Value {
    serde_json::from_slice(&std::fs::read(parent.path().join("export/manifest.json")).unwrap())
        .unwrap()
}

#[test]
fn publishing_display_defaults_preserve_execution_and_disclosure_evidence() {
    use grida_fx_core::docs::project::ViewerDisplay;

    let fixture = source("image/png");
    let visible = tempfile::tempdir().unwrap();
    let hidden = tempfile::tempdir().unwrap();
    let before_plan = std::fs::read(fixture.root().join("plan.json")).unwrap();
    let before_events = std::fs::read(fixture.root().join("events.jsonl")).unwrap();
    export(&fixture, &visible, ExportOptions::default()).unwrap();
    let config = ViewerConfig {
        display: ViewerDisplay {
            workflow_inputs: false,
            workflow_outputs: true,
            notes: false,
        },
    };
    run_export_with_viewer(
        &fixture.root(),
        &hidden.path().join("export"),
        "export",
        ExportOptions::default(),
        config,
    )
    .unwrap();
    let original = manifest(&visible);
    let configured = manifest(&hidden);
    assert!(original.get("viewer").is_none());
    assert_eq!(configured["viewer"], serde_json::to_value(config).unwrap());
    for field in ["source", "view", "layout", "disclosure", "artifacts"] {
        assert_eq!(original[field], configured[field], "{field}");
    }
    assert_eq!(
        std::fs::read(fixture.root().join("plan.json")).unwrap(),
        before_plan
    );
    assert_eq!(
        std::fs::read(fixture.root().join("events.jsonl")).unwrap(),
        before_events
    );
}

#[test]
fn default_disclosure_preserves_media_and_withholds_author_values() {
    let fixture = source("image/png");
    let parent = tempfile::tempdir().unwrap();
    let plan_before = std::fs::read(fixture.root().join("plan.json")).unwrap();
    let events_before = std::fs::read(fixture.root().join("events.jsonl")).unwrap();
    let result = export(&fixture, &parent, ExportOptions::default()).unwrap();
    let data = manifest(&parent);
    assert_eq!(result.files, 1);
    assert_eq!(result.omitted_artifacts.len(), 1);
    assert_eq!(data["source"]["plan_sha256"], sha256_hex(&plan_before));
    assert_eq!(
        data["source"]["events_sha256"],
        sha256_hex(
            canon(&json!(
                observation::snapshot(&fixture.root()).unwrap().events
            ))
            .as_bytes()
        )
    );
    assert_eq!(data["view"]["inputs"]["caption"], json!({"withheld":true}));
    assert_eq!(data["view"]["nodes"][1]["error"], Value::Null);
    assert_eq!(
        data["view"]["outputs"]["result"]["file"]["digest"],
        fixture.digest
    );
    assert!(
        data["view"]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|artifact| artifact["url"].is_null())
    );
    assert_eq!(data["layout"]["cursor"], Value::Null);
    let encoded = std::fs::read(parent.path().join("export/manifest.json")).unwrap();
    assert_eq!(result.manifest_sha256, sha256_hex(&encoded));
    assert!(!String::from_utf8_lossy(&encoded).contains(fixture.root().to_str().unwrap()));
    assert_eq!(
        std::fs::read(fixture.root().join("plan.json")).unwrap(),
        plan_before
    );
    assert_eq!(
        std::fs::read(fixture.root().join("events.jsonl")).unwrap(),
        events_before
    );
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-run-export-v1.schema.json"
    ))
    .unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&data)
        .unwrap();
}

#[test]
fn explicit_disclosure_and_positive_schema_projection() {
    let fixture = source("image/png");
    let parent = tempfile::tempdir().unwrap();
    export(
        &fixture,
        &parent,
        ExportOptions {
            include_values: true,
            include_diagnostics: true,
        },
    )
    .unwrap();
    let data = manifest(&parent);
    assert_eq!(
        data["view"]["inputs"]["caption"],
        json!({"value":"A synthetic sample"})
    );
    assert_eq!(data["view"]["nodes"][1]["error"], "Synthetic failure");
    let params = json!({"type":"object","default":"private","description":"private","enum":["private"],"pattern":"private","properties":{"nested":{"type":"string","examples":["private"]}},"additionalProperties":false,"required":["nested"],"x-private":"private"});
    assert_eq!(
        schema(&params, false),
        json!({"type":"object","properties":{"nested":{"type":"string"}},"additionalProperties":false,"required":["nested"]})
    );
    let mut view = data["view"].clone();
    view["workflow"]["file"] = json!("/private/source.yaml");
    view["private_extension"] = json!("private");
    let safe = project_view(&view, ExportOptions::default());
    assert!(safe["workflow"].get("file").is_none());
    assert!(safe.get("private_extension").is_none());
}

#[test]
fn video_poster_selection_survives_export_when_original_video_is_policy_excluded() {
    let fixture = source("video/x-matroska");
    let poster = b"synthetic poster artifact";
    let digest = sha256_hex(poster);
    let path = grida_fx_runtime::folder::step_file_path("draw", "poster", None, "image/png");
    std::fs::write(fixture.root().join(path), poster).unwrap();
    let event_path = fixture.root().join("events.jsonl");
    let mut events: Vec<Value> = std::fs::read_to_string(&event_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for event in &mut events {
        if event["event"] == "node_started" && event["id"] == "draw#1" {
            event["preview"] = json!({"video":"image", "poster":"poster"});
            event["ports"] = json!({"inputs":{}, "params":{}, "outputs":{"image":"video/x-matroska", "poster":"image/png"}});
        }
        if event["event"] == "node_finished" && event["id"] == "draw#1" {
            event["outputs"]["poster"] = json!({"file":{"digest":digest, "kind":"image/png", "name":"poster.png", "size":poster.len()}});
        }
    }
    std::fs::write(
        &event_path,
        events
            .iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let parent = tempfile::tempdir().unwrap();
    export(&fixture, &parent, ExportOptions::default()).unwrap();
    let data = manifest(&parent);
    let node = data["view"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "draw#1")
        .unwrap();
    assert_eq!(node["preview"], json!({"video":"image", "poster":"poster"}));
    let inventory = data["artifacts"].as_array().unwrap();
    let master = inventory
        .iter()
        .find(|file| file["digest"] == fixture.digest)
        .unwrap();
    assert_eq!(master["omitted_reason"], "policy_excluded");
    let included = inventory
        .iter()
        .find(|file| file["digest"] == digest)
        .unwrap();
    assert!(included["omitted_reason"].is_null());
    assert_eq!(
        std::fs::read(
            parent
                .path()
                .join("export")
                .join(included["path"].as_str().unwrap())
        )
        .unwrap(),
        poster
    );
    references::validate(&data).unwrap();
    let mut invalid = data.clone();
    invalid["view"]["nodes"][0]["preview"]["poster"] = json!("missing");
    assert!(references::validate(&invalid).is_err());
}

#[test]
fn raw_plan_marker_objects_stay_opaque_and_never_become_artifacts() {
    let fixture = source("image/png");
    let fake_digest = "c".repeat(64);
    let literals = json!({
        "list": {"list":[1,2], "title":"Keep the list title"},
        "sole_list": {"list":[1,2]},
        "collection": {"collection":[["item",3]], "title":"Keep the collection title"},
        "file": {"file":{"digest":fake_digest,"kind":"image/png","name":"literal.png","size":17},"note":"Ordinary authored JSON"},
        "sole_file": {"file":{"digest":fake_digest,"kind":"image/png","name":"literal.png","size":17}},
        "value": {"value":{"list":[1,2]},"note":"Keep the value note"},
        "sole_value": {"value":"Keep this literal wrapper"}
    });
    let plan_path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
    plan["inputs"] = literals.clone();
    let mut unused = plan["instances"][0].clone();
    unused["id"] = json!("unused#1");
    unused["path"] = json!("unused");
    unused["step"] = json!("unused");
    unused["with"] = literals.clone();
    plan["instances"].as_array_mut().unwrap().push(unused);
    std::fs::write(&plan_path, plan.to_string()).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-run-export-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for include_values in [false, true] {
        let parent = tempfile::tempdir().unwrap();
        export(
            &fixture,
            &parent,
            ExportOptions {
                include_values,
                include_diagnostics: false,
            },
        )
        .unwrap();
        let data = manifest(&parent);
        assert_eq!(data["artifacts"].as_array().unwrap().len(), 2);
        assert!(
            data["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["digest"] != fake_digest)
        );
        let nodes = data["view"]["nodes"].as_array().unwrap();
        let planned = nodes.iter().find(|node| node["id"] == "unused#1").unwrap();
        assert_eq!(planned["state"], "skipped");
        for (key, literal) in literals.as_object().unwrap() {
            let expected = if include_values {
                json!({"value":literal})
            } else {
                json!({"withheld":true})
            };
            assert_eq!(data["view"]["inputs"][key], expected);
            assert_eq!(planned["with"][key], expected);
        }
        // Event values already have their encoding, so real file references survive.
        let executed = nodes.iter().find(|node| node["id"] == "draw#1").unwrap();
        assert_eq!(executed["with"]["input"]["file"]["digest"], "b".repeat(64));
        validator.validate(&data).unwrap();
    }
}

#[test]
fn policy_exclusion_never_reads_attachment_bytes() {
    let fixture = source("text/plain");
    let parent = tempfile::tempdir().unwrap();
    std::fs::remove_file(fixture.root().join("files/draw/image.txt")).unwrap();
    read::take_verified_paths();
    let result = export(&fixture, &parent, ExportOptions::default()).unwrap();
    assert_eq!(result.files, 0);
    assert!(
        result
            .omitted_artifacts
            .iter()
            .any(|entry| entry.reason == "policy_excluded")
    );
    assert!(read::take_verified_paths().is_empty());
}

#[test]
fn selected_corrupt_or_missing_bytes_fail_without_partial_destination() {
    for missing in [true, false] {
        let fixture = source("image/png");
        let parent = tempfile::tempdir().unwrap();
        let path = fixture.root().join("files/draw/image.png");
        if missing {
            std::fs::remove_file(path).unwrap();
        } else {
            std::fs::write(path, b"wrong bytes").unwrap();
        }
        assert_eq!(
            export(&fixture, &parent, ExportOptions::default())
                .unwrap_err()
                .code,
            "artifact_unavailable"
        );
        assert!(!parent.path().join("export").exists());
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }
}

#[test]
fn admission_requires_existing_inactive_lock_and_terminal_latest_invocation() {
    let fixture = Fixture::new("image/png");
    let parent = tempfile::tempdir().unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "unavailable"
    );
    assert!(!fixture.root().join("run.lock").exists());
    let lock = File::create(fixture.root().join("run.lock")).unwrap();
    lock.try_lock().unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "busy"
    );
    drop(lock);
    let mut log = OpenOptions::new()
        .append(true)
        .open(fixture.root().join("events.jsonl"))
        .unwrap();
    writeln!(log,"{{\"kind\":\"fx-run-events-v1\",\"event\":\"run_started\",\"plan\":\"{}\",\"invocation_id\":\"second\",\"offset_ms\":0}}", "a".repeat(64)).unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "incomplete_run"
    );
    write!(log, "{{\"event\":").unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "incomplete_run"
    );
}

#[test]
fn failed_and_cancelled_latest_invocations_export_terminal_evidence() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-run-export-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for cancelled in [false, true] {
        let fixture = source("image/png");
        let parent = tempfile::tempdir().unwrap();
        let mut events = vec![
            json!({"event":"run_started", "charged_usd":0}),
            json!({"event":"node_started", "id":"draw#1", "path":"draw", "uses":"acme/draw@1", "with":{}, "reads":[]}),
        ];
        if cancelled {
            // Cancellation can terminate an executing step before a node terminal event.
            events.push(json!({"event":"cancel_requested"}));
            events.push(json!({"event":"run_cancelled", "charged_usd":0}));
        } else {
            events.push(json!({"event":"node_failed", "id":"draw#1", "path":"draw", "error":"Synthetic second-invocation failure", "duration_ms":5}));
            events.push(json!({"event":"run_finished", "ok":false, "charged_usd":0, "outputs":{}}));
        }
        let mut log = OpenOptions::new()
            .append(true)
            .open(fixture.root().join("events.jsonl"))
            .unwrap();
        for (offset, mut event) in events.into_iter().enumerate() {
            event["kind"] = json!("fx-run-events-v1");
            event["plan"] = json!("a".repeat(64));
            event["invocation_id"] = json!("second-invocation");
            event["offset_ms"] = json!(offset);
            writeln!(log, "{event}").unwrap();
        }
        drop(log);
        export(&fixture, &parent, ExportOptions::default()).unwrap();
        let data = manifest(&parent);
        assert_eq!(
            data["view"]["state"],
            if cancelled { "cancelled" } else { "failed" }
        );
        assert_eq!(data["view"]["outputs"], json!({}));
        let draw = data["view"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "draw#1")
            .unwrap();
        assert_eq!(draw["state"], "failed");
        assert_eq!(draw["cache"], Value::Null);
        assert_eq!(draw["outputs"], json!({}));
        assert_eq!(draw["error"], Value::Null);
        if cancelled {
            assert_eq!(draw["interrupted"], true);
        }
        validator.validate(&data).unwrap();
    }
}

#[test]
fn existing_destination_is_never_replaced_and_source_cannot_be_destination_parent() {
    let fixture = source("image/png");
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir(parent.path().join("export")).unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "destination_exists"
    );
    assert_eq!(
        run_export(
            &fixture.root(),
            &fixture.root().join("export"),
            "export",
            ExportOptions::default()
        )
        .unwrap_err()
        .code,
        "destination_invalid"
    );
    let temporary = tempfile::tempdir_in(parent.path()).unwrap();
    std::fs::write(temporary.path().join("marker"), b"preserve").unwrap();
    let destination = parent.path().join("existing");
    std::fs::create_dir(&destination).unwrap();
    assert!(place_directory(temporary.path(), &destination).is_err());
    assert!(temporary.path().join("marker").exists());
}

#[cfg(unix)]
#[test]
fn symbolic_links_in_lock_artifacts_and_destination_are_refused() {
    use std::os::unix::fs::symlink;
    let fixture = source("image/png");
    let parent = tempfile::tempdir().unwrap();
    let placed = fixture.root().join("files/draw/image.png");
    let foreign = parent.path().join("image.png");
    std::fs::copy(&placed, &foreign).unwrap();
    std::fs::remove_file(&placed).unwrap();
    symlink(&foreign, &placed).unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "artifact_unavailable"
    );
    std::fs::remove_file(fixture.root().join("run.lock")).unwrap();
    symlink(&foreign, fixture.root().join("run.lock")).unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "unavailable"
    );
    let fixture = source("image/png");
    symlink("absent", parent.path().join("export")).unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "destination_exists"
    );
}

#[cfg(unix)]
#[test]
fn symbolic_links_cannot_substitute_saved_plan_or_event_evidence() {
    use std::os::unix::fs::symlink;
    for name in ["plan.json", "events.jsonl"] {
        let fixture = source("image/png");
        let parent = tempfile::tempdir().unwrap();
        let original = fixture.root().join(name);
        let outside = parent.path().join(name);
        std::fs::copy(&original, &outside).unwrap();
        std::fs::remove_file(&original).unwrap();
        symlink(&outside, &original).unwrap();
        assert_eq!(
            export(&fixture, &parent, ExportOptions::default())
                .unwrap_err()
                .code,
            "invalid_record"
        );
        assert!(!parent.path().join("export").exists());
    }
}

#[test]
fn nested_collection_media_is_preserved_and_original_empty_values_are_withheld() {
    let value = json!({"collection":[["first",{"list":[{"file":{"digest":"a".repeat(64),"kind":"image/png","name":"/private/input.png","size":8,"private_path":"/private/input.png"}},null,{},[]]}]]});
    let projected = export_value(&value, false);
    let values = projected["collection"][0][1]["list"].as_array().unwrap();
    assert_eq!(values[0]["file"]["name"], "input.png");
    assert!(values[0]["file"].get("private_path").is_none());
    for value in &values[1..] {
        assert_eq!(*value, json!({"withheld":true}));
    }
}

#[test]
fn missing_node_dependencies_refuse_export_without_a_destination() {
    for field in ["reads", "needs", "judges"] {
        let fixture = source("image/png");
        let path = fixture.root().join("plan.json");
        let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        plan["instances"][0][field] = if field == "judges" {
            json!("missing#1")
        } else {
            json!(["missing#1"])
        };
        let dependency = plan["instances"][0][field].clone();
        std::fs::write(path, plan.to_string()).unwrap();
        let events_path = fixture.root().join("events.jsonl");
        let events: Vec<_> = std::fs::read_to_string(&events_path)
            .unwrap()
            .lines()
            .map(|line| {
                let mut event: Value = serde_json::from_str(line).unwrap();
                if event["event"] == "node_started" && event["id"] == "draw#1" {
                    event[field] = dependency.clone();
                }
                event.to_string() + "\n"
            })
            .collect();
        std::fs::write(events_path, events.concat()).unwrap();
        let parent = tempfile::tempdir().unwrap();
        assert_eq!(
            export(&fixture, &parent, ExportOptions::default())
                .unwrap_err()
                .code,
            "invalid_record",
            "missing {field} dependency"
        );
        assert!(!parent.path().join("export").exists());
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }
}

#[test]
fn frozen_reference_closure_refuses_broken_scopes_ports_and_layout() {
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../../spec/vectors/exports/minimal.json"
    ))
    .unwrap();
    manifest["view"]["nodes"][0]["ports"] = json!({
        "inputs": {}, "outputs": {"image":"image"}, "params": {}
    });
    manifest["view"]["scopes"] = json!([{
        "id":"scope:sub#1", "parent":null, "kind":"workflow", "path":"sub",
        "step":"sub", "take":[1], "title":"Subworkflow", "source":null,
        "ports":{"inputs":{},"outputs":["result"]}, "input_bindings":[],
        "output_bindings":[{"source":"prepare#1","source_port":"image","source_kind":"output","target_port":"result"}],
        "nodes":["prepare#1"], "pending":[]
    }]);
    manifest["layout"]["cells"]["sub"] = json!({"column":0,"row":0,"source":"automatic"});
    manifest["layout"]["order"]
        .as_array_mut()
        .unwrap()
        .push(json!("scope:sub#1"));
    references::validate(&manifest).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-run-export-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let cases: &[fn(&mut Value)] = &[
        |value| value["view"]["scopes"][0]["parent"] = json!("scope:missing#1"),
        |value| value["view"]["scopes"][0]["parent"] = json!("scope:sub#1"),
        |value| value["view"]["scopes"][0]["nodes"] = json!(["missing#1"]),
        |value| value["view"]["scopes"][0]["pending"] = json!(["missing"]),
        |value| value["view"]["scopes"][0]["output_bindings"][0]["source"] = json!("missing#1"),
        |value| value["view"]["scopes"][0]["output_bindings"][0]["source_port"] = json!("missing"),
        |value| value["view"]["scopes"][0]["output_bindings"][0]["target_port"] = json!("missing"),
        |value| {
            value["layout"]["cells"]
                .as_object_mut()
                .unwrap()
                .remove("prepare");
        },
        |value| value["layout"]["order"] = json!(["scope:sub#1"]),
        |value| value["layout"]["order"] = json!(["prepare#1"]),
        |value| value["layout"]["order"] = json!(["prepare#1", "scope:sub#1", "missing#1"]),
        |value| value["view"]["nodes"][0]["outputs"] = json!({"image":{"file":{"digest":"b".repeat(64),"kind":"image/png","name":"image.png","size":8}}}),
    ];
    for (index, mutate) in cases.iter().enumerate() {
        let mut invalid = manifest.clone();
        mutate(&mut invalid);
        validator.validate(&invalid).unwrap();
        assert_eq!(
            references::validate(&invalid).unwrap_err().code,
            "invalid_record",
            "case {index}"
        );
    }
}

#[test]
fn pending_repeat_dependencies_are_verified_without_requiring_legacy_metadata() {
    for waiting_on in [Some("draw#1"), Some("missing#1"), None] {
        let fixture = source("image/png");
        let plan_path = fixture.root().join("plan.json");
        let mut plan: Value = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
        let mut pending = json!({"path":"repeat", "max":2, "phase":2, "high_usd":0});
        if let Some(id) = waiting_on {
            pending["waiting_on"] = json!([id]);
        }
        plan["pending"] = json!([pending]);
        std::fs::write(plan_path, plan.to_string()).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let result = export(&fixture, &parent, ExportOptions::default());
        if waiting_on == Some("missing#1") {
            assert_eq!(result.unwrap_err().code, "invalid_record");
            assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
        } else {
            result.unwrap();
            assert_eq!(
                manifest(&parent)["view"]["pending"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
    }
}

#[test]
fn structural_skip_causes_are_portable_without_disclosing_diagnostic_payloads() {
    use std::io::Write;

    let fixture = source("image/png");
    let event = json!({
        "kind":"fx-run-events-v1", "event":"node_skipped", "plan":"a".repeat(64),
        "invocation_id":"offline-fixture", "offset_ms":0,
        "id":"later#1", "path":"later", "blocked":true,
        "skip_reason":{"code":"upstream_failed","sources":["draw#1"],"private_text":"withheld-author-value"}
    });
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(fixture.root().join("events.jsonl"))
            .unwrap(),
        "{event}"
    )
    .unwrap();
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(fixture.root().join("events.jsonl"))
            .unwrap(),
        "{}",
        json!({"kind":"fx-run-events-v1","event":"run_finished","plan":"a".repeat(64),
            "invocation_id":"offline-fixture","offset_ms":0,"ok":false,"charged_usd":0,"outputs":{}})
    )
    .unwrap();
    let parent = tempfile::tempdir().unwrap();
    export(&fixture, &parent, ExportOptions::default()).unwrap();
    let data = manifest(&parent);
    let node = data["view"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "later#1")
        .unwrap();
    assert_eq!(
        node["skip_reason"],
        json!({"code":"upstream_failed","sources":["draw#1"]})
    );
    assert!(node["error"].is_null());
    assert!(!data.to_string().contains("withheld-author-value"));
    assert!(!data.to_string().contains("private_text"));
    references::validate(&data).unwrap();

    let mut dangling = data.clone();
    dangling["view"]["nodes"][1]["skip_reason"]["sources"] = json!(["unknown#1"]);
    assert_eq!(
        references::validate(&dangling).unwrap_err().code,
        "invalid_record"
    );

    let mut unrelated_state = data.clone();
    unrelated_state["view"]["nodes"][1]["state"] = json!("succeeded");
    assert_eq!(
        references::validate(&unrelated_state).unwrap_err().code,
        "invalid_record"
    );
}

#[test]
fn provider_evidence_is_exported_only_with_explicit_diagnostic_disclosure() {
    use std::io::Write;
    let fixture = source("image/png");
    let diagnostic = json!({"source":"provider","phase":"submit","http_status":400,
        "code":"bad_input","message":"Unsupported native format","detail":"available",
        "thread_id":"thread-a","turn_id":"turn-a","ephemeral":true});
    let mut events = std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.root().join("events.jsonl"))
        .unwrap();
    for event in [
        json!({"event":"node_failed","id":"later#1","path":"later","error":"Provider rejected the input","diagnostic":diagnostic}),
        json!({"event":"run_finished","ok":false,"charged_usd":0,"outputs":{}}),
    ] {
        let mut event = event;
        event["kind"] = json!("fx-run-events-v1");
        event["plan"] = json!("a".repeat(64));
        event["invocation_id"] = json!("offline-fixture");
        event["offset_ms"] = json!(0);
        writeln!(events, "{event}").unwrap();
    }
    for include_diagnostics in [false, true] {
        let parent = tempfile::tempdir().unwrap();
        export(
            &fixture,
            &parent,
            ExportOptions {
                include_values: false,
                include_diagnostics,
            },
        )
        .unwrap();
        let data = manifest(&parent);
        let node = data["view"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "later#1")
            .unwrap();
        if include_diagnostics {
            assert_eq!(node["diagnostic"], diagnostic);
        } else {
            assert!(node.get("diagnostic").is_none());
            assert!(!data.to_string().contains("Unsupported native format"));
        }
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../spec/schemas/fx-run-export-v1.schema.json"
        ))
        .unwrap();
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&data)
            .unwrap();
    }
}

#[test]
fn captured_notes_are_disclosed_explicitly_without_exporting_local_source() {
    let fixture = source("image/png");
    let plan_path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&plan_path).unwrap()).unwrap();
    plan["notes"] = json!([{"id":"note:root:help","name":"help","scope_id":null,"about":"draw","format":"markdown","text":"## Original documentation\n${{ inputs.example }}","source":{"path":"notes/help.md","digest":"c".repeat(64)},"diagnostic":"A synthetic note diagnostic."}]);
    std::fs::write(&plan_path, plan.to_string()).unwrap();
    for include_values in [false, true] {
        let parent = tempfile::tempdir().unwrap();
        export(
            &fixture,
            &parent,
            ExportOptions {
                include_values,
                include_diagnostics: false,
            },
        )
        .unwrap();
        let value = manifest(&parent);
        let note = &value["view"]["notes"][0];
        assert!(note.get("source").is_none());
        assert!(note.get("diagnostic").is_none());
        assert!(value["view"].get("note_templates").is_none());
        assert_eq!(
            note["text"],
            if include_values {
                plan["notes"][0]["text"].clone()
            } else {
                Value::Null
            }
        );
        assert_eq!(
            note.get("withheld"),
            (!include_values).then_some(&Value::Bool(true))
        );
        assert!(value["layout"]["notes"].get("note:root:help").is_some());
        assert!(
            !value["layout"]["order"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "note:root:help")
        );
        let mut tampered = value.clone();
        tampered["layout"]["notes"]["note:root:unknown"] =
            json!({"column":0,"row":0,"source":"automatic"});
        assert!(references::validate(&tampered).is_err());
        tampered = value;
        tampered["view"]["notes"][0]["scope_id"] = json!("scope:unknown#1");
        assert!(references::validate(&tampered).is_err());
    }
}

#[test]
fn effective_values_export_with_explicit_disclosure_and_closed_references() {
    let fixture = source("image/png");
    let path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let input = format!("value:{}", "a".repeat(64));
    let file = format!("value:{}", "b".repeat(64));
    let ordinary = format!("value:{}", "c".repeat(64));
    plan["value_views"] = json!([
        {"id":input,"scope_id":null,"kind":"input","name":"prompt","state":"known","data":{"value":"A recorded default"},"diagnostic":"Controlled presentation diagnostic."},
        {"id":file,"scope_id":null,"kind":"output","name":"image","state":"known","data":{"file":{"digest":fixture.digest,"kind":"image/png","name":"original.png","size":b"a synthetic offline artifact".len()}}},
        {"id":ordinary,"scope_id":null,"kind":"table","name":"configuration","state":"known","data":{"value":{"file":{"digest":"d".repeat(64),"kind":"image/png","name":"not-a-file","size":1}}}}
    ]);
    plan["value_links"] = json!([{"source":{"kind":"instance","instance_id":"draw#1","port":"image","source_kind":"output"},"target":{"kind":"value","value_id":file}}]);
    std::fs::write(path, plan.to_string()).unwrap();
    for include_values in [false, true] {
        let parent = tempfile::tempdir().unwrap();
        export(
            &fixture,
            &parent,
            ExportOptions {
                include_values,
                include_diagnostics: false,
            },
        )
        .unwrap();
        let value = manifest(&parent);
        let views = value["view"]["value_views"].as_array().unwrap();
        assert_eq!(
            views[0]["state"],
            if include_values { "known" } else { "withheld" }
        );
        assert_eq!(views[0].get("data").is_some(), include_values);
        assert!(views[0].get("diagnostic").is_none());
        assert_eq!(views[1]["data"]["file"]["name"], "original.png");
        assert_eq!(views[1]["state"], "known");
        assert!(!value.to_string().contains("/private/"));
        assert!(
            !value["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|artifact| artifact["digest"] == "d".repeat(64))
        );
        assert!(value["layout"]["values"].get(&input).is_some());
        assert!(
            value["layout"]["order"]
                .as_array()
                .unwrap()
                .iter()
                .all(|id| id.as_str().is_none_or(|id| !id.starts_with("value:")))
        );
        references::validate(&value).unwrap();
        let mut broken = value.clone();
        broken["view"]["value_links"][0]["target"]["value_id"] =
            json!(format!("value:{}", "f".repeat(64)));
        assert!(references::validate(&broken).is_err());
    }
}

#[test]
fn recorded_input_snapshots_export_independently_of_transformed_outputs() {
    let fixture = source("image/png");
    let bytes = b"an original synthetic input, different from the transformed output";
    let digest = grida_fx_core::value::file_digest(bytes);
    let directory = fixture.root().join("values");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join(&digest), bytes).unwrap();
    let path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    plan["value_views"] = json!([{
        "id":format!("value:{}", "a".repeat(64)),"scope_id":null,"kind":"input","name":"reference","state":"known",
        "data":{"file":{"digest":digest,"kind":"image/png","name":"reference.png","size":bytes.len()}}
    }]);
    plan["value_links"] = json!([]);
    std::fs::write(path, plan.to_string()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    export(&fixture, &parent, ExportOptions::default()).unwrap();
    let value = manifest(&parent);
    let input = value["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["digest"] == digest)
        .unwrap();
    assert!(input["omitted_reason"].is_null());
    let relative = input["path"].as_str().unwrap();
    assert_eq!(
        std::fs::read(parent.path().join("export").join(relative)).unwrap(),
        bytes
    );
    assert_ne!(digest, fixture.digest);
    // Input snapshot failure is display-only: retaining metadata still allows an export.
    std::fs::remove_file(directory.join(&digest)).unwrap();
    let parent = tempfile::tempdir().unwrap();
    export(&fixture, &parent, ExportOptions::default()).unwrap();
    let value = manifest(&parent);
    let input = value["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["digest"] == digest)
        .unwrap();
    assert!(input["path"].is_null());
    assert_eq!(input["omitted_reason"], "unplaced_input");
    // Existing replacement bytes are a broken recorded candidate, never silently exported.
    std::fs::write(directory.join(&digest), b"replacement bytes").unwrap();
    let parent = tempfile::tempdir().unwrap();
    assert_eq!(
        export(&fixture, &parent, ExportOptions::default())
            .unwrap_err()
            .code,
        "artifact_unavailable"
    );
}

#[test]
fn nested_typed_input_files_keep_their_identity_without_reinterpreting_opaque_objects() {
    let fixture = source("image/png");
    let bytes = b"synthetic nested reference bytes";
    let digest = grida_fx_core::value::file_digest(bytes);
    std::fs::create_dir(fixture.root().join("values")).unwrap();
    std::fs::write(fixture.root().join("values").join(&digest), bytes).unwrap();
    let path = fixture.root().join("plan.json");
    let mut plan: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let opaque = json!({"object":{"image":{"file":{"digest":"f".repeat(64),"kind":"image/png","name":"opaque.png","size":1}}}});
    plan["value_views"] = json!([{
        "id":format!("value:{}", "a".repeat(64)),"scope_id":null,"kind":"input","name":"layers","state":"known",
        "data":{"list":[{"object":{
            "image":{"file":{"digest":digest,"kind":"image/png","name":"reference.png","size":bytes.len()}},
            "caption":{"value":"A nested caption"},
            "opaque":{"value":opaque}
        }}]}
    }]);
    plan["value_links"] = json!([]);
    std::fs::write(path, plan.to_string()).unwrap();
    for include_values in [false, true] {
        let parent = tempfile::tempdir().unwrap();
        export(
            &fixture,
            &parent,
            ExportOptions {
                include_values,
                include_diagnostics: false,
            },
        )
        .unwrap();
        let value = manifest(&parent);
        let object = &value["view"]["value_views"][0]["data"]["list"][0]["object"];
        assert_eq!(object["image"]["file"]["digest"], digest);
        assert_eq!(
            object["caption"],
            if include_values {
                json!({"value":"A nested caption"})
            } else {
                json!({"withheld":true})
            }
        );
        assert_eq!(
            object["opaque"],
            if include_values {
                json!({"value":opaque})
            } else {
                json!({"withheld":true})
            }
        );
        assert!(
            value["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|artifact| artifact["digest"] == digest && artifact["path"].is_string())
        );
        assert!(
            !value["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|artifact| artifact["digest"] == "f".repeat(64))
        );
        let mut invalid = value;
        invalid["view"]["value_views"][0]["data"]["list"][0]["object"]["image"]["file"]["digest"] =
            json!("e".repeat(64));
        assert!(references::validate(&invalid).is_err());
    }
}
