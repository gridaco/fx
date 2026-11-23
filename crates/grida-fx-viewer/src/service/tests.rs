use super::*;
use crate::tests::{Fixture, static_plan};
use axum::body::to_bytes;
use tempfile::TempDir;

fn catalog() -> (TempDir, Catalog) {
    let directory = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(directory.path(), &directory.path().join("runs")).unwrap();
    (directory, catalog)
}

fn app(catalog: Catalog) -> (Router, oneshot::Receiver<()>) {
    app_with_launcher(catalog, None)
}

fn app_with_launcher(
    catalog: Catalog,
    run_launcher: Option<RunLauncher>,
) -> (Router, oneshot::Receiver<()>) {
    let (sender, receiver) = oneshot::channel();
    (
        service_router(ServiceState {
            catalog,
            authority: "127.0.0.1:43123".into(),
            url: "http://127.0.0.1:43123/".into(),
            instance_id: "instance-test".into(),
            control_token: "private-test-token".into(),
            run_launcher,
            stop: Arc::new(Mutex::new(Some(sender))),
        }),
        receiver,
    )
}

fn request(path: &str) -> axum::http::request::Builder {
    Request::builder()
        .uri(path)
        .header(header::HOST, "127.0.0.1:43123")
}

async fn value(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn recorded_workflow(
    fixture: &Fixture,
    source: &str,
    name: Option<&str>,
    created_at: Option<&str>,
) {
    let root = fixture.root();
    let plan_path = root.join("plan.json");
    let mut plan: Value = serde_json::from_slice(&fs::read(&plan_path).unwrap()).unwrap();
    plan["workflow"]["file"] = json!(source);
    fs::write(plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let events_path = root.join("events.jsonl");
    let events = fs::read_to_string(&events_path).unwrap();
    let (initial, rest) = events.split_once('\n').unwrap();
    let mut first: Value = serde_json::from_str(initial).unwrap();
    if let Some(name) = name {
        first["name"] = json!(name);
    }
    if let Some(created_at) = created_at {
        first["created_at"] = json!(created_at);
    }
    fs::write(events_path, format!("{first}\n{rest}")).unwrap();
}

#[test]
fn recorded_workflow_and_first_run_metadata_survive_resume_and_unavailability() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    recorded_workflow(
        &fixture,
        "workflows/sample.yaml",
        Some("character_1"),
        Some("2026-10-08T01:02:03.123Z"),
    );
    let first = catalog.register_run(&fixture.root()).unwrap();
    assert_eq!(
        first.workflow,
        Some(Workflow {
            id: "sample".into(),
            source: "workflows/sample.yaml".into()
        })
    );
    assert_eq!(first.name.as_deref(), Some("character_1"));
    assert_eq!(first.created_at, "2026-10-08T01:02:03.123Z");
    let reopened = Catalog::open(&catalog.root, &catalog.runs_dir).unwrap();
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(fixture.root().join("events.jsonl"))
        .unwrap();
    writeln!(log, "{}", json!({"kind":"fx-run-events-v1","event":"run_started","plan":"a".repeat(64),"invocation_id":"resumed","offset_ms":0,"name":"another","created_at":"2026-10-09T09:00:00.000Z"})).unwrap();
    let resumed = reopened.register_run(&fixture.root()).unwrap();
    assert_eq!(resumed.id, first.id);
    assert_eq!(resumed.created_at, first.created_at);
    assert_eq!(resumed.name, first.name);
    assert_eq!(resumed.workflow, first.workflow);
    assert_eq!(resumed.state, "unfinished");
    fs::remove_file(fixture.root().join("plan.json")).unwrap();
    let unavailable = reopened.entries().unwrap().pop().unwrap();
    assert_eq!(unavailable.state, "unavailable");
    assert_eq!(unavailable.created_at, first.created_at);
    assert_eq!(unavailable.name, first.name);
    assert_eq!(unavailable.workflow, first.workflow);
}

#[test]
fn inspected_run_registration_refuses_a_replaced_record() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    let root = fixture.root();
    let graph = serde_json::from_slice(&fs::read(root.join("plan.json")).unwrap()).unwrap();
    let events = fs::read_to_string(root.join("events.jsonl")).unwrap();
    let first = serde_json::from_str(events.lines().next().unwrap()).unwrap();
    let original = catalog
        .register_inspected_run(&root, &graph, &first)
        .unwrap();
    recorded_workflow(&fixture, "other.yaml", None, None);
    assert!(
        catalog
            .register_inspected_run(&root, &graph, &first)
            .is_err()
    );
    let entries = catalog.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, original.id);
    assert_eq!(entries[0].state, "unavailable");
}

#[test]
fn presentation_metadata_does_not_change_run_url_identity() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    recorded_workflow(
        &fixture,
        "sample.yaml",
        Some("first"),
        Some("2026-10-08T01:00:00.000Z"),
    );
    let first = catalog.register_run(&fixture.root()).unwrap();
    recorded_workflow(
        &fixture,
        "sample.yaml",
        Some("changed"),
        Some("2026-10-09T01:00:00.000Z"),
    );
    let second = catalog.register_run(&fixture.root()).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.name.as_deref(), Some("first"));
    assert_eq!(second.created_at, first.created_at);
    assert!(catalog.source(&first.id, "runs").is_ok());
}

#[test]
fn workflow_identity_distinguishes_sources_but_not_changed_inputs() {
    let (_directory, catalog) = catalog();
    let first = Fixture::new("image/png");
    recorded_workflow(&first, "first.yaml", None, None);
    let first = catalog.register_run(&first.root()).unwrap();
    let second_fixture = Fixture::new("image/png");
    recorded_workflow(&second_fixture, "second.yaml", None, None);
    let second = catalog.register_run(&second_fixture.root()).unwrap();
    assert_eq!(first.title, second.title);
    assert_ne!(first.workflow, second.workflow);
    let changed_fixture = Fixture::new("image/png");
    recorded_workflow(&changed_fixture, "first.yaml", None, None);
    let path = changed_fixture.root().join("plan.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    graph["inputs"]["caption"] = json!("Different input for the same authored workflow");
    fs::write(path, graph.to_string()).unwrap();
    let changed = catalog.register_run(&changed_fixture.root()).unwrap();
    assert_eq!(first.workflow, changed.workflow);
    assert_ne!(first.id, changed.id);
}

#[test]
fn legacy_run_catalog_uses_original_plan_time_and_is_not_rewritten() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    recorded_workflow(&fixture, "sample.yaml", None, None);
    let plan_path = fixture.root().join("plan.json");
    let time = DateTime::parse_from_rfc3339("2026-10-01T02:03:04.500Z")
        .unwrap()
        .with_timezone(&Utc);
    File::open(&plan_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time.into()))
        .unwrap();
    let first = catalog.register_run(&fixture.root()).unwrap();
    let path = catalog.state_dir.join(format!("entries/{}.json", first.id));
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record.as_object_mut().unwrap().remove("metadata");
    let legacy = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &legacy).unwrap();
    let reopened = Catalog::open(&catalog.root, &catalog.runs_dir).unwrap();
    let resumed = reopened.register_run(&fixture.root()).unwrap();
    assert_eq!(resumed.id, first.id);
    assert_eq!(resumed.created_at, "2026-10-01T02:03:04.500Z");
    assert_eq!(resumed.workflow, first.workflow);
    assert_eq!(fs::read(path).unwrap(), legacy);
}

#[test]
fn saved_plan_registration_retains_creation_time_and_accepts_legacy_catalogs() {
    let (_directory, catalog) = catalog();
    let graph = static_plan();
    let first = catalog.register_plan(&graph).unwrap();
    assert_eq!(
        first.workflow,
        Some(Workflow {
            id: "static-case".into(),
            source: "example.py:build".into()
        })
    );
    let reopened = Catalog::open(&catalog.root, &catalog.runs_dir).unwrap();
    assert_eq!(
        first.created_at,
        reopened.register_plan(&graph).unwrap().created_at
    );
    let path = catalog.state_dir.join(format!("entries/{}.json", first.id));
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record.as_object_mut().unwrap().remove("created_at");
    let legacy = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &legacy).unwrap();
    let time = DateTime::parse_from_rfc3339("2026-09-30T02:03:04.125Z")
        .unwrap()
        .with_timezone(&Utc);
    File::open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time.into()))
        .unwrap();
    let first_legacy = catalog.register_plan(&graph).unwrap();
    let repeated = reopened.register_plan(&graph).unwrap();
    assert_eq!(first_legacy.id, first.id);
    assert_eq!(first_legacy.created_at, "2026-09-30T02:03:04.125Z");
    assert_eq!(first_legacy.created_at, repeated.created_at);
    assert_eq!(fs::read(path).unwrap(), legacy);
}

#[test]
fn missing_or_private_sources_never_create_guessed_workflow_groups() {
    let mut graph = static_plan();
    graph["workflow"].as_object_mut().unwrap().remove("file");
    assert!(graph_workflow(&graph).is_none());
    for source in [
        "",
        "/private/sample.yaml",
        "C:\\private\\sample.yaml",
        "https://example.test/sample.yaml",
        "file:sample.yaml",
        "FILE:sample.yaml",
        "sample\n.yaml",
    ] {
        graph["workflow"]["file"] = json!(source);
        assert!(graph_workflow(&graph).is_none(), "{source:?}");
    }
    for source in [
        "sample.yaml",
        "example.py:build",
        "file.py:build",
        "../shared/workflow.yaml",
    ] {
        graph["workflow"]["file"] = json!(source);
        assert_eq!(graph_workflow(&graph).unwrap().source, source);
    }
}

#[test]
fn saved_plans_join_the_same_workflow_as_runs_and_concurrent_registration_is_stable() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    recorded_workflow(&fixture, "../shared/sample.py:build", None, None);
    let run = catalog.register_run(&fixture.root()).unwrap();
    let mut graph = static_plan();
    graph["workflow"]["id"] = json!("sample");
    graph["workflow"]["file"] = json!("../shared/sample.py:build");
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let catalog = catalog.clone();
            let graph = graph.clone();
            std::thread::spawn(move || catalog.register_plan(&graph).unwrap())
        })
        .collect();
    let plans: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    for plan in &plans {
        assert_eq!(plan.workflow, run.workflow);
        assert_eq!(plan.id, plans[0].id);
        assert_eq!(plan.created_at, plans[0].created_at);
        assert!(plan.name.is_none());
    }
    assert_eq!(catalog.entries().unwrap().len(), 2);
}

#[test]
fn malformed_run_metadata_falls_back_without_exposing_private_names() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    recorded_workflow(
        &fixture,
        "sample.yaml",
        Some("/private/invalid-name"),
        Some("not-a-date"),
    );
    let path = fixture.root().join("plan.json");
    let time = DateTime::parse_from_rfc3339("2026-10-01T02:03:04.500Z")
        .unwrap()
        .with_timezone(&Utc);
    File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time.into()))
        .unwrap();
    let run = catalog.register_run(&fixture.root()).unwrap();
    assert_eq!(run.created_at, "2026-10-01T02:03:04.500Z");
    assert!(run.name.is_none());
}

#[test]
fn invalid_recorded_run_names_are_omitted_from_catalog_entries() {
    let (_directory, catalog) = catalog();
    for name in [
        "".to_owned(),
        "two words".into(),
        "_leading".into(),
        "-leading".into(),
        "café".into(),
        "a".repeat(65),
        "name.dot".into(),
    ] {
        let fixture = Fixture::new("image/png");
        recorded_workflow(&fixture, "sample.yaml", Some(&name), None);
        let entry = catalog.register_run(&fixture.root()).unwrap();
        assert!(entry.name.is_none(), "{name:?}");
        assert!(json!(entry).get("name").is_none());
    }
    for name in ["A".repeat(64), "character_1-ready".into(), "0".into()] {
        let fixture = Fixture::new("image/png");
        recorded_workflow(&fixture, "sample.yaml", Some(&name), None);
        assert_eq!(
            catalog.register_run(&fixture.root()).unwrap().name,
            Some(name)
        );
    }
}

#[test]
fn file_uri_sources_are_omitted_without_rejecting_saved_plan_entries() {
    let (_directory, catalog) = catalog();
    for source in ["file:sample.yaml", "FILE:sample.yaml"] {
        let mut graph = static_plan();
        graph["workflow"]["file"] = json!(source);
        let entry = catalog.register_plan(&graph).unwrap();
        assert!(entry.workflow.is_none());
        assert!(json!(entry).get("workflow").is_none());
    }
}

#[test]
fn registration_survives_restart_and_resume_but_not_replacement() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    let entry = catalog.register_run(&fixture.root()).unwrap();
    let reopened = Catalog::open(&catalog.root, &catalog.runs_dir).unwrap();
    assert_eq!(entry.id, reopened.register_run(&fixture.root()).unwrap().id);
    assert_eq!(reopened.entries().unwrap().len(), 1);
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(fixture.root().join("events.jsonl"))
        .unwrap();
    writeln!(log,"{}",json!({"kind":"fx-run-events-v1","event":"run_started","plan":"a".repeat(64),"invocation_id":"resumed","offset_ms":0})).unwrap();
    assert_eq!(entry.id, reopened.register_run(&fixture.root()).unwrap().id);
    assert_eq!(reopened.entries().unwrap()[0].state, "unfinished");
    let path = fixture.root().join("events.jsonl");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("offline-fixture", "replacement");
    fs::write(path, text).unwrap();
    let replacement = reopened.register_run(&fixture.root()).unwrap();
    assert_ne!(entry.id, replacement.id);
    assert!(reopened.source(&entry.id, "runs").is_err());
    assert!(reopened.source(&replacement.id, "runs").is_ok());
    fs::remove_file(fixture.root().join("plan.json")).unwrap();
    assert!(
        reopened
            .entries()
            .unwrap()
            .iter()
            .all(|entry| entry.state == "unavailable")
    );
}

#[test]
fn plans_are_immutable_and_distinct_from_run_records() {
    let (_directory, catalog) = catalog();
    let plan = static_plan();
    let first = catalog.register_plan(&plan).unwrap();
    assert_eq!(first.id, catalog.register_plan(&plan).unwrap().id);
    let mut changed = plan.clone();
    changed["workflow"]["title"] = json!("Changed");
    assert_ne!(first.id, catalog.register_plan(&changed).unwrap().id);
    assert_eq!(catalog.entries().unwrap().len(), 2);
    assert!(catalog.source(&first.id, "runs").is_err());
    match catalog.source(&first.id, "plans").unwrap() {
        Source::Plan(saved) => assert_eq!(*saved, plan),
        _ => panic!("wrong source"),
    }
}

#[test]
fn discovers_only_configured_runs_and_skips_symlinks() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    let old = catalog.runs_dir.join("sample/old");
    fs::create_dir_all(&old).unwrap();
    for name in ["plan.json", "events.jsonl"] {
        fs::copy(fixture.root().join(name), old.join(name)).unwrap();
    }
    let unrelated = catalog.root.join("outside");
    fs::create_dir_all(&unrelated).unwrap();
    for name in ["plan.json", "events.jsonl"] {
        fs::copy(fixture.root().join(name), unrelated.join(name)).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&unrelated, catalog.runs_dir.join("linked")).unwrap();
    assert_eq!(catalog.entries().unwrap().len(), 1);
    catalog.register_run(&unrelated).unwrap();
    assert_eq!(catalog.entries().unwrap().len(), 2);
}

#[cfg(unix)]
#[test]
fn private_state_refuses_symlinks_and_uses_private_modes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_directory, catalog) = catalog();
    let entry = catalog.register_plan(&static_plan()).unwrap();
    assert_eq!(
        fs::metadata(catalog.state_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let ignore = catalog.state_dir().join(".gitignore");
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "*\n");
    assert_eq!(
        fs::metadata(&ignore).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let file = catalog
        .state_dir()
        .join(format!("entries/{}.json", entry.id));
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();
    let external = tempfile::NamedTempFile::new().unwrap();
    fs::write(external.path(), saved).unwrap();
    symlink(external.path(), &file).unwrap();
    assert!(catalog.register_plan(&static_plan()).is_err());
    assert!(catalog.source(&entry.id, "plans").is_err());
    fs::remove_file(&file).unwrap();
    fs::remove_dir(catalog.state_dir().join("entries")).unwrap();
    let other = tempfile::tempdir().unwrap();
    symlink(other.path(), catalog.state_dir().join("entries")).unwrap();
    assert!(Catalog::open(&catalog.root, &catalog.runs_dir).is_err());
}

#[tokio::test]
async fn nested_routes_scope_artifacts_and_reject_wrong_projects() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    let entry = catalog.register_run(&fixture.root()).unwrap();
    let plan = catalog.register_plan(&static_plan()).unwrap();
    let (app, _receiver) = app(catalog.clone());
    for endpoint in ["api/view", "api/run", "api/snapshot"] {
        let response = app
            .clone()
            .oneshot(
                request(&format!("{}{endpoint}", entry.url))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = value(response).await;
        let view = if endpoint == "api/snapshot" {
            &response["view"]
        } else {
            &response
        };
        let available = view["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["available"] == true)
            .unwrap();
        assert_eq!(
            available["url"],
            format!("{}api/artifacts/{}", entry.url, fixture.digest)
        );
    }
    let response = app
        .clone()
        .oneshot(
            request(&format!("{}api/artifacts/{}", entry.url, fixture.digest))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(&entry.url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let script = Assets::iter().find(|path| path.ends_with(".js")).unwrap();
    let response = app
        .clone()
        .oneshot(
            request(&format!("{}{script}", entry.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    for path in [
        entry.url.replace(catalog.project_id(), &"b".repeat(64)),
        format!("/p/{}/runs/{}/", catalog.project_id(), "c".repeat(64)),
        format!("{}api/artifacts/{}", plan.url, fixture.digest),
    ] {
        let response = app
            .clone()
            .oneshot(request(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = app
        .clone()
        .oneshot(
            request(&format!("{}api/events", entry.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            request(&format!("{}api/view", plan.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(value(response).await, static_plan());
}

fn save_attempt(root: &Path, fixture: &Fixture, run_id: &str, number: u64, succeeded: bool) {
    let events = fs::read_to_string(fixture.root().join("events.jsonl")).unwrap();
    let (initial, rest) = events.split_once('\n').unwrap();
    let mut first: Value = serde_json::from_str(initial).unwrap();
    first["run_id"] = json!(run_id);
    first["attempt_number"] = json!(number);
    fs::write(
        root.join("events.jsonl"),
        format!("{first}\n{rest}").replace(
            "\"ok\":false",
            if succeeded {
                "\"ok\":true"
            } else {
                "\"ok\":false"
            },
        ),
    )
    .unwrap();
    if root != fixture.root() {
        fs::copy(fixture.root().join("plan.json"), root.join("plan.json")).unwrap();
    }
}

#[tokio::test]
async fn run_links_resolve_latest_and_keep_exact_attempts_pinned() {
    let (_directory, catalog) = catalog();
    let first = Fixture::new("image/png");
    let run_id = "d".repeat(64);
    save_attempt(&first.root(), &first, &run_id, 1, true);
    let entry = catalog.register_run(&first.root()).unwrap();
    assert_eq!(entry.run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(entry.attempt_number, Some(1));
    let stable = catalog
        .registered_run_latest_route(&first.root())
        .unwrap()
        .unwrap();
    assert_eq!(stable, format!("/p/{}/run/{run_id}/", catalog.project_id()));
    assert_eq!(
        catalog.registered_run_route(&first.root()).unwrap(),
        Some(entry.url.clone())
    );
    let (before, _receiver) = app(catalog.clone());
    let response = before
        .oneshot(request(&stable).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::LOCATION], entry.url);

    // The next attempt used no viewer and lives outside the configured runs tree.
    let second = catalog.root.join("elsewhere/attempt-2");
    let second_allocation =
        attempts::Allocation::reserve(&catalog.root, &second, 2, 1, None).unwrap();
    let third = catalog.root.join("elsewhere/attempt-3");
    let third_allocation =
        attempts::Allocation::reserve(&catalog.root, &third, 3, 2, None).unwrap();
    let allocation_lock = run_index::allocate_attempt(&catalog.root).unwrap();
    attempts::save(
        &catalog.root,
        &run_id,
        &[second_allocation, third_allocation],
    )
    .unwrap();
    let failed = Fixture::new("image/png");
    save_attempt(&second, &failed, &run_id, 2, false);
    run_index::RunIndex {
        project_root: catalog.root.clone(),
    }
    .record(&second)
    .unwrap();
    attempts::confirm_start(
        &catalog.root,
        &attempts::Attempt {
            run_id: run_id.clone(),
            attempt_number: 2,
        },
        &second,
    )
    .unwrap();
    drop(allocation_lock);
    assert_eq!(catalog.registered_run_latest_route(&second).unwrap(), None);

    let reopened = Catalog::open(&catalog.root, &catalog.runs_dir).unwrap();
    let (restarted, _receiver) = app(reopened.clone());
    let response = restarted
        .clone()
        .oneshot(request(&stable).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let latest = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert_ne!(latest, entry.url);
    assert_eq!(
        reopened.registered_run_latest_route(&second).unwrap(),
        Some(stable.clone())
    );

    let history_path = format!("{}api/history", entry.url);
    let response = restarted
        .clone()
        .oneshot(request(&history_path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        value(response).await,
        json!({
            "kind": "fx-viewer-run-history-v1", "run_id": run_id, "selected_attempt": 1,
        "latest_url": stable, "latest_available": true, "attempts": [
                {"attempt_number": 2, "url": latest, "state": "failed"},
                {"attempt_number": 1, "url": entry.url, "state": "succeeded"}
            ]
        })
    );

    // A process can publish its start before confirming the allocation. The index
    // can recover such a start only while its on-disk reservation still matches.
    save_attempt(&third, &failed, &run_id, 3, false);
    run_index::RunIndex {
        project_root: catalog.root.clone(),
    }
    .record(&third)
    .unwrap();
    let marker_path = third.join("run.lock");
    let marker = fs::read(&marker_path).unwrap();
    fs::write(&marker_path, b"replaced reservation\n").unwrap();
    assert!(reopened.latest_exact_url(&run_id).is_err());
    fs::write(marker_path, marker).unwrap();
    fs::remove_file(third.join("events.jsonl")).unwrap();

    for path in [stable.as_str(), &history_path] {
        let post = request(path)
            .method(Method::POST)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            restarted.clone().oneshot(post).await.unwrap().status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    let wrong = stable.replace(catalog.project_id(), &"e".repeat(64));
    let response = restarted
        .clone()
        .oneshot(request(&wrong).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // A damaged accepted latest attempt does not send the visitor back to the first one.
    fs::write(second.join("events.jsonl"), b"corrupt\n").unwrap();
    let response = restarted
        .clone()
        .oneshot(request(&stable).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!response.headers().contains_key(header::LOCATION));
    let response = restarted
        .clone()
        .oneshot(request(&history_path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let retained_history = value(response).await;
    assert_eq!(retained_history["attempts"][0]["url"], entry.url);
    assert_eq!(retained_history["latest_available"], false);
    let response = restarted
        .oneshot(
            request(&format!("{}api/view", entry.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(value(response).await["state"], "succeeded");
}

fn viewer_action(path: &str, action: &str) -> axum::http::request::Builder {
    request(&format!("{path}api/control/{action}"))
        .method(Method::POST)
        .header(header::ORIGIN, "http://127.0.0.1:43123")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-fx-action", action)
}

#[tokio::test]
async fn viewer_starts_require_exact_project_run_and_browser_submission() {
    let (_directory, catalog) = catalog();
    let first = Fixture::new("image/png");
    let next = Fixture::new("image/png");
    let run_id = "a".repeat(64);
    let invocation = "0123456789abcdef";
    let next_invocation = "fedcba9876543210";
    save_attempt(&first.root(), &first, &run_id, 1, false);
    let log = first.root().join("events.jsonl");
    let events = fs::read_to_string(&log)
        .unwrap()
        .replace("offline-fixture", invocation);
    fs::write(
        log,
        events.replace("\"event\":\"run_finished\"", "\"event\":\"run_paused\""),
    )
    .unwrap();
    save_attempt(&next.root(), &next, &run_id, 2, false);
    let log = next.root().join("events.jsonl");
    let events = fs::read_to_string(&log)
        .unwrap()
        .replace("offline-fixture", next_invocation);
    fs::write(log, events).unwrap();
    let index = run_index::RunIndex {
        project_root: catalog.root.clone(),
    };
    index.record(&first.root()).unwrap();
    index.record(&next.root()).unwrap();
    let first_entry = catalog.register_run(&first.root()).unwrap();
    let plan = catalog.register_plan(&static_plan()).unwrap();
    let external = Fixture::new("image/png");
    let external = catalog.register_run(&external.root()).unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::clone(&received);
    let next_root = next.root();
    let launcher: RunLauncher = Arc::new(move |request| {
        let response = match request.request_id.as_str() {
            "needs-spending" => Err(LaunchRefused {
                code: "approval_required".into(),
                message: "Continuing this run needs current spending authorization.".into(),
                unknown: false,
            }),
            "unknown" => Err(LaunchRefused {
                code: "unknown".into(),
                message: "The detached start could not be confirmed.".into(),
                unknown: true,
            }),
            _ => Ok(LaunchAccepted {
                folder: match request.operation {
                    LaunchOperation::Resume => request.folder.clone(),
                    LaunchOperation::Rerun => next_root.clone(),
                },
                invocation_id: next_invocation.into(),
            }),
        };
        requests.lock().unwrap().push(request);
        response
    });
    let (app, _receiver) = app_with_launcher(catalog.clone(), Some(launcher));
    let response = app
        .clone()
        .oneshot(
            request(&format!("{}api/control", first_entry.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        value(response).await,
        json!({"kind":"fx-viewer-run-control-v1", "invocation_id":invocation,
        "attempt_number":1,"actions":{"pause":false,"resume":true,"rerun":true,"cancel":false}})
    );

    let body = json!({"invocation_id":invocation,"request_id":"retry-1"});
    let missing_origin = request(&format!("{}api/control/rerun", first_entry.url))
        .method(Method::POST)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-fx-action", "rerun")
        .body(Body::from(body.to_string()))
        .unwrap();
    for (request, status) in [
        (missing_origin, StatusCode::FORBIDDEN),
        (
            viewer_action(&first_entry.url, "rerun")
                .header(header::ORIGIN, "http://example.test")
                .body(Body::from(body.to_string()))
                .unwrap(),
            StatusCode::FORBIDDEN,
        ),
        (
            viewer_action(&first_entry.url, "rerun")
                .body(Body::from(json!({"invocation_id":invocation}).to_string()))
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            viewer_action(&first_entry.url, "rerun")
                .body(Body::from(
                    json!({"invocation_id":invocation,"request_id":"retry-1","folder":"other"})
                        .to_string(),
                ))
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            viewer_action(&external.url, "rerun")
                .body(Body::from(body.to_string()))
                .unwrap(),
            StatusCode::CONFLICT,
        ),
        (
            viewer_action(&plan.url, "rerun")
                .body(Body::from(body.to_string()))
                .unwrap(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
    }
    assert!(received.lock().unwrap().is_empty());

    let response = app
        .clone()
        .oneshot(
            viewer_action(&first_entry.url, "resume")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(value(response).await["url"], first_entry.url);
    let response = app
        .clone()
        .oneshot(
            viewer_action(&first_entry.url, "rerun")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let latest = catalog.registered_run_route(&next.root()).unwrap().unwrap();
    assert_eq!(value(response).await["url"], latest);
    for (key, status, command) in [
        (
            "needs-spending",
            StatusCode::CONFLICT,
            format!("grida fx rerun {run_id} --attempt 1"),
        ),
        (
            "unknown",
            StatusCode::SERVICE_UNAVAILABLE,
            format!("grida fx inspect {run_id} --history"),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                viewer_action(&first_entry.url, "rerun")
                    .body(Body::from(
                        json!({"invocation_id":invocation,"request_id":key}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let response = value(response).await;
        assert_eq!(response["cli_command"], command);
        assert!(
            !response
                .to_string()
                .contains(first.root().to_str().unwrap())
        );
    }
    let observed = received.lock().unwrap();
    assert_eq!(observed.len(), 4);
    assert_eq!(observed[0].operation, LaunchOperation::Resume);
    assert_eq!(observed[1].operation, LaunchOperation::Rerun);
    assert_eq!(observed[1].expected_invocation, invocation);
    assert_eq!(observed[1].source_attempt, 1);
    assert_eq!(observed[1].run_id, run_id);
    assert_eq!(observed[1].folder, first.root());
    assert_eq!(observed[1].project_root, catalog.root);
    assert_eq!(observed[1].request_id, "retry-1");
}

#[cfg(unix)]
#[tokio::test]
async fn viewer_pause_and_cancel_target_the_observed_invocation() {
    use grida_fx_runtime::{
        engine::Cancel, events::EventLog, folder::RunFolder, run_control::RunControl,
    };

    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    let folder = fixture.root();
    let invocation = "0123456789abcdef";
    let plan = "a".repeat(64);
    let events_path = folder.join("events.jsonl");
    let events = fs::read_to_string(&events_path).unwrap();
    let initial = events
        .lines()
        .next()
        .unwrap()
        .replace("offline-fixture", invocation);
    fs::write(&events_path, format!("{initial}\n")).unwrap();
    let lock = RunFolder::lock_existing(&folder, "test run").unwrap();
    let events = Arc::new(EventLog::open(&events_path, invocation, &plan).unwrap());
    let owner = control::ControlServer::start(
        &folder,
        &plan,
        invocation,
        RunControl::new(events, Cancel::new()),
        &tokio::runtime::Handle::current(),
    )
    .unwrap();
    let entry = catalog.register_run(&folder).unwrap();
    let (app, _receiver) = app(catalog);
    let response = app
        .clone()
        .oneshot(
            request(&format!("{}api/control", entry.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        value(response).await["actions"],
        json!({"pause":true,"cancel":true,"resume":false,"rerun":false})
    );
    let stale = || {
        viewer_action(&entry.url, "pause")
            .body(Body::from(
                json!({"invocation_id":"fedcba9876543210"}).to_string(),
            ))
            .unwrap()
    };
    let response = app.clone().oneshot(stale()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(value(response).await["code"], "invocation_mismatch");
    assert!(
        !fs::read_to_string(&events_path)
            .unwrap()
            .contains("pause_requested")
    );
    for action in ["pause", "pause", "cancel"] {
        let request = viewer_action(&entry.url, action)
            .body(Body::from(json!({"invocation_id":invocation}).to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(value(response).await["outcome"], "accepted");
    }
    // A different invocation's recorded acceptance cannot acknowledge this stale request.
    let response = app.oneshot(stale()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(value(response).await["code"], "invocation_mismatch");
    let events = grida_fx_runtime::events::read_events_tolerant(&events_path).unwrap();
    for action in ["pause_requested", "cancel_requested"] {
        let matching: Vec<_> = events
            .iter()
            .filter(|event| event["event"] == action)
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0]["source"], "viewer");
        assert_eq!(matching[0]["invocation_id"], invocation);
    }
    drop(owner);
    drop(lock);
}

#[tokio::test]
async fn public_index_has_no_private_paths_or_control_credentials() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    catalog.register_run(&fixture.root()).unwrap();
    let (app, _receiver) = app(catalog.clone());
    for path in ["/api/view", "/api/catalog"] {
        let response = app
            .clone()
            .oneshot(request(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = value(response).await;
        assert_eq!(response["kind"], "fx-service-index-v1");
        let serialized = response.to_string();
        for secret in [
            fixture.root().to_string_lossy().as_ref(),
            catalog.root.to_string_lossy().as_ref(),
            "private-test-token",
        ] {
            assert!(!serialized.contains(secret));
        }
        let entry = &response["entries"][0];
        assert_eq!(entry["title"], "Sample workflow");
        assert!(DateTime::parse_from_rfc3339(entry["created_at"].as_str().unwrap()).is_ok());
        assert!(entry.get("workflow").is_none());
        assert!(entry.get("name").is_none());
    }
}

#[tokio::test]
async fn viewer_configuration_uses_the_current_service_project_for_every_entry() {
    let (_directory, catalog) = catalog();
    let fixture = Fixture::new("image/png");
    fs::write(
        fixture.root().join("fx.yaml"),
        "fx: project/v1\nviewer: {display: {notes: false}}\n",
    )
    .unwrap();
    let run = catalog.register_run(&fixture.root()).unwrap();
    let plan = catalog.register_plan(&static_plan()).unwrap();
    let (app, _receiver) = app(catalog.clone());
    fs::write(
        catalog.root.join("fx.yaml"),
        "fx: project/v1\nviewer: {display: {workflow_outputs: false}}\n",
    )
    .unwrap();
    for path in [
        "/api/viewer_config".to_string(),
        format!("{}api/viewer_config", run.url),
        format!("{}api/viewer_config", plan.url),
    ] {
        let response = app
            .clone()
            .oneshot(request(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            value(response).await,
            json!({"display":{"workflow_inputs":true,"workflow_outputs":false,"notes":true}})
        );
    }
    fs::write(
        catalog.root.join("fx.yaml"),
        "fx: project/v1\nviewer: {display: {workflow_inputs: false, notes: false}}\n",
    )
    .unwrap();
    let response = app
        .oneshot(
            request(&format!("{}api/viewer_config", run.url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        value(response).await,
        json!({"display":{"workflow_inputs":false,"workflow_outputs":true,"notes":false}})
    );
}

#[tokio::test]
async fn lifecycle_requires_authentication_and_same_origin() {
    let (_directory, catalog) = catalog();
    let (app, mut receiver) = app(catalog);
    for path in ["/api/service", "/api/service/stop"] {
        let method = if path.ends_with("stop") {
            Method::POST
        } else {
            Method::GET
        };
        for token in [None, Some("Bearer wrong")] {
            let mut request = request(path).method(method.clone());
            if let Some(token) = token {
                request = request.header(header::AUTHORIZATION, token);
            }
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let request = request(path)
            .method(method)
            .header(header::AUTHORIZATION, "Bearer private-test-token")
            .header(header::ORIGIN, "https://unrelated.invalid")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let response = app
        .clone()
        .oneshot(
            request("/api/service")
                .header(header::AUTHORIZATION, "Bearer private-test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let response = value(response).await;
    assert_eq!(response["instance_id"], "instance-test");
    assert_eq!(response["run_history"], true);
    assert!(!response.to_string().contains("private-test-token"));
    let response = app
        .oneshot(
            request("/api/service/stop")
                .method(Method::POST)
                .header(header::AUTHORIZATION, "Bearer private-test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    receiver.await.unwrap();
}

#[cfg(unix)]
#[test]
fn service_exclusion_file_refuses_symlinks_or_changed_rules() {
    use std::os::unix::fs::symlink;
    let (_directory, catalog) = catalog();
    let ignore = catalog.state_dir().join(".gitignore");
    fs::write(&ignore, "instance.json\n").unwrap();
    assert!(Catalog::open(&catalog.root, &catalog.runs_dir).is_err());
    fs::remove_file(&ignore).unwrap();
    let other = tempfile::NamedTempFile::new().unwrap();
    fs::write(other.path(), "*\n").unwrap();
    symlink(other.path(), &ignore).unwrap();
    assert!(Catalog::open(&catalog.root, &catalog.runs_dir).is_err());
}

#[tokio::test]
async fn entry_pages_redirect_to_a_trailing_slash_for_relative_assets() {
    let (_directory, catalog) = catalog();
    let plan = catalog.register_plan(&static_plan()).unwrap();
    let (app, _receiver) = app(catalog);
    let request = request(&format!(
        "{}?selected=example",
        plan.url.trim_end_matches('/')
    ))
    .body(Body::empty())
    .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("{}?selected=example", plan.url)
    );
}
