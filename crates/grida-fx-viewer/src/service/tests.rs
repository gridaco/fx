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
    let (sender, receiver) = oneshot::channel();
    (
        service_router(ServiceState {
            catalog,
            authority: "127.0.0.1:43123".into(),
            url: "http://127.0.0.1:43123/".into(),
            instance_id: "instance-test".into(),
            control_token: "private-test-token".into(),
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
