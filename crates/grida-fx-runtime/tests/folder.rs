//! Run folders (spec/store.md §8): names, the "one file or several" rule, new folders, the lock,
//! the plan check, `plan.json` and placing files.

use grida_fx_core::host::{FakeHost, NoCache};
use grida_fx_core::plan::make_plan;
use grida_fx_core::plan::output::{graph_document, plan_digest};
use grida_fx_core::project::{PlanRequest, make_planner};
use grida_fx_core::val::{Collection, FileValue, Val};
use grida_fx_core::value::{file_digest, write_json};
use grida_fx_runtime::folder::{
    FolderRefused, NAME_BYTES, RunFolder, capped, check_plan, keyed_path, local_date, named,
    new_folder, output_files, plan_document, recorded_plan, safe_name, step_files, step_folder,
    takes_of_id,
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const PNG: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const JPG: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const TXT: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn file(digest: &str, kind: &str, key: Option<&str>) -> Val {
    Val::File(Box::new(FileValue {
        digest: digest.into(),
        kind: kind.into(),
        name: "x".into(),
        size: 1,
        key: key.map(str::to_string),
        content: None,
        location: None,
    }))
}

fn collection(items: Vec<(&str, Val)>) -> Val {
    Val::Collection(Box::new(Collection {
        items: items.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        verdicts: IndexMap::new(),
    }))
}

fn paths(placed: Vec<(String, String)>) -> Vec<(String, String)> {
    placed
}

fn pair(path: &str, digest: &str) -> (String, String) {
    (path.to_string(), digest.to_string())
}

// ------------------------------------------------------------------ names

#[test]
fn step_paths_become_one_folder_name() {
    let cases = [
        ("entity['ada'].draw", "entity__ada__.draw"),
        ("loud['ada']", "loud__ada"),
        ("draw", "draw"),
        ("a b", "a_b"),
        ("a/b", "a_b"),
        ("g['a/b'].x", "g__a_b__.x"),
        ("__x__", "x"),
        ("", "step"),
        ("['']", "step"),
        ("___", "step"),
        ("café-1.2_x", "café-1.2_x"),
        ("場面['夜']", "場面__夜"),
        ("v[٣]", "v_٣"),
        ("a\u{1F600}b", "a_b"),
        // General category L or N only: a combining mark (Mc) and a circled letter (So) are
        // Alphabetic, yet neither is a letter.
        ("\u{915}\u{93F}", "\u{915}"),
        ("x\u{24B6}y", "x_y"),
        ("\u{2163}\u{BD}", "\u{2163}\u{BD}"),
    ];
    for (path, expected) in cases {
        assert_eq!(safe_name(path), expected, "{path:?}");
    }
}

#[test]
fn keys_become_paths_that_stay_inside() {
    let cases = [
        ("ada", "ada"),
        ("hero.png", "hero.png"),
        ("a/b", "a/b"),
        ("a b/c d", "a_b/c_d"),
        ("../x", "_/x"),
        ("..", "_"),
        (".", "_"),
        ("", "_"),
        ("/abs", "_/abs"),
        ("a//b", "a/_/b"),
        ("a/", "a/_"),
        ("x/../../y", "x/_/_/y"),
        ("__", "step"),
        ("..hidden", "..hidden"),
    ];
    for (key, expected) in cases {
        assert_eq!(keyed_path(key), expected, "{key:?}");
    }
}

#[test]
fn suffixes_are_the_first_one_identity_lists() {
    let cases = [
        ("image", "image/png", "image.png"),
        ("image", "image/jpeg", "image.jpg"),
        ("image", "image/gif", "image.gif"),
        ("doc", "text/yaml", "doc.yaml"),
        ("model", "model/gltf+json", "model.gltf"),
        ("marks", "annotations", "marks.json"),
        ("data", "json", "data.json"),
        ("data.json", "json", "data.json"),
        ("blob", "file", "blob"),
        ("blob", "image", "blob"),
        ("hero.png", "image/png", "hero.png"),
        // Compared case-sensitively.
        ("hero.PNG", "image/png", "hero.PNG.png"),
        ("hero.jpeg", "image/jpeg", "hero.jpeg.jpg"),
    ];
    for (label, kind, expected) in cases {
        assert_eq!(named(label, kind), expected, "{label} {kind}");
    }
}

// ------------------------------------------------------------------ one file or several

#[test]
fn a_port_with_one_file_is_named_after_the_port() {
    let outputs = IndexMap::from([
        ("image".to_string(), file(PNG, "image/png", None)),
        ("note".to_string(), Val::Str("not a file".into())),
        ("nothing".to_string(), Val::Null),
    ]);
    assert_eq!(
        paths(step_files("entity['ada'].draw", &[1], &outputs)),
        [pair("files/entity__ada__.draw/image.png", PNG)]
    );
}

#[test]
fn a_port_with_several_files_gets_a_folder() {
    let outputs = IndexMap::from([
        (
            "seq".to_string(),
            Val::List(vec![file(JPG, "json", None), file(TXT, "text/plain", None)]),
        ),
        (
            "items".to_string(),
            collection(vec![
                ("k0", file(TXT, "text/plain", Some("k0"))),
                ("hero.png", file(PNG, "image/png", Some("hero.png"))),
            ]),
        ),
        (
            "mixed".to_string(),
            Val::Object(IndexMap::from([
                ("a".to_string(), file(PNG, "image/png", None)),
                ("b".to_string(), Val::Number(1.0)),
                ("c".to_string(), file(JPG, "image/jpeg", None)),
            ])),
        ),
    ]);
    assert_eq!(
        step_files("m", &[1], &outputs),
        [
            pair("files/m/seq/0.json", JPG),
            pair("files/m/seq/1.txt", TXT),
            pair("files/m/items/k0.txt", TXT),
            pair("files/m/items/hero.png", PNG),
            pair("files/m/mixed/0.png", PNG),
            pair("files/m/mixed/1.jpg", JPG),
        ]
    );
}

#[test]
fn a_keyed_collection_with_one_item_is_one_file_of_a_step() {
    let outputs = IndexMap::from([(
        "items".to_string(),
        collection(vec![("ada", file(PNG, "image/png", Some("ada")))]),
    )]);
    assert_eq!(
        step_files("draw", &[1], &outputs),
        [pair("files/draw/items.png", PNG)]
    );
}

#[test]
fn keys_with_slashes_and_dots_stay_under_the_port() {
    let outputs = IndexMap::from([(
        "items".to_string(),
        collection(vec![
            ("a/b", file(PNG, "image/png", Some("a/b"))),
            ("../up", file(PNG, "image/png", Some("../up"))),
            ("", file(TXT, "text/plain", Some(""))),
            // A collection item whose file carries no key of its own takes the item's key.
            ("bo", file(JPG, "image/jpeg", None)),
        ]),
    )]);
    assert_eq!(
        step_files("s", &[1], &outputs),
        [
            pair("files/s/items/a/b.png", PNG),
            pair("files/s/items/_/up.png", PNG),
            pair("files/s/items/2.txt", TXT),
            pair("files/s/items/bo.jpg", JPG),
        ]
    );
}

#[test]
fn an_output_is_one_file_only_when_its_value_is_one() {
    assert_eq!(
        output_files("one", &file(TXT, "text/plain", None)),
        [pair("outputs/one.txt", TXT)]
    );
    // A keyed file output keeps the output's name.
    assert_eq!(
        output_files("hero", &file(PNG, "image/png", Some("ada"))),
        [pair("outputs/hero.png", PNG)]
    );
    assert_eq!(
        output_files("seq", &Val::List(vec![file(JPG, "json", None)])),
        [pair("outputs/seq/0.json", JPG)]
    );
    assert_eq!(
        output_files(
            "each",
            &collection(vec![("x", file(TXT, "text/plain", Some("x")))])
        ),
        [pair("outputs/each/x.txt", TXT)]
    );
    assert_eq!(
        output_files(
            "each",
            &collection(vec![
                ("hero.png", file(PNG, "image/png", Some("hero.png"))),
                ("a/../b", file(PNG, "image/png", Some("a/../b"))),
            ])
        ),
        [
            pair("outputs/each/hero.png", PNG),
            pair("outputs/each/a/_/b.png", PNG),
        ]
    );
    // An object holding one file is not one file.
    assert_eq!(
        output_files(
            "obj",
            &Val::Object(IndexMap::from([(
                "a".to_string(),
                file(PNG, "image/png", None)
            )]))
        ),
        [pair("outputs/obj/0.png", PNG)]
    );
    for nothing in [
        Val::Str("text".into()),
        Val::Null,
        Val::Missing,
        Val::Failed("bad#1".into()),
        Val::List(vec![]),
    ] {
        assert!(output_files("v", &nothing).is_empty(), "{nothing:?}");
    }
}

// ------------------------------------------------------------------ new folders

#[test]
fn a_new_folder_takes_the_smallest_free_number() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let date = local_date();
    assert_eq!(date.len(), 10);
    assert!(date.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 {
        c == '-'
    } else {
        c.is_ascii_digit()
    }));
    let first = new_folder(&runs, "case").unwrap();
    assert_eq!(first, runs.join("case").join(format!("{date}-1")));
    assert!(
        first.is_dir(),
        "the new folder is made, so nobody else takes it"
    );
    std::fs::create_dir_all(runs.join("case").join(format!("{date}-3"))).unwrap();
    let second = new_folder(&runs, "case").unwrap();
    assert_eq!(second, runs.join("case").join(format!("{date}-2")));
    // A plain file of that name is taken too.
    std::fs::remove_dir(&second).unwrap();
    std::fs::write(&second, b"").unwrap();
    assert_eq!(
        new_folder(&runs, "case").unwrap(),
        runs.join("case").join(format!("{date}-4"))
    );
    assert_eq!(
        new_folder(&runs, "other").unwrap(),
        runs.join("other").join(format!("{date}-1"))
    );
}

#[test]
fn invocations_starting_at_once_never_share_a_new_folder() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let made: Vec<std::path::PathBuf> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| new_folder(&runs, "case").unwrap()))
            .collect();
        workers.into_iter().map(|w| w.join().unwrap()).collect()
    });
    let distinct: std::collections::BTreeSet<_> = made.iter().collect();
    assert_eq!(distinct.len(), 16, "{made:?}");
}

// ------------------------------------------------------------------ takes and long names

#[test]
fn each_take_of_a_step_has_a_folder_of_its_own() {
    let outputs = IndexMap::from([("text".to_string(), file(TXT, "text/plain", None))]);
    assert_eq!(
        step_files("draw", &[1], &outputs),
        [pair("files/draw/text.txt", TXT)]
    );
    assert_eq!(
        step_files("draw", &[2], &outputs),
        [pair("files/draw#2/text.txt", TXT)]
    );
    assert_eq!(
        step_files("entity['ada'].draw", &[1, 3], &outputs),
        [pair("files/entity__ada__.draw#1.3/text.txt", TXT)]
    );
    assert_eq!(step_folder("draw", &[1, 1]), "draw#1.1");
    assert_eq!(takes_of_id("draw#2"), [2]);
    assert_eq!(takes_of_id("entity['a#b'].draw#1.3"), [1, 3]);
    assert_eq!(takes_of_id("draw"), [1]);
}

#[test]
fn names_too_long_for_a_file_system_are_cut_with_a_digest() {
    let key = "k".repeat(300);
    let outputs = IndexMap::from([(
        "items".to_string(),
        collection(vec![
            (key.as_str(), file(PNG, "image/png", Some(key.as_str()))),
            ("short", file(TXT, "text/plain", Some("short"))),
        ]),
    )]);
    let placed = step_files(&"s".repeat(400), &[1], &outputs);
    for (path, _) in &placed {
        for segment in path.split('/') {
            assert!(segment.len() <= NAME_BYTES, "{segment}");
        }
    }
    let long = &placed[0].0;
    let name = long.rsplit('/').next().unwrap();
    assert_eq!(name.len(), NAME_BYTES);
    assert!(name.ends_with(".png") && name.contains('~'), "{name}");
    assert!(name.starts_with("kkkk"));
    // Names that fit are left as they are; a cut is the same every time and differs per name.
    assert!(placed[1].0.ends_with("/items/short.txt"));
    assert_eq!(capped(long, "image/png"), *long);
    assert_ne!(
        capped(&format!("files/s/{}", "a".repeat(300)), "file"),
        capped(&format!("files/s/{}b", "a".repeat(299)), "file")
    );
    // A name of exactly the limit is kept; one byte more is cut, at a character boundary.
    let exact = "é".repeat(127) + "x";
    assert_eq!(exact.len(), NAME_BYTES);
    assert_eq!(capped(&exact, "file"), exact);
    let over = "é".repeat(128);
    let cut = capped(&over, "file");
    assert!(cut.len() <= NAME_BYTES && cut.contains('~'), "{cut}");
}

#[test]
fn a_name_that_fits_is_placed_however_long() {
    let dir = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let source = store.path().join(TXT);
    std::fs::write(&source, b"x").unwrap();
    let folder = RunFolder::lock(&dir.path().join("run"), "runs/three").unwrap();
    // The longest name a file system takes: the temporary name used to place it is shorter.
    let name = format!("files/s/{}", "k".repeat(NAME_BYTES));
    folder.place(&name, &source).unwrap();
    assert_eq!(std::fs::read(folder.path.join(&name)).unwrap(), b"x");
}

#[test]
fn what_a_killed_invocation_half_placed_is_swept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run");
    let folder = RunFolder::lock(&path, "runs/one").unwrap();
    std::fs::create_dir_all(path.join("files/big__b2")).unwrap();
    std::fs::create_dir_all(path.join("outputs/all")).unwrap();
    let left = [
        path.join("files/big__b2/.blob.11366-4.part"),
        path.join("outputs/all/.0123456789abcdef.part"),
        path.join(".fedcba9876543210.part"),
    ];
    let kept = [
        path.join("files/big__b2/blob"),
        path.join("outputs/all/.hidden.part"),
        path.join("events.jsonl"),
    ];
    for file in left.iter().chain(&kept) {
        std::fs::write(file, b"x").unwrap();
    }
    folder.sweep();
    for file in &left {
        assert!(!file.exists(), "{}", file.display());
    }
    for file in &kept {
        assert!(file.exists(), "{}", file.display());
    }
}

// ------------------------------------------------------------------ the lock

#[test]
fn a_held_folder_is_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs/one");
    let held = RunFolder::lock(&path, "runs/one").unwrap();
    assert!(path.join("run.lock").is_file());
    assert_eq!(held.label, "runs/one");
    assert_eq!(held.plan_path(), path.join("plan.json"));
    assert_eq!(held.events_path(), path.join("events.jsonl"));
    assert_eq!(
        RunFolder::lock(&path, "runs/one").unwrap_err(),
        FolderRefused("another invocation is running runs/one".into())
    );
    drop(held);
    let again = RunFolder::lock(&path, "runs/one").unwrap();
    drop(again);
    // The lock file stays in place.
    assert!(path.join("run.lock").is_file());
}

// ------------------------------------------------------------------ the plan check

#[test]
fn a_folder_of_another_plan_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path();
    let digest = "a".repeat(64);
    // No plan.json: a new run.
    assert_eq!(check_plan(folder, "runs/x", &digest), Ok(()));
    assert_eq!(recorded_plan(folder), Ok(None));
    std::fs::write(
        folder.join("plan.json"),
        write_json(&json!({"kind": "fx-graph-v1", "plan": digest})),
    )
    .unwrap();
    assert_eq!(check_plan(folder, "runs/x", &digest), Ok(()));
    assert_eq!(recorded_plan(folder), Ok(Some(Value::from(digest.clone()))));
    let refused = FolderRefused(
        "runs/x holds a run of another workflow or other inputs; choose a new folder".into(),
    );
    assert_eq!(
        check_plan(folder, "runs/x", &"b".repeat(64)),
        Err(refused.clone())
    );
    // A plan.json that records no plan is another plan.
    std::fs::write(folder.join("plan.json"), "{\"kind\": \"fx-graph-v1\"}").unwrap();
    assert_eq!(check_plan(folder, "runs/x", &digest), Err(refused));
    assert_eq!(recorded_plan(folder), Ok(None));
    // An unreadable one is refused with a sentence that names no absolute path.
    for broken in ["{not json", "[1, 2]", "{\"plan\": 1, \"plan\": 2}"] {
        std::fs::write(folder.join("plan.json"), broken).unwrap();
        let FolderRefused(message) = check_plan(folder, "runs/x", &digest).unwrap_err();
        assert!(
            message.starts_with("runs/x holds a plan.json that cannot be read (")
                && message.ends_with("); choose a new folder"),
            "{message}"
        );
        assert!(
            !message.contains(&*dir.path().to_string_lossy()),
            "{message}"
        );
        assert!(recorded_plan(folder).is_err());
    }
}

// ------------------------------------------------------------------ plan.json

#[test]
fn plan_json_is_written_once() {
    let dir = tempfile::tempdir().unwrap();
    let folder = RunFolder::lock(&dir.path().join("run"), "run").unwrap();
    let first = json!({"kind": "fx-graph-v1", "plan": "a".repeat(64), "b": [1, {"c": "é"}]});
    folder.write_plan_once(&first).unwrap();
    let written = std::fs::read_to_string(folder.plan_path()).unwrap();
    assert_eq!(written, write_json(&first));
    assert!(!written.ends_with('\n'));
    folder
        .write_plan_once(&json!({"kind": "fx-graph-v1", "plan": "b".repeat(64)}))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(folder.plan_path()).unwrap(),
        written
    );
    // No temporary file is left behind.
    let names: Vec<String> = std::fs::read_dir(&folder.path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new() -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("proj");
        let project = Project { _dir: dir, root };
        project.write(
            "fx.yaml",
            "fx: project/v1\nview_origins: [\"https://example.test\"]\nroutes:\n  image.generate: img-a@acme\n",
        );
        project.write(
            "routes.yaml",
            "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, price: { low_usd: 0.01, high_usd: 0.04 } }\n",
        );
        project.write(
            "workflows/case.yaml",
            r#"fx: workflow/v1
id: case
title: A plan kept in a run folder
description: Two pictures, one inside a group
inputs:
  mode: { type: string, default: a }
steps:
  base:
    title: The base picture
    description: Drawn first
    view: true
    uses: fx/image.generate@1
    with: { prompt: "base ${{ inputs.mode }}" }
  build:
    title: A group
    steps:
      draw: { uses: fx/image.generate@1, with: { prompt: a mesh } }
"#,
        );
        project
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

#[test]
fn plan_json_is_the_graph_with_the_run_members() {
    let project = Project::new();
    let request = PlanRequest {
        target: "case".into(),
        cwd: project.root.clone(),
        rest: vec!["--mode".into(), "b".into()],
        routes: vec!["routes.yaml".into()],
        ..PlanRequest::default()
    };
    let mut host = FakeHost::new();
    let mut planner = make_planner(&request, &mut host).unwrap();
    let plan = make_plan(&mut planner, &mut host, None, &NoCache).unwrap();
    assert!(plan.ok(), "{:?}", plan.problems);
    let digest = plan_digest(&plan, &planner).unwrap();
    let document = plan_document(&plan, &planner, &digest, "workflows/case.takes.yaml", false);

    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-graph-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&document)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");

    assert_eq!(document["plan"], Value::from(digest));
    assert_eq!(document["takes_file"], "workflows/case.takes.yaml");
    assert_eq!(document["inputs"], json!({"mode": "b"}));
    assert_eq!(document["view_origins"], json!(["https://example.test"]));
    assert_eq!(
        document["steps"],
        json!({
            "base": {"title": "The base picture", "description": "Drawn first", "uses": "fx/image.generate@1", "view": true, "order": 0},
            "build": {"title": "A group", "description": null, "uses": null, "view": false, "order": 1},
            "build.draw": {"title": null, "description": null, "uses": "fx/image.generate@1", "view": false, "order": 2},
        })
    );
    // Everything else is exactly what `grida-fx expand` prints, `problems` included.
    let mut rest = document.as_object().unwrap().clone();
    for added in ["plan", "steps", "inputs", "view_origins", "takes_file"] {
        rest.remove(added);
    }
    assert_eq!(Value::Object(rest), graph_document(&plan, &planner));
    assert_eq!(document["problems"], json!([]));
    // No absolute path anywhere in it.
    let text = write_json(&document);
    assert!(!text.contains(&*project.root.to_string_lossy()), "{text}");

    // An empty takes file is left out.
    let without = plan_document(&plan, &planner, &"0".repeat(64), "", false);
    assert!(without.get("takes_file").is_none());
}

#[test]
fn the_schema_holds_takes_file_to_a_relative_path() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/fx-graph-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let graph = |takes_file: &str| {
        json!({
            "kind": "fx-graph-v1",
            "workflow": {"id": "case", "title": "t"},
            "instances": [],
            "pending": [],
            "estimate": {"low_usd": 0, "high_usd": 0, "ceiling_usd": null},
            "takes_file": takes_file,
        })
    };
    assert!(validator.is_valid(&graph("workflows/case.takes.yaml")));
    assert!(validator.is_valid(&graph("case.takes.yaml")));
    for refused in ["/abs/case.takes.yaml", "../case.takes.yaml", "a//b", ""] {
        assert!(!validator.is_valid(&graph(refused)), "{refused}");
    }
}

// ------------------------------------------------------------------ placing

/// A file named by its digest, as the store keeps it.
fn stored(dir: &Path, bytes: &[u8]) -> PathBuf {
    let digest = file_digest(bytes);
    let path = dir.join(&digest);
    std::fs::write(&path, bytes).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn placing_links_the_store_file_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let source = stored(&store, b"one\ntwo\n");
    let folder = RunFolder::lock(&dir.path().join("run"), "run").unwrap();
    folder.place("files/draw/text.txt", &source).unwrap();
    let target = folder.path.join("files/draw/text.txt");
    assert_eq!(std::fs::read(&target).unwrap(), b"one\ntwo\n");
    #[cfg(unix)]
    assert_eq!(
        inode(&target),
        inode(&source),
        "a hard link to the store's copy"
    );
    folder.place("files/draw/text.txt", &source).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"one\ntwo\n");
    // No temporary file is left in the destination folder.
    let names: Vec<String> = std::fs::read_dir(folder.path.join("files/draw"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["text.txt"]);
}

#[test]
fn a_name_holding_the_same_bytes_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let source = stored(&store, b"same bytes");
    let folder = RunFolder::lock(&dir.path().join("run"), "run").unwrap();
    let target = folder.path.join("outputs/one.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"same bytes").unwrap();
    #[cfg(unix)]
    let before = inode(&target);
    folder.place("outputs/one.txt", &source).unwrap();
    #[cfg(unix)]
    assert_eq!(inode(&target), before, "a copy with the same bytes is kept");
    assert_eq!(std::fs::read(&target).unwrap(), b"same bytes");
}

#[test]
fn different_bytes_of_the_same_size_are_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let first = stored(&store, b"take one");
    let second = stored(&store, b"take two");
    let folder = RunFolder::lock(&dir.path().join("run"), "run").unwrap();
    folder.place("files/draw/image.png", &first).unwrap();
    folder.place("files/draw/image.png", &second).unwrap();
    let target = folder.path.join("files/draw/image.png");
    assert_eq!(std::fs::read(&target).unwrap(), b"take two");
    #[cfg(unix)]
    assert_eq!(inode(&target), inode(&second));
    // A file of another size is replaced too.
    let longer = stored(&store, b"a longer third take");
    folder.place("files/draw/image.png", &longer).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"a longer third take");
}

#[test]
fn a_source_not_named_by_a_digest_is_compared_by_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("plain-name");
    std::fs::write(&source, b"abc").unwrap();
    let folder = RunFolder::lock(&dir.path().join("run"), "run").unwrap();
    let target = folder.path.join("files/x/y.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"xyz").unwrap();
    folder.place("files/x/y.txt", &source).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"abc");
}

#[test]
fn placing_refuses_a_path_that_leaves_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let source = stored(dir.path(), b"x");
    let folder = RunFolder::lock(&dir.path().join("run"), "run").unwrap();
    for bad in ["../x", "files/../../x", "/abs", "", "files//x", "files/./x"] {
        let error = folder.place(bad, &source).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{bad}");
    }
    assert!(!dir.path().join("x").exists());
    // A missing source is an error that names no path.
    let error = folder
        .place("files/a/b.txt", &dir.path().join("missing"))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(!error.to_string().contains(&*dir.path().to_string_lossy()));
}
