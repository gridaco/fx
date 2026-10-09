//! `grida-fx cache prune` (spec/store.md §9, "Pruning a store"), over stores and runs written by
//! hand; the tests that run a real workflow skip without a Python that has the `grida` package
//! (`GRIDA_FX_PYTHON`, else `python/.venv/bin/python`).

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

fn python() -> Option<PathBuf> {
    std::env::var_os("GRIDA_FX_PYTHON")
        .map(PathBuf::from)
        .or_else(|| {
            let candidate = repository().join("python/.venv/bin/python");
            candidate.is_file().then_some(candidate)
        })
}

fn command(cwd: &Path, args: &[&str]) -> Command {
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
        .env("PYTHONDONTWRITEBYTECODE", "1");
    command
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

fn conforms(document: &Value) {
    let path = repository().join("spec/schemas/fx-cache-prune-v1.schema.json");
    let schema: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let problems: Vec<String> = validator
        .iter_errors(document)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    assert!(problems.is_empty(), "{problems:#?}\n{document:#}");
}

fn sha256(bytes: &[u8]) -> String {
    grida_fx_core::value::sha256_hex(bytes)
}

/// A project at a canonical path whose store and runs are written by hand.
struct Project {
    _folder: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new() -> Project {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        let project = Project {
            _folder: folder,
            root,
        };
        project.mark_user(&project.root);
        project
    }

    fn store(&self) -> PathBuf {
        self.root.join(".fx/cache")
    }

    fn prune(&self, args: &[&str]) -> Output {
        let mut all = vec!["cache", "prune", "--json"];
        all.extend_from_slice(args);
        command(&self.root, &all).output().unwrap()
    }

    /// Records `project` among the store's users, as an engine would.
    fn mark_user(&self, project: &Path) {
        let users = self.store().join("projects");
        fs::create_dir_all(&users).unwrap();
        fs::write(
            users.join(grida_fx_runtime::store::lease::user_id(project)),
            "",
        )
        .unwrap();
    }

    /// A file in the store: its digest.
    fn file(&self, bytes: &[u8]) -> String {
        let digest = sha256(bytes);
        let folder = self.store().join("files").join(&digest[..2]);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(&digest), bytes).unwrap();
        digest
    }

    /// A result record naming `file`: its identity.
    fn result(&self, seed: &str, file: &str) -> String {
        let identity = sha256(seed.as_bytes());
        self.record(
            "results",
            &identity,
            &json!({"kind": "fx-result-record-v1", "identity": identity, "read": {},
                    "outputs": {"image": {"file": {"digest": file, "kind": "image/png",
                    "name": "image", "size": 1}}}, "facts": {}, "cost_usd": null}),
        );
        identity
    }

    /// A call record naming `file` and costing `cost`: its key.
    fn call(&self, seed: &str, file: &str, cost: f64) -> String {
        let key = sha256(seed.as_bytes());
        self.record(
            "calls",
            &key,
            &json!({"kind": "fx-call-record-v1", "key": key,
                    "files": [{"digest": file, "kind": "image/png", "name": "image", "size": 1}],
                    "data": {}, "cost_usd": cost}),
        );
        key
    }

    fn record(&self, top: &str, name: &str, value: &Value) {
        let folder = self.store().join(top).join(&name[..2]);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("{name}.json")), value.to_string()).unwrap();
    }

    fn present(&self, top: &str, name: &str) -> bool {
        let suffix = if top == "files" { "" } else { ".json" };
        self.store()
            .join(top)
            .join(&name[..2])
            .join(format!("{name}{suffix}"))
            .exists()
    }

    /// A run at `relative` whose log names `names` (in a node_finished, a call event and a
    /// reservation, as a run would).
    fn run(&self, relative: &str, events: &[Value]) -> PathBuf {
        let folder = self.root.join(relative);
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("plan.json"),
            json!({"kind": "fx-graph-v1", "workflow": {"id": "case", "file": "case.yaml"}})
                .to_string(),
        )
        .unwrap();
        let mut log = format!(
            "{}\n",
            json!({"event": "run_started", "created_at": "2026-10-01T00:00:00.000Z"})
        );
        for event in events {
            log.push_str(&format!("{event}\n"));
        }
        fs::write(folder.join("events.jsonl"), log).unwrap();
        folder
    }
}

fn finished() -> Value {
    json!({"event": "run_finished", "ok": true, "incomplete": false, "charged_usd": 0})
}

#[test]
fn pruning_keeps_what_runs_name_and_removes_the_rest() {
    let project = Project::new();
    let kept_file = project.file(b"kept");
    let kept = project.result("kept", &kept_file);
    let reserved_file = project.file(b"reserved");
    // A paid answer only a reservation names: its run was killed before its call event.
    let reserved = project.call("reserved", &reserved_file, 0.25);
    let gone_file = project.file(b"gone");
    let gone = project.result("gone", &gone_file);
    let gone_call = project.call("gone-call", &gone_file, 0.5);
    project.run(
        "runs/case/2026-10-01-1",
        &[
            json!({"event": "node_started", "id": "a", "identity": kept}),
            json!({"event": "budget_reserved", "node_id": "a~1", "amount_usd": 0.5,
                   "call": reserved}),
            finished(),
        ],
    );

    let preview = project.prune(&[]);
    assert_eq!(status(&preview), 0, "{}", text(&preview.stdout));
    let document = json_of(&preview);
    conforms(&document);
    assert_eq!(document["applied"], false);
    let main = &document["stores"][0];
    assert_eq!(main["results"], json!({"kept": 1, "removed": 1}));
    assert_eq!(main["calls"], json!({"kept": 1, "removed": 1}));
    assert_eq!(main["files"], json!({"kept": 2, "removed": 1}));
    assert_eq!(main["paid_usd"], 0.5);
    assert!(
        project.present("results", &gone),
        "a preview removes nothing"
    );

    let pruned = project.prune(&["--yes"]);
    assert_eq!(status(&pruned), 0, "{}", text(&pruned.stdout));
    conforms(&json_of(&pruned));
    assert!(project.present("results", &kept) && project.present("files", &kept_file));
    assert!(project.present("calls", &reserved) && project.present("files", &reserved_file));
    assert!(!project.present("results", &gone) && !project.present("calls", &gone_call));
    assert!(!project.present("files", &gone_file));
}

#[test]
fn a_file_linked_from_elsewhere_stays_with_its_records() {
    let project = Project::new();
    let file = project.file(b"placed by a run nobody listed");
    let result = project.result("elsewhere", &file);
    let elsewhere = tempfile::tempdir().unwrap();
    let store_copy = project.store().join("files").join(&file[..2]).join(&file);
    fs::hard_link(&store_copy, elsewhere.path().join("image.png")).unwrap();
    let output = project.prune(&["--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(project.present("files", &file) && project.present("results", &result));
}

#[test]
fn a_shared_or_older_cache_is_refused_until_its_other_users_are_forgotten() {
    let project = Project::new();
    let file = project.file(b"theirs");
    let other = project.root.join("../another-project");
    project.mark_user(&other);
    let other_id = grida_fx_runtime::store::lease::user_id(&other);
    let output = project.prune(&["--yes"]);
    assert_eq!(status(&output), 1);
    let document = json_of(&output);
    conforms(&document);
    assert_eq!(document["refused"][0]["code"], "shared_cache");
    assert!(project.present("files", &file));

    let output = project.prune(&["--yes", "--forget-user", &other_id]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert_eq!(json_of(&output)["forgotten_users"], json!([other_id]));
    assert!(!project.present("files", &file));

    // A store written before FX recorded its users.
    let older = Project::new();
    fs::remove_dir_all(older.store().join("projects")).unwrap();
    older.file(b"from before");
    let output = older.prune(&[]);
    assert_eq!(status(&output), 1);
    assert_eq!(json_of(&output)["refused"][0]["code"], "shared_cache");
    let output = older.prune(&["--yes", "--forget-user", "legacy"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
}

#[test]
fn what_a_prune_cannot_see_whole_refuses_it() {
    type Setup<'a> = &'a dyn Fn(&Project);
    let cases: [(&str, Setup); 5] = [
        ("job_outstanding", &|p: &Project| {
            let key = sha256(b"job");
            fs::create_dir_all(p.store().join("jobs")).unwrap();
            fs::write(
                p.store().join(format!("jobs/{key}.json")),
                json!({"kind": "fx-job-record-v1", "key": key, "state": "submitted",
                       "handle": {"id": "j1"}, "capability": "video.generate",
                       "route": {"id": "vid-a@acme", "fingerprint": sha256(b"route")},
                       "request": {}, "take": [1]})
                .to_string(),
            )
            .unwrap();
        }),
        ("foreign_run", &|p: &Project| {
            fs::create_dir_all(p.root.join("runs/old")).unwrap();
            fs::write(p.root.join("runs/old/plan.json"), r#"{"kind": "graph/v2"}"#).unwrap();
        }),
        ("legacy_interrupted_run", &|p: &Project| {
            p.run(
                "runs/case/2026-10-01-1",
                &[json!({"event": "budget_reserved", "node_id": "a~1", "amount_usd": 1})],
            );
        }),
        ("missing_run", &|p: &Project| {
            let index = grida_fx_runtime::run_index::RunIndex {
                project_root: p.root.clone(),
            };
            let gone = p.root.join("elsewhere");
            fs::create_dir_all(&gone).unwrap();
            index.record(&gone).unwrap();
            fs::remove_dir(&gone).unwrap();
        }),
        ("overlap", &|p: &Project| {
            fs::write(
                p.root.join("fx.yaml"),
                "fx: project/v1\ncache: runs/cache\n",
            )
            .unwrap();
            fs::create_dir_all(p.root.join("runs/cache/files")).unwrap();
        }),
    ];
    for (code, setup) in cases {
        let project = Project::new();
        let file = project.file(b"orphan");
        setup(&project);
        for confirmed in [false, true] {
            let args: &[&str] = if confirmed { &["--yes"] } else { &[] };
            let output = project.prune(args);
            assert_eq!(status(&output), 1, "{code}: {}", text(&output.stdout));
            let document = json_of(&output);
            conforms(&document);
            let codes: Vec<&str> = document["refused"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["code"].as_str().unwrap())
                .collect();
            assert!(codes.contains(&code), "{code}: {codes:?}");
        }
        assert!(project.present("files", &file), "{code}: nothing removed");
    }
}

#[test]
fn a_cache_or_run_in_use_is_refused() {
    let project = Project::new();
    let file = project.file(b"orphan");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(project.store().join("lock"))
        .unwrap();
    lock.try_lock_shared().unwrap();
    let output = project.prune(&["--yes"]);
    assert_eq!(status(&output), 1);
    assert_eq!(json_of(&output)["refused"][0]["code"], "cache_in_use");
    // Without --yes nothing is locked: the preview still counts.
    let output = project.prune(&[]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    drop(lock);

    // An engine older than the store's lock holds only its run.lock.
    let run = project.run("runs/case/2026-10-01-1", &[]);
    let run_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(run.join("run.lock"))
        .unwrap();
    run_lock.try_lock().unwrap();
    let output = project.prune(&["--yes"]);
    assert_eq!(status(&output), 1);
    assert_eq!(json_of(&output)["refused"][0]["code"], "cache_in_use");
    drop(run_lock);
    let output = project.prune(&["--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(!project.present("files", &file));
}

#[cfg(unix)]
#[test]
fn a_store_that_cannot_be_read_whole_refuses() {
    use std::os::unix::fs::PermissionsExt;
    let project = Project::new();
    let file = project.file(b"orphan");
    let result = project.result("hidden", &file);
    let fan = project.store().join("results").join(&result[..2]);
    fs::set_permissions(&fan, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&fan).is_ok() {
        // Running as root: permissions do not hide anything.
        fs::set_permissions(&fan, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let output = project.prune(&["--yes"]);
    fs::set_permissions(&fan, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(status(&output), 1, "{}", text(&output.stdout));
    let document = json_of(&output);
    conforms(&document);
    assert_eq!(document["refused"][0]["code"], "unreadable_store");
    assert!(project.present("files", &file));
}

#[test]
fn a_folder_without_a_plan_keeps_what_its_log_names() {
    let project = Project::new();
    let file = project.file(b"named by a half-removed run");
    let result = project.result("half", &file);
    let gone = project.file(b"named by nobody");
    let folder = project.root.join("runs/case/2026-10-01-1");
    fs::create_dir_all(folder.join("files")).unwrap();
    fs::write(
        folder.join("events.jsonl"),
        format!("{}\n", json!({"event": "node_started", "identity": result})),
    )
    .unwrap();
    let output = project.prune(&["--yes"]);
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    assert!(project.present("results", &result) && project.present("files", &file));
    assert!(!project.present("files", &gone));
}

/// A project whose workflow runs one Python node: `None` without a Python that has `grida`.
fn python_project() -> Option<Project> {
    python()?;
    let project = Project::new();
    fs::remove_dir_all(project.store()).unwrap();
    fs::write(
        project.root.join("workflow.yaml"),
        "fx: workflow/v1\nid: greeting\ntitle: Greeting\ninputs:\n  name: {type: string, \
         default: Ada}\nsteps:\n  greet:\n    uses: ./node.py#greet\n    with: { name: \
         \"${{ inputs.name }}\" }\noutputs:\n  text: ${{ steps.greet.outputs.text }}\n",
    )
    .unwrap();
    fs::write(
        project.root.join("node.py"),
        "from grida.fx import Ctx, node\n\n\n@node(\"cache_test_greet\", params={\"name\": str}, \
         outputs={\"text\": \"text\"}, version=1)\ndef greet(ctx: Ctx) -> dict:\n    return \
         {\"text\": ctx.out.text(\"Hello \" + ctx.params[\"name\"])}\n",
    )
    .unwrap();
    Some(project)
}

#[test]
fn integrated_a_run_marks_its_project_waits_for_a_prune_and_survives_one() {
    let Some(project) = python_project() else {
        return;
    };
    let run = |args: &[&str]| {
        let mut all = vec!["run", "workflow.yaml", "--no-view"];
        all.extend_from_slice(args);
        command(&project.root, &all).output().unwrap()
    };
    let output = run(&["--run", "runs/ada"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let users: Vec<String> = fs::read_dir(project.store().join("projects"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        users,
        [grida_fx_runtime::store::lease::user_id(&project.root)],
        "a new store has no legacy user"
    );
    let output = run(&["--run", "runs/bo", "--", "--name", "Bo"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );

    // While a prune holds the store, a run waits for it.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(project.store().join("lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let waiting = command(
        &project.root,
        &[
            "run",
            "workflow.yaml",
            "--no-view",
            "--run",
            "runs/cy",
            "--",
            "--name",
            "Cy",
        ],
    )
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    drop(lock);
    let output = waiting.wait_with_output().unwrap();
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert!(
        text(&output.stderr).contains("waiting for grida-fx cache prune to finish"),
        "{}",
        text(&output.stderr)
    );

    let output = command(&project.root, &["runs", "remove", "runs/ada", "--yes"])
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    let output = project.prune(&["--yes"]);
    assert_eq!(
        status(&output),
        0,
        "{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let document = json_of(&output);
    conforms(&document);
    assert_eq!(
        document["stores"][0]["results"]["removed"], 1,
        "{document:#}"
    );
    let output = command(&project.root, &["inspect", "runs/bo", "--verify"])
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", text(&output.stdout));
    // Bo's result is still cached; Ada's is made again.
    let output = run(&["--run", "runs/bo2", "--", "--name", "Bo"]);
    assert_eq!(status(&output), 0);
    let log = fs::read_to_string(project.root.join("runs/bo2/events.jsonl")).unwrap();
    assert!(log.contains(r#""cache":"hit""#), "{log}");
    let output = run(&["--run", "runs/ada2"]);
    assert_eq!(status(&output), 0);
    let log = fs::read_to_string(project.root.join("runs/ada2/events.jsonl")).unwrap();
    assert!(log.contains(r#""cache":"miss""#), "{log}");
}
