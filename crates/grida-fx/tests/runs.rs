//! `grida-fx runs list` and `grida-fx runs remove` (spec/store.md §9), in temporary projects.
//!
//! Most runs here are written by hand, so no test needs Python; the tests that run a real
//! workflow skip without a Python that has the `grida` package (`GRIDA_FX_PYTHON`, else
//! `python/.venv/bin/python`).

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn grida_fx(cwd: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command.args(args).current_dir(cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    if let Some(python) = python() {
        command.env("GRIDA_FX_PYTHON", python);
    }
    command
        .env("HOME", cwd)
        .env("GRIDA_FX_NETWORK", "off")
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap()
}

fn python() -> Option<PathBuf> {
    std::env::var_os("GRIDA_FX_PYTHON")
        .map(PathBuf::from)
        .or_else(|| {
            let candidate = repository().join("python/.venv/bin/python");
            candidate.is_file().then_some(candidate)
        })
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn status(output: &Output) -> i32 {
    output.status.code().expect("an exit status")
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}{}", text(&output.stdout), text(&output.stderr)))
}

/// Checks a document against a schema of spec/schemas.
fn conforms(document: &Value, schema: &str) {
    let path = repository().join(format!("spec/schemas/{schema}.schema.json"));
    let schema: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let problems: Vec<String> = validator
        .iter_errors(document)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    assert!(problems.is_empty(), "{problems:#?}\n{document:#}");
}

/// A project with `fx.yaml`, at a canonical path.
struct Project {
    _folder: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new() -> Project {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        Project {
            _folder: folder,
            root,
        }
    }

    fn fx(&self, args: &[&str]) -> Output {
        grida_fx(&self.root, args)
    }

    /// A run of `id` written by hand at `relative`.
    fn run_at(&self, relative: &str, id: &str, made: &Made) -> PathBuf {
        let folder = self.root.join(relative);
        fs::create_dir_all(&folder).unwrap();
        let mut plan = json!({
            "kind": "fx-graph-v1",
            "workflow": {"id": id, "file": format!("workflows/{id}.yaml")},
            "takes_file": format!("workflows/{id}.takes.yaml"),
        });
        if made.stand_in {
            plan["stand_in"] = json!(true);
        }
        fs::write(folder.join("plan.json"), plan.to_string()).unwrap();
        let mut lines = vec![json!({
            "kind": "fx-run-events-v1", "event": "run_started",
            "created_at": made.created_at, "name": made.name,
        })];
        if let Some(picked) = &made.picked {
            lines.push(
                json!({"event": "node_finished", "id": "draw#2", "path": "draw",
                "outputs": {"image": {"file": {"digest": picked, "kind": "image/png",
                "name": "draw/image", "size": 3}}}}),
            );
        }
        match made.ended {
            Some(Ended::Ok) => lines.push(json!({"event": "run_finished", "ok": true,
                "incomplete": false, "charged_usd": 0.5})),
            Some(Ended::Failed) => lines.push(json!({"event": "run_finished", "ok": false,
                "incomplete": false, "charged_usd": 0.25})),
            Some(Ended::Incomplete) => lines.push(json!({"event": "run_finished", "ok": false,
                "incomplete": true, "stopped": "phase 2 is past --yes-up-to", "charged_usd": 0})),
            None => {}
        }
        let log: String = lines.iter().map(|line| format!("{line}\n")).collect();
        fs::write(folder.join("events.jsonl"), log).unwrap();
        fs::write(folder.join("run.lock"), "").unwrap();
        folder
    }

    /// `<runs>/<id>/named-<d>/<name>-<d>` for a run of `workflows/<id>.yaml`.
    fn named(&self, id: &str, name: &str) -> String {
        self.named_of(id, &format!("workflows/{id}.yaml"), name)
    }

    fn named_of(&self, id: &str, source: &str, name: &str) -> String {
        format!(
            "runs/{id}/named-{}/{name}-{}",
            sha256(source.as_bytes()),
            sha256(name.as_bytes())
        )
    }

    fn list(&self) -> Value {
        let output = self.fx(&["runs", "list", "--json"]);
        assert_eq!(status(&output), 0, "{}", text(&output.stderr));
        let document = json_of(&output);
        conforms(&document, "fx-run-list-v1");
        document
    }

    fn row(&self, folder: &str) -> Value {
        self.list()["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["folder"] == folder)
            .cloned()
            .unwrap_or_else(|| panic!("no run {folder}"))
    }
}

enum Ended {
    Ok,
    Failed,
    Incomplete,
}

struct Made {
    created_at: &'static str,
    name: Option<&'static str>,
    ended: Option<Ended>,
    stand_in: bool,
    picked: Option<String>,
}

fn made(created_at: &'static str, ended: Option<Ended>) -> Made {
    Made {
        created_at,
        name: None,
        ended,
        stand_in: false,
        picked: None,
    }
}

fn sha256(bytes: &[u8]) -> String {
    grida_fx_core::value::sha256_hex(bytes)
}

#[test]
fn listing_finds_every_placement_and_writes_nothing() {
    let project = Project::new();
    project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", Some(Ended::Ok)),
    );
    project.run_at(
        "runs/case/2026-10-02-1",
        "case",
        &made("2026-10-02T00:00:00.000Z", Some(Ended::Failed)),
    );
    project.run_at(
        "runs/case/2026-10-03-1",
        "case",
        &made("2026-10-03T00:00:00.000Z", Some(Ended::Incomplete)),
    );
    let named = project.named("case", "baseline");
    project.run_at(
        &named,
        "case",
        &Made {
            name: Some("baseline"),
            ..made("2026-10-04T00:00:00.000Z", Some(Ended::Ok))
        },
    );
    // A `--run runs/case` folder, with a dated run of its own workflow inside it.
    project.run_at("runs/case", "case", &made("2026-10-05T00:00:00.000Z", None));
    project.run_at(
        "runs/case/2026-10-06-1",
        "other",
        &made("2026-10-06T00:00:00.000Z", None),
    );
    fs::create_dir_all(project.root.join("runs/case/2026-10-07-1")).unwrap();
    fs::create_dir_all(
        project
            .root
            .join("runs/case/.removing-0123456789abcdef/files"),
    )
    .unwrap();
    fs::create_dir_all(project.root.join("runs/legacy")).unwrap();
    fs::write(
        project.root.join("runs/legacy/plan.json"),
        r#"{"kind": "graph/v2"}"#,
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&project.root, project.root.join("runs/loop")).unwrap();

    let document = project.list();
    assert!(!project.root.join(".fx").exists(), "listing created .fx");
    let rows: Vec<(String, String, String)> = document["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            (
                run["folder"].as_str().unwrap().to_string(),
                run["placement"].as_str().unwrap().to_string(),
                run["state"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let row = |folder: &str| {
        rows.iter()
            .find(|(f, _, _)| f == folder)
            .map(|(_, p, s)| (p.as_str(), s.as_str()))
            .unwrap_or_else(|| panic!("{folder} not in {rows:#?}"))
    };
    assert_eq!(row("runs/case/2026-10-01-1"), ("allocated", "succeeded"));
    assert_eq!(row("runs/case/2026-10-02-1"), ("allocated", "failed"));
    assert_eq!(row("runs/case/2026-10-03-1"), ("allocated", "incomplete"));
    assert_eq!(row(&named), ("named", "succeeded"));
    assert_eq!(row("runs/case"), ("explicit", "unfinished"));
    // Its position says `case`, its record `other`.
    assert_eq!(row("runs/case/2026-10-06-1"), ("explicit", "unfinished"));
    assert_eq!(row("runs/case/2026-10-07-1"), ("allocated", "empty"));
    assert_eq!(
        row("runs/case/.removing-0123456789abcdef"),
        ("allocated", "removing")
    );
    assert_eq!(rows[0].0, "runs/case/2026-10-06-1", "newest first");
    assert_eq!(
        project.row("runs/case/2026-10-03-1")["stopped"],
        "phase 2 is past --yes-up-to"
    );
    let skipped: Vec<(&str, &str)> = document["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["folder"].as_str().unwrap(), s["code"].as_str().unwrap()))
        .collect();
    assert!(
        skipped.contains(&("runs/legacy", "foreign_run")),
        "{skipped:?}"
    );
    #[cfg(unix)]
    assert!(
        skipped.contains(&("runs/loop", "not_a_folder")),
        "{skipped:?}"
    );

    let output = project.fx(&["runs", "list"]);
    let lines = text(&output.stdout);
    assert!(
        lines.contains("succeeded   case/baseline  $0.50"),
        "{lines}"
    );
    assert!(
        text(&output.stderr).contains("skipped   runs/legacy"),
        "{}",
        text(&output.stderr)
    );
    let only = project.fx(&["runs", "list", "--workflow", "other", "--json"]);
    assert_eq!(json_of(&only)["runs"].as_array().unwrap().len(), 1);
}

#[test]
fn what_is_not_a_run_is_never_removed() {
    let project = Project::new();
    project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", None),
    );
    project.run_at(
        "runs/holder",
        "case",
        &made("2026-10-01T00:00:00.000Z", None),
    );
    project.run_at(
        "runs/holder/2026-10-02-1",
        "x",
        &made("2026-10-02T00:00:00.000Z", None),
    );
    fs::create_dir_all(project.root.join("notes")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        project.root.join("runs/case/2026-10-01-1"),
        project.root.join("alias"),
    )
    .unwrap();
    let mut refused = vec![
        (".", "not_a_run"),
        ("runs", "not_a_run"),
        ("runs/case", "not_a_run"),
        ("runs/holder", "not_a_run"),
        ("notes", "not_a_run"),
        ("case", "invalid_target"),
        ("runs/case/2026-10-09-1", "invalid_target"),
        ("case/nobody", "invalid_target"),
    ];
    #[cfg(unix)]
    refused.push(("alias", "not_a_run"));
    for (run, code) in refused {
        for confirmed in [false, true] {
            let mut argv = vec!["runs", "remove", "--json", run, "runs/case/2026-10-01-1"];
            if confirmed {
                argv.push("--yes");
            }
            let output = project.fx(&argv);
            assert_eq!(status(&output), 2, "{run}: {}", text(&output.stdout));
            let document = json_of(&output);
            conforms(&document, "fx-run-removal-v1");
            assert_eq!(document["error"]["code"], code, "{run}");
            assert_eq!(document["applied"], false);
        }
    }
    assert!(
        project
            .root
            .join("runs/case/2026-10-01-1/plan.json")
            .is_file()
    );
    assert!(
        project
            .root
            .join("runs/holder/2026-10-02-1/plan.json")
            .is_file()
    );
    let output = project.fx(&["runs", "remove"]);
    assert_eq!(status(&output), 2);
    let output = project.fx(&["runs", "remove", "x", "--state", "failed"]);
    assert_eq!(status(&output), 2, "runs and filters together");
    let output = project.fx(&["runs", "remove", "--before", "yesterday"]);
    assert_eq!(status(&output), 2);
}

#[test]
fn a_removal_is_previewed_then_done_and_leaves_nothing_behind() {
    let project = Project::new();
    let first = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", Some(Ended::Ok)),
    );
    fs::create_dir_all(first.join("files/draw")).unwrap();
    // One file linked from a store elsewhere, one only here, in a read-only folder.
    let store = project.root.join(".fx/cache/files/ab");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("shared"), vec![7u8; 4000]).unwrap();
    fs::hard_link(store.join("shared"), first.join("files/draw/image.png")).unwrap();
    fs::write(first.join("files/draw/notes.txt"), vec![1u8; 100]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(first.join("files/draw"), fs::Permissions::from_mode(0o555)).unwrap();
    }
    let preview = project.fx(&["runs", "remove", "runs/case/2026-10-01-1", "--json"]);
    assert_eq!(status(&preview), 0, "{}", text(&preview.stderr));
    let document = json_of(&preview);
    conforms(&document, "fx-run-removal-v1");
    assert_eq!(document["applied"], false);
    assert_eq!(document["runs"][0]["outcome"], "would_remove");
    assert!(
        first.join("plan.json").is_file(),
        "a preview removes nothing"
    );
    #[cfg(unix)]
    {
        assert_eq!(document["cache_bytes"], 4000);
        assert!(document["freed_bytes"].as_u64().unwrap() > 100);
    }

    let done = project.fx(&[
        "runs",
        "remove",
        "runs/case/2026-10-01-1",
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&done), 0, "{}", text(&done.stdout));
    let document = json_of(&done);
    conforms(&document, "fx-run-removal-v1");
    assert_eq!(document["applied"], true);
    assert_eq!(document["runs"][0]["outcome"], "removed");
    assert!(!first.exists());
    let leftovers: Vec<_> = fs::read_dir(project.root.join("runs/case"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert_eq!(
        fs::read(store.join("shared")).unwrap().len(),
        4000,
        "the store keeps its copy"
    );

    let text_output = project.fx(&["runs", "remove", "--workflow", "case"]);
    assert_eq!(status(&text_output), 0);
    assert_eq!(text(&text_output.stdout), "nothing to remove\n");
}

#[test]
fn an_active_run_is_refused_and_kept() {
    let project = Project::new();
    let folder = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", None),
    );
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(folder.join("run.lock"))
        .unwrap();
    held.try_lock().unwrap();
    for confirmed in [false, true] {
        let mut argv = vec!["runs", "remove", "runs/case/2026-10-01-1", "--json"];
        if confirmed {
            argv.push("--yes");
        }
        let output = project.fx(&argv);
        assert_eq!(status(&output), 1, "{}", text(&output.stdout));
        let document = json_of(&output);
        conforms(&document, "fx-run-removal-v1");
        assert_eq!(document["runs"][0]["outcome"], "refused");
        assert_eq!(document["runs"][0]["code"], "active");
    }
    assert!(folder.join("plan.json").is_file());
    drop(held);
    let output = project.fx(&["runs", "remove", "runs/case/2026-10-01-1", "--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(!folder.exists());
}

#[test]
fn filters_take_dated_runs_only_and_say_what_they_passed_over() {
    let project = Project::new();
    let failed = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", Some(Ended::Failed)),
    );
    let incomplete = project.run_at(
        "runs/case/2026-10-01-2",
        "case",
        &made("2026-10-01T01:00:00.000Z", Some(Ended::Incomplete)),
    );
    let newer = project.run_at(
        "runs/case/2026-10-05-1",
        "case",
        &made("2026-10-05T00:00:00.000Z", Some(Ended::Failed)),
    );
    let named = project.named("case", "keep");
    project.run_at(
        &named,
        "case",
        &Made {
            name: Some("keep"),
            ..made("2026-10-01T00:00:00.000Z", Some(Ended::Failed))
        },
    );
    project.run_at(
        "runs/mine",
        "case",
        &made("2026-10-01T00:00:00.000Z", Some(Ended::Failed)),
    );
    // A claim made just now: a run may be starting in it.
    fs::create_dir_all(project.root.join("runs/case/2026-10-09-1")).unwrap();

    let output = project.fx(&[
        "runs",
        "remove",
        "--state",
        "failed",
        "--state",
        "empty",
        "--before",
        "2026-10-03",
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    let document = json_of(&output);
    conforms(&document, "fx-run-removal-v1");
    let removed: Vec<&str> = document["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["folder"].as_str().unwrap())
        .collect();
    assert_eq!(removed, ["runs/case/2026-10-01-1"]);
    assert_eq!(document["skipped"], json!({"named": 1, "explicit": 1}));
    assert!(!failed.exists());
    assert!(incomplete.exists() && newer.exists());
    assert!(project.root.join(&named).exists() && project.root.join("runs/mine").exists());

    let output = project.fx(&["runs", "remove", "--state", "empty", "--json"]);
    assert_eq!(json_of(&output)["skipped"], json!({"young": 1}));
    let output = project.fx(&["runs", "remove", "--state", "empty"]);
    assert!(
        text(&output.stdout).contains("passed    1 just claimed"),
        "{}",
        text(&output.stdout)
    );

    // Named and placed runs go when they are named.
    let output = project.fx(&["runs", "remove", "case/keep", "runs/mine", "--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(!project.root.join(&named).exists() && !project.root.join("runs/mine").exists());
}

#[test]
fn an_empty_named_claim_is_found_by_its_name() {
    let project = Project::new();
    let claim = project.root.join(project.named("case", "draft"));
    fs::create_dir_all(&claim).unwrap();
    assert_eq!(
        project.row(&project.named("case", "draft"))["state"],
        "empty"
    );
    let output = project.fx(&["runs", "remove", "case/draft", "--yes", "--json"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert_eq!(json_of(&output)["runs"][0]["outcome"], "removed");
    assert!(!claim.exists());
}

#[test]
fn the_last_run_holding_a_pick_stays() {
    let project = Project::new();
    let picked = sha256(b"take two");
    fs::create_dir_all(project.root.join("workflows")).unwrap();
    fs::write(
        project.root.join("workflows/case.takes.yaml"),
        format!("draw:\n  take: 2\n  result: \"{picked}\"\n"),
    )
    .unwrap();
    let older = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &Made {
            picked: Some(picked.clone()),
            ..made("2026-10-01T00:00:00.000Z", Some(Ended::Ok))
        },
    );
    let newer = project.run_at(
        "runs/case/2026-10-02-1",
        "case",
        &Made {
            picked: Some(picked.clone()),
            ..made("2026-10-02T00:00:00.000Z", Some(Ended::Ok))
        },
    );
    let output = project.fx(&["runs", "remove", "--workflow", "case", "--yes", "--json"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    let document = json_of(&output);
    assert_eq!(document["skipped"], json!({"holds_pick": 1}));
    assert!(!older.exists() && newer.exists(), "the newest holder stays");

    let output = project.fx(&[
        "runs",
        "remove",
        "runs/case/2026-10-02-1",
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&output), 1);
    let document = json_of(&output);
    conforms(&document, "fx-run-removal-v1");
    assert_eq!(document["runs"][0]["code"], "holds_pick");
    assert!(newer.exists());

    // Picking another take releases it.
    fs::write(
        project.root.join("workflows/case.takes.yaml"),
        "draw:\n  take: 3\n",
    )
    .unwrap();
    let output = project.fx(&["runs", "remove", "runs/case/2026-10-02-1", "--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(!newer.exists());
}

#[test]
fn a_stand_in_run_holds_no_pick() {
    let project = Project::new();
    let picked = sha256(b"take two");
    fs::create_dir_all(project.root.join("workflows")).unwrap();
    fs::write(
        project.root.join("workflows/case.takes.yaml"),
        format!("draw:\n  take: 2\n  result: \"{picked}\"\n"),
    )
    .unwrap();
    let folder = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &Made {
            picked: Some(picked),
            stand_in: true,
            ..made("2026-10-01T00:00:00.000Z", Some(Ended::Ok))
        },
    );
    assert_eq!(project.row("runs/case/2026-10-01-1")["stand_in"], true);
    let output = project.fx(&["runs", "remove", "runs/case/2026-10-01-1", "--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(!folder.exists());
}

#[test]
fn runs_outside_the_tree_come_from_the_index_and_are_forgotten_with_their_folder() {
    let project = Project::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().canonicalize().unwrap().join("run");
    let index = grida_fx_runtime::run_index::RunIndex {
        project_root: project.root.clone(),
    };
    fs::create_dir_all(&outside).unwrap();
    index.record(&outside).unwrap();
    // Written by hand, as the runner would.
    fs::write(
        outside.join("plan.json"),
        json!({"kind": "fx-graph-v1", "workflow": {"id": "case", "file": "workflows/case.yaml"}})
            .to_string(),
    )
    .unwrap();
    fs::write(
        outside.join("events.jsonl"),
        format!(
            "{}\n",
            json!({"event": "run_started", "created_at": "2026-10-01T00:00:00.000Z"})
        ),
    )
    .unwrap();
    let gone = elsewhere.path().canonicalize().unwrap().join("gone");
    fs::create_dir_all(&gone).unwrap();
    index.record(&gone).unwrap();
    fs::remove_dir(&gone).unwrap();

    let document = project.list();
    let placements: Vec<(&str, &str)> = document["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            (
                run["placement"].as_str().unwrap(),
                run["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(placements.len(), 2, "{document:#}");
    assert!(placements.contains(&("external", "unfinished")));
    assert!(placements.contains(&("external", "missing")));
    // Filters never reach outside the tree.
    let output = project.fx(&["runs", "remove", "--workflow", "case", "--json"]);
    assert_eq!(json_of(&output)["skipped"], json!({"external": 1}));

    let shown = |path: &Path| path.to_string_lossy().into_owned();
    let output = project.fx(&[
        "runs",
        "remove",
        &shown(&outside),
        &shown(&gone),
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    let document = json_of(&output);
    conforms(&document, "fx-run-removal-v1");
    let outcomes: Vec<&str> = document["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["removed", "forgotten"]);
    assert!(!outside.exists());
    assert!(
        grida_fx_runtime::run_index::entries(&project.root)
            .unwrap()
            .is_empty()
    );
    assert!(project.list()["runs"].as_array().unwrap().is_empty());
}

#[test]
fn removals_wait_for_each_other() {
    let project = Project::new();
    let folder = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", Some(Ended::Ok)),
    );
    fs::create_dir_all(project.root.join(".fx/runs")).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(project.root.join(".fx/runs/lock"))
        .unwrap();
    held.try_lock().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    let waiting = command
        .args(["runs", "remove", "runs/case/2026-10-01-1", "--yes"])
        .current_dir(&project.root)
        .env_clear()
        .env("HOME", &project.root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    assert!(
        folder.exists(),
        "nothing is removed while another removal holds the lock"
    );
    drop(held);
    let output = waiting.wait_with_output().unwrap();
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(
        text(&output.stderr).contains("waiting for another grida-fx runs remove to finish"),
        "{}",
        text(&output.stderr)
    );
    assert!(!folder.exists());
}

#[test]
fn a_run_whose_takes_file_cannot_be_read_stays() {
    let project = Project::new();
    fs::create_dir_all(project.root.join("workflows")).unwrap();
    fs::write(
        project.root.join("workflows/case.takes.yaml"),
        "draw: [unclosed\n",
    )
    .unwrap();
    let folder = project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", Some(Ended::Ok)),
    );
    let output = project.fx(&["runs", "remove", "--workflow", "case", "--yes", "--json"]);
    assert_eq!(json_of(&output)["skipped"], json!({"holds_pick": 1}));
    let output = project.fx(&[
        "runs",
        "remove",
        "runs/case/2026-10-01-1",
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&output), 1);
    let document = json_of(&output);
    conforms(&document, "fx-run-removal-v1");
    assert_eq!(document["runs"][0]["code"], "holds_pick");
    assert!(folder.exists());
}

#[test]
fn a_folder_with_files_but_no_plan_is_never_a_claim() {
    let project = Project::new();
    let folder = project.root.join("runs/case/2026-10-01-1");
    fs::create_dir_all(folder.join("files/draw")).unwrap();
    fs::write(folder.join("events.jsonl"), "{}\n").unwrap();
    let document = project.list();
    assert!(document["runs"].as_array().unwrap().is_empty());
    assert_eq!(document["skipped"][0]["code"], "no_plan");
    let output = project.fx(&["runs", "remove", "--state", "empty", "--yes", "--json"]);
    assert!(json_of(&output)["runs"].as_array().unwrap().is_empty());
    let output = project.fx(&[
        "runs",
        "remove",
        "runs/case/2026-10-01-1",
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&output), 2);
    assert!(folder.join("events.jsonl").exists());
}

#[test]
fn a_folder_holding_the_users_files_is_never_emptied() {
    let project = Project::new();
    let folder = project.run_at("renders", "case", &made("2026-10-01T00:00:00.000Z", None));
    // Run outside the tree, as the runner records it.
    grida_fx_runtime::run_index::RunIndex {
        project_root: project.root.clone(),
    }
    .record(&folder)
    .unwrap();
    fs::write(folder.join("holiday.png"), "mine").unwrap();
    for confirmed in [false, true] {
        let mut argv = vec!["runs", "remove", "renders", "--json"];
        if confirmed {
            argv.push("--yes");
        }
        let output = project.fx(&argv);
        assert_eq!(status(&output), 1, "{}", text(&output.stdout));
        let document = json_of(&output);
        conforms(&document, "fx-run-removal-v1");
        assert_eq!(document["runs"][0]["code"], "not_removable");
        assert_eq!(document["freed_bytes"], 0);
    }
    assert!(folder.join("holiday.png").exists() && folder.join("plan.json").exists());
    fs::remove_file(folder.join("holiday.png")).unwrap();
    // Folders holding no file are not the user's.
    fs::create_dir_all(folder.join("named-x/empty")).unwrap();
    let output = project.fx(&["runs", "remove", "renders", "--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(!folder.exists());
}

#[test]
fn a_deleted_runs_folder_leaves_no_missing_runs() {
    let project = Project::new();
    let index = grida_fx_runtime::run_index::RunIndex {
        project_root: project.root.clone(),
    };
    for n in 1..=3 {
        let folder = project.run_at(
            &format!("runs/case/2026-10-0{n}-1"),
            "case",
            &made("2026-10-01T00:00:00.000Z", None),
        );
        index.record(&folder).unwrap();
    }
    fs::remove_dir_all(project.root.join("runs")).unwrap();
    let document = project.list();
    assert!(
        document["runs"].as_array().unwrap().is_empty(),
        "{document:#}"
    );
    assert!(
        document["skipped"].as_array().unwrap().is_empty(),
        "{document:#}"
    );
    // A stale entry inside the tree can still be forgotten by its folder.
    let output = project.fx(&[
        "runs",
        "remove",
        "runs/case/2026-10-01-1",
        "--yes",
        "--json",
    ]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert_eq!(json_of(&output)["runs"][0]["outcome"], "forgotten");
    assert_eq!(
        grida_fx_runtime::run_index::entries(&project.root)
            .unwrap()
            .len(),
        2
    );
}

#[cfg(unix)]
#[test]
fn a_linked_fx_folder_is_read_like_a_real_one() {
    let project = Project::new();
    let state = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(state.path(), project.root.join(".fx")).unwrap();
    project.run_at(
        "runs/case/2026-10-01-1",
        "case",
        &made("2026-10-01T00:00:00.000Z", None),
    );
    let document = project.list();
    assert!(
        document["skipped"].as_array().unwrap().is_empty(),
        "{document:#}"
    );
    assert_eq!(document["runs"].as_array().unwrap().len(), 1);
}

/// A project whose workflow runs one Python node: `None` without a Python that has `grida`.
fn python_project() -> Option<Project> {
    python()?;
    let project = Project::new();
    fs::write(
        project.root.join("workflow.yaml"),
        "fx: workflow/v1\nid: greeting\ntitle: Greeting\nsteps:\n  greet:\n    uses: \
         ./node.py#greet\noutputs:\n  text: ${{ steps.greet.outputs.text }}\n",
    )
    .unwrap();
    fs::write(
        project.root.join("node.py"),
        "from grida.fx import Ctx, node\n\n\n@node(\"runs_test_greet\", outputs={\"text\": \
         \"text\"}, version=1)\ndef greet(ctx: Ctx) -> dict:\n    return {\"text\": \
         ctx.out.text(\"Hello\")}\n",
    )
    .unwrap();
    Some(project)
}

#[test]
fn integrated_a_real_run_is_indexed_outside_the_tree_removed_and_never_resumed() {
    let Some(project) = python_project() else {
        return;
    };
    let run = |args: &[&str]| {
        let mut all = vec!["run", "workflow.yaml", "--no-view", "--max-usd", "0"];
        all.extend_from_slice(args);
        project.fx(&all)
    };
    let output = run(&["--run", "elsewhere/one"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let outside = project.root.join("elsewhere/one");
    assert_eq!(
        grida_fx_runtime::run_index::entries(&project.root).unwrap(),
        vec![outside.clone()]
    );
    let output = run(&["--name", "baseline"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(
        grida_fx_runtime::run_index::entries(&project.root)
            .unwrap()
            .len(),
        2,
        "a run in the tree is indexed too"
    );
    let row = project.row("elsewhere/one");
    assert_eq!(row["placement"], "external");
    assert_eq!(row["state"], "succeeded");

    let baseline = project
        .root
        .join(project.named_of("greeting", "workflow.yaml", "baseline"));
    let catalog =
        grida_fx_viewer::service::Catalog::open(&project.root, &project.root.join("runs")).unwrap();
    catalog.register_run(&baseline).unwrap();
    let registered = || {
        grida_fx_viewer::service::registered_runs(&project.root)
            .unwrap()
            .0
    };
    assert_eq!(registered().len(), 1);

    let output = project.fx(&["runs", "remove", "greeting/baseline", "--json"]);
    let document = json_of(&output);
    // The placed text is a link to the store's copy.
    #[cfg(unix)]
    assert!(
        document["cache_bytes"].as_u64().unwrap() > 0,
        "{document:#}"
    );
    let output = project.fx(&[
        "runs",
        "remove",
        "greeting/baseline",
        "elsewhere/one",
        "--yes",
    ]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(
        grida_fx_runtime::run_index::entries(&project.root)
            .unwrap()
            .is_empty()
    );
    assert!(registered().is_empty(), "the catalog forgets a removed run");
    assert!(!baseline.exists());

    // A run in the tree is indexed too, so a changed runs setting keeps it.
    let output = run(&["--run", "runs/kept"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    fs::write(
        project.root.join("fx.yaml"),
        "fx: project/v1\nruns: history\n",
    )
    .unwrap();
    let row = project.row("runs/kept");
    assert_eq!(row["placement"], "external");
    fs::write(project.root.join("fx.yaml"), "fx: project/v1\n").unwrap();

    let output = run(&["--resume", "baseline"]);
    assert_eq!(
        status(&output),
        2,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert!(text(&output.stderr).contains("no named run greeting/baseline"));
    // The cache still answers a new run.
    let output = run(&["--name", "baseline"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}
