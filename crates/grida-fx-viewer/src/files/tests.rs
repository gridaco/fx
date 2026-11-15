use super::*;
use crate::tests::Fixture;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use grida_fx_core::value::file_digest;
use tempfile::TempDir;
use tower::ServiceExt;

fn reference(target: Value) -> Value {
    json!({"subject":{"kind":"run","workflow":{"id":"sample"}},"target":target})
}

fn artifact_target(fixture: &Fixture) -> Value {
    let list = list(&fixture.root(), None).unwrap();
    list["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["target"]["artifact"]["digest"] == fixture.digest)
        .unwrap()["target"]
        .clone()
}

fn source_fixture() -> (Fixture, TempDir) {
    let fixture = Fixture::new("image/png");
    let project = tempfile::tempdir().unwrap();
    fs::create_dir(project.path().join("nodes")).unwrap();
    fs::create_dir(project.path().join("workflows")).unwrap();
    fs::write(project.path().join("fx.yaml"), "fx: project/v1\n").unwrap();
    let source = b"raise RuntimeError('opening source must never import this module')\n";
    fs::write(project.path().join("nodes/render.py"), source).unwrap();
    fs::write(
        project.path().join("workflows/main.yaml"),
        "This is current editable workflow text.\n",
    )
    .unwrap();
    fs::write(
        project.path().join("workflows/child.yaml"),
        "This is an imported workflow source.\n",
    )
    .unwrap();
    let path = fixture.root().join("plan.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    graph["workflow"]["file"] = json!("workflows/main.yaml");
    graph["instances"][0]["uses"] = json!("./nodes/render.py#render");
    graph["instances"][0]["type"] = json!("nodes/render.py#render@1");
    graph["types"] = json!({"./nodes/render.py#render":{
        "identity":"nodes/render.py#render@1",
        "source":{"files":{"nodes/render.py":file_digest(source)},"resources":{}},
    }});
    graph["scopes"] = json!([{
        "id":"scope:child#1","parent":null,"kind":"workflow","path":"child","step":"child",
        "take":[1],"title":"Child workflow","source":"workflows/child.yaml",
        "ports":{"inputs":{},"outputs":[]},"input_bindings":[],"output_bindings":[],
        "nodes":["draw#1"],"pending":[],
    }]);
    fs::write(&path, graph.to_string()).unwrap();
    let log = fixture.root().join("events.jsonl");
    let rewritten: String = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| {
            let mut event: Value = serde_json::from_str(line).unwrap();
            if event["id"] == "draw#1" && event.get("uses").is_some() {
                event["uses"] = json!("./nodes/render.py#render");
            }
            format!("{event}\n")
        })
        .collect();
    fs::write(log, rewritten).unwrap();
    (fixture, project)
}

fn node_target(fixture: &Fixture) -> Value {
    list(&fixture.root(), Some("draw#1")).unwrap()["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["role"] == "node")
        .unwrap()["target"]
        .clone()
}

#[test]
fn resolves_only_recorded_verified_artifact_bytes() {
    let fixture = Fixture::new("image/png");
    let target = artifact_target(&fixture);
    let request = reference(target.clone());
    let resolved = resolve(&fixture.root(), &request, None, None).unwrap();
    let path = Path::new(resolved["local_path"].as_str().unwrap());
    assert!(path.is_absolute() && path.starts_with(fixture.root()));
    assert_eq!(resolved["source_match"], Value::Null);
    fs::write(path, b"tampered synthetic data").unwrap();
    let unavailable = resolve(&fixture.root(), &request, None, None).unwrap();
    assert_eq!(unavailable["local_path"], Value::Null);
    assert!(unavailable["reason"].as_str().is_some());
    let mut unknown = target;
    unknown["artifact"]["digest"] = json!("f".repeat(64));
    assert_eq!(
        resolve(&fixture.root(), &reference(unknown), None, None)
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[test]
fn source_opening_is_current_read_only_and_never_imports_code() {
    let (fixture, project) = source_fixture();
    let target = node_target(&fixture);
    let request = reference(target);
    let found = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
    assert_eq!(found["source_match"], "match");
    assert_eq!(
        Path::new(found["local_path"].as_str().unwrap()),
        project
            .path()
            .canonicalize()
            .unwrap()
            .join("nodes/render.py")
    );
    fs::write(
        project.path().join("nodes/render.py"),
        "raise RuntimeError('changed source is still editable')\n",
    )
    .unwrap();
    let changed = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
    assert_eq!(changed["source_match"], "changed");
    assert!(changed["local_path"].is_string());
    let unbound = resolve(&fixture.root(), &request, None, None).unwrap();
    assert_eq!(unbound["local_path"], Value::Null);
    fs::remove_file(project.path().join("nodes/render.py")).unwrap();
    assert_eq!(
        resolve(&fixture.root(), &request, Some(project.path()), None).unwrap()["local_path"],
        Value::Null
    );
}

#[test]
fn source_inventory_covers_workflow_imports_nodes_and_instance_filtering() {
    let (fixture, project) = source_fixture();
    let all = list(&fixture.root(), None).unwrap();
    assert!(all["files"].as_array().unwrap().iter().any(
        |file| file["role"] == "workflow" && file["target"]["source"] == "workflows/main.yaml"
    ));
    let imported = all["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["scope_id"] == "scope:child#1")
        .unwrap();
    let resolved = resolve(
        &fixture.root(),
        &reference(imported["target"].clone()),
        Some(project.path()),
        None,
    )
    .unwrap();
    assert!(
        resolved["local_path"]
            .as_str()
            .unwrap()
            .ends_with("workflows/child.yaml")
    );
    assert_eq!(resolved["source_match"], "unknown");
    let one = list(&fixture.root(), Some("draw#1")).unwrap();
    assert!(
        one["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file.get("instance_id").is_none_or(|id| id == "draw#1"))
    );
    assert_eq!(
        list(&fixture.root(), Some("unknown#1")).unwrap_err().code,
        "not_found"
    );
    assert!(!all.to_string().contains(project.path().to_str().unwrap()));
    assert!(!all.to_string().contains(fixture.root().to_str().unwrap()));
}

#[test]
fn references_cannot_select_arbitrary_project_files_or_escape_paths() {
    let (fixture, project) = source_fixture();
    fs::write(project.path().join("private.txt"), "not a recorded source").unwrap();
    let unknown = reference(json!({"kind":"source","source":"private.txt"}));
    assert_eq!(
        resolve(&fixture.root(), &unknown, Some(project.path()), None)
            .unwrap_err()
            .code,
        "not_found"
    );
    for source in [
        "../private.txt",
        "/tmp/private.txt",
        "file:///tmp/private.txt",
        "nodes/../private.txt",
        "nodes\\render.py",
    ] {
        let request = reference(json!({"kind":"source","source":source}));
        assert_eq!(
            resolve(&fixture.root(), &request, Some(project.path()), None)
                .unwrap_err()
                .code,
            "invalid_request"
        );
    }
}

#[cfg(unix)]
#[test]
fn source_hashing_keeps_its_open_handle_after_path_replacement() {
    use std::os::unix::fs::symlink;

    for replace_parent in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fs::create_dir(root.join("nodes")).unwrap();
        let path = root.join("nodes/render.py");
        let bytes = b"confined source bytes";
        fs::write(&path, bytes).unwrap();
        let foreign = tempfile::tempdir().unwrap();
        fs::write(foreign.path().join("render.py"), b"outside source bytes").unwrap();
        let (mut file, _) = read::open_confined_file(&root, "nodes/render.py").unwrap();
        if replace_parent {
            fs::rename(root.join("nodes"), root.join("previous")).unwrap();
            symlink(foreign.path(), root.join("nodes")).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
            symlink(foreign.path().join("render.py"), &path).unwrap();
        }
        assert_eq!(source_digest(&mut file).unwrap(), file_digest(bytes));
    }
}

#[cfg(unix)]
#[test]
fn source_fifo_replacement_is_unavailable_with_or_without_recorded_digest() {
    for recorded_digest in [false, true] {
        let (fixture, project) = source_fixture();
        let target = if recorded_digest {
            node_target(&fixture)
        } else {
            json!({"kind":"source","source":"workflows/main.yaml"})
        };
        let request = reference(target.clone());
        let initial = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
        assert!(initial["local_path"].is_string());
        let path = project.path().join(target["source"].as_str().unwrap());
        fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let resolved = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
            sent.send(resolved).unwrap();
        });
        let resolved = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("source resolution must not wait for a FIFO writer");
        reader.join().unwrap();
        assert_eq!(resolved["local_path"], Value::Null);
        assert_eq!(resolved["source_match"], "unknown");
    }
}

#[cfg(unix)]
#[test]
fn source_links_are_unavailable_even_when_pointing_inside_the_bound_root() {
    let (fixture, project) = source_fixture();
    let request = reference(node_target(&fixture));
    fs::rename(
        project.path().join("nodes/render.py"),
        project.path().join("nodes/actual.py"),
    )
    .unwrap();
    std::os::unix::fs::symlink("actual.py", project.path().join("nodes/render.py")).unwrap();
    assert_eq!(
        resolve(&fixture.root(), &request, Some(project.path()), None).unwrap()["local_path"],
        Value::Null
    );
}

#[test]
fn saved_plan_discovery_supports_builders_without_running_them() {
    let (fixture, project) = source_fixture();
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root().join("plan.json")).unwrap()).unwrap();
    graph["workflow"]["file"] = json!("nodes/render.py:build");
    let saved = project.path().join("saved.json");
    fs::write(&saved, graph.to_string()).unwrap();
    let all = list(&saved, None).unwrap();
    let target = all["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["role"] == "workflow" && file["target"]["symbol"] == "build")
        .unwrap()["target"]
        .clone();
    let request = json!({"subject":{"kind":"plan","workflow":{"id":"sample","source":"nodes/render.py:build"}},"target":target});
    let resolved = resolve(&saved, &request, Some(project.path()), None).unwrap();
    assert!(
        resolved["local_path"]
            .as_str()
            .unwrap()
            .ends_with("nodes/render.py")
    );
    assert_eq!(resolved["source_match"], "unknown");
}

#[test]
fn exact_service_subject_is_validated_and_source_root_can_differ() {
    let (fixture, source_root) = source_fixture();
    let planning = tempfile::tempdir().unwrap();
    let catalog = service::Catalog::open(planning.path(), &planning.path().join("runs")).unwrap();
    let entry = catalog.register_run(&fixture.root()).unwrap();
    let mut request = reference(node_target(&fixture));
    request["subject"]["reference"] = json!({"kind":"service_entry","project_id":catalog.project_id(),"entry_kind":"run","entry_id":entry.id});
    let resolved = resolve(
        &fixture.root(),
        &request,
        Some(planning.path()),
        Some(source_root.path()),
    )
    .unwrap();
    assert!(
        resolved["local_path"]
            .as_str()
            .unwrap()
            .ends_with("nodes/render.py")
    );
    assert_eq!(
        resolve(&fixture.root(), &request, None, Some(source_root.path()))
            .unwrap_err()
            .code,
        "stale_subject"
    );
    request["subject"]["reference"]["entry_id"] = json!("f".repeat(64));
    assert_eq!(
        resolve(
            &fixture.root(),
            &request,
            Some(planning.path()),
            Some(source_root.path())
        )
        .unwrap_err()
        .code,
        "stale_subject"
    );
}

#[test]
fn subject_changes_cannot_silently_retarget_an_activation() {
    let fixture = Fixture::new("image/png");
    let target = artifact_target(&fixture);
    for subject in [
        json!({"kind":"run","workflow":{"id":"other"}}),
        json!({"kind":"plan"}),
        json!({"kind":"run","reference":{"kind":"export","manifest_sha256":"a".repeat(64)}}),
    ] {
        assert_eq!(
            resolve(
                &fixture.root(),
                &json!({"subject":subject,"target":target}),
                None,
                None
            )
            .unwrap_err()
            .code,
            "stale_subject"
        );
    }
}

#[tokio::test]
async fn http_inventory_contains_only_portable_descriptors_and_scoped_artifact_urls() {
    let (fixture, project) = source_fixture();
    let app = crate::router(crate::AppState {
        source: crate::Source::Run(fixture.root()),
        authority: "127.0.0.1:43123".into(),
        artifact_prefix: "/p/acme/runs/one".into(),
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/files")
                .header(header::HOST, "127.0.0.1:43123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(document["kind"], "fx-files-v1");
    assert!(!text.contains(project.path().to_str().unwrap()));
    assert!(!text.contains(fixture.root().to_str().unwrap()));
    assert!(text.contains(&format!(
        "/p/acme/runs/one/api/artifacts/{}",
        fixture.digest
    )));
}

#[test]
fn native_documents_follow_their_language_neutral_schemas() {
    let (fixture, project) = source_fixture();
    let inventory = list(&fixture.root(), None).unwrap();
    let files_schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-files-v1.schema.json"
    ))
    .unwrap();
    jsonschema::validator_for(&files_schema)
        .unwrap()
        .validate(&inventory)
        .unwrap();
    let resolution_schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-file-resolution-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&resolution_schema).unwrap();
    for (target, bound) in [
        (node_target(&fixture), Some(project.path())),
        (artifact_target(&fixture), None),
        (node_target(&fixture), None),
    ] {
        let result = resolve(&fixture.root(), &reference(target), bound, None).unwrap();
        validator.validate(&result).unwrap();
    }
    let error_schema: Value = serde_json::from_str(include_str!(
        "../../../../spec/schemas/fx-file-error-v1.schema.json"
    ))
    .unwrap();
    let failure =
        serde_json::to_value(resolve(&fixture.root(), &json!({}), None, None).unwrap_err())
            .unwrap();
    jsonschema::validator_for(&error_schema)
        .unwrap()
        .validate(&failure)
        .unwrap();
}

#[test]
fn packaged_source_labels_are_not_guessed_from_lookalike_project_paths() {
    let (fixture, project) = source_fixture();
    fs::write(
        project.path().join("fx.yaml"),
        "fx: project/v1\nsources: [acme_tools]\n",
    )
    .unwrap();
    fs::create_dir(project.path().join("acme_tools")).unwrap();
    fs::write(
        project.path().join("acme_tools/helper.py"),
        "lookalike project file\n",
    )
    .unwrap();
    let plan = fixture.root().join("plan.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    graph["types"]["./nodes/render.py#render"]["source"]["files"]["acme_tools/helper.py"] =
        json!(file_digest(b"lookalike project file\n"));
    fs::write(plan, graph.to_string()).unwrap();
    let targets = list(&fixture.root(), None).unwrap();
    let target = targets["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["target"]["source"] == "acme_tools/helper.py")
        .unwrap()["target"]
        .clone();
    let request = reference(target);
    for config in [
        Some("fx: project/v1\nsources: [acme_tools]\n"),
        Some("fx: [invalid: yaml\n"),
        Some("fx: project/v1\n"),
        None,
    ] {
        match config {
            Some(text) => fs::write(project.path().join("fx.yaml"), text).unwrap(),
            None => fs::remove_file(project.path().join("fx.yaml")).unwrap(),
        }
        let unresolved = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
        assert_eq!(unresolved["local_path"], Value::Null);
        assert!(
            unresolved["reason"]
                .as_str()
                .unwrap()
                .contains("source-root")
        );
    }
    // The host can explicitly bind the base for this complete portable label. That
    // authority is independent of the current project's missing/broken package list.
    let bound = resolve(
        &fixture.root(),
        &request,
        Some(project.path()),
        Some(project.path()),
    )
    .unwrap();
    assert!(
        bound["local_path"]
            .as_str()
            .unwrap()
            .ends_with("acme_tools/helper.py")
    );
    assert_eq!(bound["source_match"], "match");
}

#[test]
fn source_label_collisions_preserve_closure_and_direct_provenance() {
    let (fixture, project) = source_fixture();
    fs::create_dir(project.path().join("acme_tools")).unwrap();
    let local = b"// Current project resource, unrelated to the declared source package.\n";
    let packaged = b"export const helper = 'declared package source';\n";
    fs::write(project.path().join("acme_tools/helper.js"), local).unwrap();
    let path = fixture.root().join("plan.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Declared TypeScript/JavaScript packages use logical package labels even
    // when their bytes live in node_modules or a linked workspace package.
    graph["types"]["./nodes/render.py#render"]["source"]["files"]["acme_tools/helper.js"] =
        json!(file_digest(packaged));
    graph["types"]["./nodes/render.py#render"]["source"]["resources"]["acme_tools/helper.js"] =
        json!(file_digest(local));
    fs::write(&path, graph.to_string()).unwrap();
    let targets = list(&fixture.root(), None).unwrap();
    let files = targets["files"].as_array().unwrap();
    let target_for = |role| {
        files
            .iter()
            .find(|file| file["role"] == role && file["target"]["source"] == "acme_tools/helper.js")
            .unwrap()["target"]
            .clone()
    };
    let closure = target_for("source");
    let unbound = resolve(
        &fixture.root(),
        &reference(closure.clone()),
        Some(project.path()),
        None,
    )
    .unwrap();
    assert_eq!(unbound["local_path"], Value::Null);
    assert_eq!(unbound["source_match"], "unknown");
    let resource = target_for("resource");
    let found = resolve(
        &fixture.root(),
        &reference(resource),
        Some(project.path()),
        None,
    )
    .unwrap();
    assert!(found["local_path"].is_string());
    assert_eq!(found["source_match"], "match");
    let explicitly_bound = resolve(
        &fixture.root(),
        &reference(closure.clone()),
        Some(project.path()),
        Some(project.path()),
    )
    .unwrap();
    assert!(explicitly_bound["local_path"].is_string());
    assert_eq!(explicitly_bound["source_match"], "changed");
    let mut unspecified = closure;
    unspecified
        .as_object_mut()
        .unwrap()
        .remove("recorded_digest");
    assert_eq!(
        resolve(
            &fixture.root(),
            &reference(unspecified),
            Some(project.path()),
            Some(project.path()),
        )
        .unwrap_err()
        .code,
        "invalid_request"
    );

    // Equal contents do not establish which namespace an identical reference
    // selects. An explicit source-root binding is still required for that case.
    graph["types"]["./nodes/render.py#render"]["source"]["files"]["acme_tools/helper.js"] =
        json!(file_digest(local));
    fs::write(&path, graph.to_string()).unwrap();
    let target = json!({"kind":"source","source":"acme_tools/helper.js","recorded_digest":file_digest(local)});
    assert_eq!(
        resolve(
            &fixture.root(),
            &reference(target.clone()),
            Some(project.path()),
            None,
        )
        .unwrap()["local_path"],
        Value::Null
    );
    assert_eq!(
        resolve(
            &fixture.root(),
            &reference(target),
            Some(project.path()),
            Some(project.path()),
        )
        .unwrap()["source_match"],
        "match"
    );
    assert_eq!(
        resolve(
            &fixture.root(),
            &reference(node_target(&fixture)),
            Some(project.path()),
            None,
        )
        .unwrap()["source_match"],
        "match"
    );
}

#[test]
fn direct_source_remains_openable_when_project_configuration_needs_repair() {
    let (fixture, project) = source_fixture();
    let request = reference(node_target(&fixture));
    for config in [Some("fx: [invalid: yaml\n"), None] {
        match config {
            Some(text) => fs::write(project.path().join("fx.yaml"), text).unwrap(),
            None => fs::remove_file(project.path().join("fx.yaml")).unwrap(),
        }
        let found = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
        assert!(
            found["local_path"]
                .as_str()
                .unwrap()
                .ends_with("nodes/render.py")
        );
        assert_eq!(found["source_match"], "match");
    }
}

#[test]
fn source_hash_comparison_reads_all_chunks_without_importing_the_file() {
    let (fixture, project) = source_fixture();
    let source = vec![b'x'; 3 * 64 * 1024 + 17];
    fs::write(project.path().join("nodes/render.py"), &source).unwrap();
    let path = fixture.root().join("plan.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    graph["types"]["./nodes/render.py#render"]["source"]["files"]["nodes/render.py"] =
        json!(file_digest(&source));
    fs::write(path, graph.to_string()).unwrap();
    let request = reference(node_target(&fixture));
    assert_eq!(
        resolve(&fixture.root(), &request, Some(project.path()), None).unwrap()["source_match"],
        "match"
    );
    let mut changed = source;
    *changed.last_mut().unwrap() = b'y';
    fs::write(project.path().join("nodes/render.py"), changed).unwrap();
    assert_eq!(
        resolve(&fixture.root(), &request, Some(project.path()), None).unwrap()["source_match"],
        "changed"
    );
}

#[test]
fn portable_target_validation_rejects_private_or_unsafe_metadata() {
    let fixture = Fixture::new("image/png");
    let target = artifact_target(&fixture);
    for (member, value) in [
        ("url", json!("https://user:secret@acme.invalid/file")),
        ("url", json!("https://acme.invalid/file?secret=value")),
        ("url", json!("https://acme.invalid/file#fragment")),
        ("url", json!("//acme.invalid/file")),
        ("url", json!("/api/\\file")),
        ("name", json!("/private/workspace/file.png")),
        ("name", json!("C:\\workspace\\file.png")),
        ("name", json!("file:///private/workspace/file.png")),
        ("name", json!("bad\nname")),
        ("name", json!("bad\u{85}name")),
        ("kind", json!("")),
        ("kind", json!("image/png\n")),
        ("size", json!(9_007_199_254_740_992_u64)),
    ] {
        let mut invalid = target.clone();
        invalid["artifact"][member] = value;
        assert_eq!(
            resolve(&fixture.root(), &reference(invalid), None, None)
                .unwrap_err()
                .code,
            "invalid_request",
            "{member}"
        );
    }
}

#[test]
fn captured_note_sources_reuse_private_native_resolution_without_changing_snapshot() {
    let (fixture, project) = source_fixture();
    fs::create_dir(project.path().join("notes")).unwrap();
    let original = b"## The original note\n";
    fs::write(project.path().join("notes/help.md"), original).unwrap();
    let path = fixture.root().join("plan.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    graph["notes"] = json!([{"id":"note:root:help","name":"help","scope_id":null,"about":null,"format":"markdown","text":"## The original note\n","source":{"path":"notes/help.md","digest":file_digest(original)}}]);
    fs::write(&path, graph.to_string()).unwrap();
    let inventory = list(&fixture.root(), None).unwrap();
    let target = inventory["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["target"]["source"] == "notes/help.md")
        .unwrap()["target"]
        .clone();
    let request = reference(target);
    assert_eq!(
        resolve(&fixture.root(), &request, Some(project.path()), None).unwrap()["source_match"],
        "match"
    );
    fs::write(project.path().join("notes/help.md"), "Current edited note").unwrap();
    let changed = resolve(&fixture.root(), &request, Some(project.path()), None).unwrap();
    assert_eq!(changed["source_match"], "changed");
    assert!(changed["local_path"].is_string());
    let snapshot = read::read_inventory(&fixture.root()).unwrap();
    assert_eq!(
        snapshot.document.notes.unwrap()[0]["text"],
        "## The original note\n"
    );
}
