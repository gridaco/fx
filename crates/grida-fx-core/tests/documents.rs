//! Documents, schema validation and workflow inputs through the public API.
//!
//! The tests at the top level need nothing but this crate's documents and inputs modules. The
//! groups below need more of the core: the strict YAML loader ([`with_yaml`]), file kinds and
//! text decoding ([`with_files`]), and Python-style quoting in messages ([`with_text`]).

use grida_fx_core::ErrorKind;
use grida_fx_core::docs::workflow::{OnReject, Pick, parse_workflow};
use grida_fx_core::inputs::bind::{RootInputs, anchor, bind_given, load_inputs};
use grida_fx_core::inputs::flags::parse_input_flags;
use grida_fx_core::inputs::{compile_inputs, flag_name, required, validate, with_defaults};
use grida_fx_core::val::{FileValue, Val};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn conformance() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

fn map(value: Value) -> IndexMap<String, Value> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn workflow(steps: Value) -> Value {
    json!({"fx": "workflow/v1", "id": "case", "title": "Case", "steps": steps})
}

fn write(root: &Path, relative: &str, text: &str) -> PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn texts(paths: &Value) -> Vec<String> {
    paths
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect()
}

// ------------------------------------------------------------------------------ workflows

#[test]
fn step_shape_messages_name_the_step() {
    let message = |steps: Value| parse_workflow(&workflow(steps), "case.yaml").unwrap_err();
    let error =
        message(json!({"draw": {"uses": "fx/image.generate@1", "steps": {"a": {"uses": "x"}}}}));
    assert_eq!(error.kind, ErrorKind::Document);
    assert_eq!(
        error.message,
        "case.yaml: steps.draw: a step has either uses: (a node type) or steps: (a group)"
    );
    assert_eq!(
        message(json!({"g": {"steps": {"inner": {"uses": "fx/x@1", "on_reject": "skip"}}}}))
            .message,
        "case.yaml: steps.g.steps.inner: on_reject: belongs on a judge (a step with judges:)"
    );
    assert_eq!(
        message(json!({"g": {"steps": {"a": {"uses": "fx/x@1"}}, "pick": "manual"}})).message,
        "case.yaml: steps.g: pick: applies to a node step, not a group"
    );
    assert_eq!(
        message(json!({"g": {"steps": {"a": {"uses": "fx/x@1"}}, "regenerate": {"max": 2}}}))
            .message,
        "case.yaml: steps.g: a group's regenerate: needs until:"
    );
}

#[test]
fn a_workflow_document_is_typed() {
    let document = json!({
        "fx": "workflow/v1", "id": "case", "title": "Case",
        "inputs": {"names": {"type": "list", "items": {"type": "string"}}},
        "tables": {"sizes": [1, 2]},
        "let": {"n": "${{ len(inputs.names) }}"},
        "assert": [{"check": "${{ let.n < 3 }}", "message": "at most two"}],
        "steps": {
            "draw": {"uses": "fx/image.generate@1", "takes": 3, "pick": "first_accepted",
                     "with": {"prompt": "three takes"}},
            "check": {"uses": "./nodes/cases.py#verdict", "judges": "draw",
                      "with": {"subject": "${{ steps.draw.outputs.image }}"},
                      "on_reject": "continue", "independent_of": ["draw"]},
        },
        "outputs": {"image": "${{ steps.draw.outputs.image }}"},
        "view": "v.html",
    });
    let w = parse_workflow(&document, "case.yaml").unwrap();
    assert_eq!(w.id, "case");
    assert_eq!(w.tables["sizes"], json!([1, 2]));
    assert_eq!(w.let_["n"], json!("${{ len(inputs.names) }}"));
    assert_eq!(w.asserts.len(), 1);
    assert_eq!(w.steps["draw"].pick, Some(Pick::FirstAccepted));
    assert_eq!(w.steps["check"].on_reject, OnReject::Continue);
    assert_eq!(w.steps["check"].judges.as_deref(), Some("draw"));
    assert_eq!(w.outputs.keys().collect::<Vec<_>>(), ["image"]);
    assert_eq!(w.view.as_deref(), Some("v.html"));
}

// ------------------------------------------------------------------------------ inputs

#[test]
fn compile_inputs_on_the_captured_shapes() {
    // What stage-gen's `schema` verb printed for the `linear` case, with FX's tag.
    let linear = compile_inputs(
        &map(json!({"brief": {"type": "file", "kind": "text/plain"}})),
        None,
    )
    .unwrap();
    assert_eq!(
        linear,
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "additionalProperties": false,
            "properties": {"brief": {"type": "string", "x-fx-file": {"kind": "text/plain"}}},
            "required": ["brief"],
            "type": "object",
        })
    );
    // `tiered-price`: no inputs.
    let none = compile_inputs(&IndexMap::new(), None).unwrap();
    assert_eq!(
        none,
        json!({"$schema": "https://json-schema.org/draft/2020-12/schema",
               "additionalProperties": false, "properties": {}, "type": "object"})
    );
    // The `flags` probe's optional members.
    let flags = compile_inputs(
        &map(json!({
            "ratio": {"type": "number", "optional": true},
            "shots": {"type": "files", "kind": "image", "optional": true},
        })),
        None,
    )
    .unwrap();
    assert_eq!(
        flags["properties"],
        json!({
            "ratio": {"default": null, "type": ["number", "null"]},
            "shots": {"default": null, "items": {"type": "string"}, "type": ["array", "null"],
                      "x-fx-file": {"glob": false, "kind": "image", "many": true}},
        })
    );
    assert!(flags.get("required").is_none());
    assert!(!required(&json!({"type": "number", "optional": true})));
}

#[test]
fn defaults_fill_nested_groups_but_not_map_values() {
    let schema = compile_inputs(
        &map(json!({
            "settings": {"tone": {"type": "string", "default": "calm"},
                         "size": {"type": "integer", "optional": true}},
            "deep": {"must": {"type": "string"}},
            "byname": {"type": "map", "values": {"w": {"type": "integer", "default": 1}}},
        })),
        None,
    )
    .unwrap();
    assert_eq!(
        with_defaults(&schema, json!({"byname": {"k": {}}})),
        json!({"byname": {"k": {}}, "settings": {"tone": "calm", "size": null}})
    );
}

#[test]
fn flags_parse_and_refuse() {
    let schema = compile_inputs(
        &map(json!({
            "max_entities": {"type": "integer"},
            "ratio": {"type": "number", "optional": true},
            "loud": {"type": "boolean"},
            "name": {"type": "string"},
            "brief": {"type": "file"},
            "shots": {"type": "files"},
            "tags": {"type": "list", "items": {"type": "string"}},
        })),
        None,
    )
    .unwrap();
    let rest = |text: &str| -> Vec<String> { text.split(' ').map(String::from).collect() };
    let parsed = parse_input_flags(
        &schema,
        &rest("--brief brief.txt --shots a.png --max-entities=3 --shots b.png --loud no"),
        "flags",
    )
    .unwrap();
    assert_eq!(
        Value::Object(parsed.into_iter().collect()),
        json!({"max_entities": 3, "loud": false, "brief": "brief.txt", "shots": ["a.png", "b.png"]})
    );
    let error = parse_input_flags(&schema, &rest("--loud maybe"), "flags").unwrap_err();
    assert_eq!(error.kind, ErrorKind::Usage);
    assert_eq!(
        error.message,
        "bad input flag in --loud maybe: argument --loud: maybe is not true or false"
    );
    assert_eq!(
        parse_input_flags(&schema, &rest("--tags a"), "flags")
            .unwrap_err()
            .message,
        "unknown input flag --tags; lists and maps of flags come from --inputs"
    );
    assert_eq!(
        parse_input_flags(&schema, &rest("extra"), "flags")
            .unwrap_err()
            .message,
        "unknown input flag extra; lists and maps of flags come from --inputs"
    );
    assert_eq!(
        parse_input_flags(&schema, &rest("--max 3"), "flags")
            .unwrap_err()
            .message,
        "unknown input flag --max; lists and maps of flags come from --inputs"
    );
    for refused in ["nan", "inf", "1_000", "2.5"] {
        let error = parse_input_flags(&schema, &rest(&format!("--max-entities {refused}")), "f")
            .unwrap_err();
        assert!(
            error.message.starts_with("bad input flag in "),
            "{}",
            error.message
        );
    }
    assert_eq!(flag_name("max_entities"), "--max-entities");
}

#[test]
fn validation_sorts_and_limits() {
    let schema = compile_inputs(
        &map(
            json!({"b": {"type": "integer"}, "a": {"type": "list", "items": {"type": "integer"}}}),
        ),
        None,
    )
    .unwrap();
    assert!(validate(&schema, &json!({"b": 2.0, "a": []})).is_ok());
    let value = json!({"b": true, "a": Value::Array(vec![json!("x"); 12])});
    let message = validate(&schema, &value).unwrap_err();
    let locations: Vec<&str> = message
        .split("; ")
        .map(|line| line.split(": ").next().unwrap())
        .collect();
    assert_eq!(
        locations,
        [
            "inputs.a[0]",
            "inputs.a[1]",
            "inputs.a[2]",
            "inputs.a[3]",
            "inputs.a[4]",
            "inputs.a[5]",
            "inputs.a[6]",
            "inputs.a[7]"
        ]
    );
}

#[test]
fn anchoring_and_globs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for file in [
        "data/b.txt",
        "data/a.txt",
        "data/sub/c.txt",
        "data/.hidden.txt",
        "data/x.md",
    ] {
        write(&root, file, "x\n");
    }
    std::fs::create_dir_all(root.join("data/folder.txt")).unwrap();
    let schema = compile_inputs(
        &map(json!({
            "brief": {"type": "file"},
            "shots": {"type": "files", "glob": true},
            "plain": {"type": "files"},
            "rows": {"type": "list", "items": {"pic": {"type": "file"}}},
            "byname": {"type": "map", "values": {"type": "file"}},
            "n": {"type": "integer"},
        })),
        None,
    )
    .unwrap();
    let anchored =
        |name: &str, value: Value| anchor(&schema["properties"][name], value, &root).unwrap();
    let at = |relative: &str| root.join(relative).to_string_lossy().into_owned();
    assert_eq!(
        anchored("brief", json!("data/a.txt")),
        json!(at("data/a.txt"))
    );
    assert_eq!(
        anchored("brief", json!("data/../data/x.md")),
        json!(at("data/x.md"))
    );
    assert_eq!(anchored("brief", Value::Null), Value::Null);
    // Matches are files only, sorted, hidden names included; `**` reaches every depth.
    assert_eq!(
        texts(&anchored("shots", json!("data/*.txt"))),
        [at("data/.hidden.txt"), at("data/a.txt"), at("data/b.txt")]
    );
    assert_eq!(
        texts(&anchored("shots", json!(["data/**/*.txt", "data/x.md"]))),
        [
            at("data/.hidden.txt"),
            at("data/a.txt"),
            at("data/b.txt"),
            at("data/sub/c.txt"),
            at("data/x.md")
        ]
    );
    assert_eq!(
        texts(&anchored("shots", json!("data/?.md"))),
        [at("data/x.md")]
    );
    assert_eq!(
        texts(&anchored("shots", json!("data/[ab].txt"))),
        [at("data/a.txt"), at("data/b.txt")]
    );
    assert!(texts(&anchored("shots", json!("nothing/*.txt"))).is_empty());
    // Without glob: true a pattern is a literal path.
    assert_eq!(
        texts(&anchored("plain", json!("data/*.txt"))),
        [at("data/*.txt")]
    );
    // An absolute pattern is refused.
    let pattern = format!("{}/data/*.txt", root.display());
    let error = anchor(&schema["properties"]["shots"], json!(pattern), &root).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
    // Nested files, list items and map values are anchored; other values are left alone.
    assert_eq!(
        anchored("rows", json!([{"pic": "data/a.txt"}])),
        json!([{"pic": at("data/a.txt")}])
    );
    assert_eq!(
        anchored("byname", json!({"k": "data/b.txt"})),
        json!({"k": at("data/b.txt")})
    );
    assert_eq!(anchored("n", json!(3)), json!(3));
}

#[test]
fn load_inputs_from_flags_checks_names_and_values() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let schema = compile_inputs(
        &map(json!({"brief": {"type": "file"}, "count": {"type": "integer", "minimum": 1}})),
        None,
    )
    .unwrap();
    let flags = |value: Value| map(value);
    let error = load_inputs(
        &schema,
        &[],
        flags(json!({"zeta": 1, "alpha": 2, "count": 1})),
        &root,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Input);
    assert_eq!(error.message, "no input named alpha, zeta");
    // A missing file is named as typed, never with the absolute path.
    let error = load_inputs(
        &schema,
        &[],
        flags(json!({"brief": "notes/missing.txt", "count": 2})),
        &root,
    )
    .unwrap_err();
    assert_eq!(error.message, "no file notes/missing.txt");
    // Validation sees a file as its name and runs before any file is read.
    let error = load_inputs(
        &schema,
        &[],
        flags(json!({"brief": "deep/missing.txt"})),
        &root,
    )
    .unwrap_err();
    assert!(error.message.starts_with("inputs: "), "{}", error.message);
    assert!(
        !error.message.contains(&root.to_string_lossy().into_owned()),
        "{}",
        error.message
    );
}

#[test]
fn bind_given_reads_files_and_validates() {
    let schema = compile_inputs(
        &map(json!({
            "name": {"type": "string"},
            "suffix": {"type": "string", "default": "!"},
            "pic": {"type": "file", "kind": "image"},
            "more": {"type": "files", "optional": true},
        })),
        None,
    )
    .unwrap();
    let file = |name: &str| FileValue {
        digest: "0".repeat(64),
        kind: "image/png".into(),
        name: name.into(),
        size: 1,
        key: None,
        content: None,
        location: None,
    };
    let seen = std::cell::RefCell::new(Vec::new());
    let mut read = |path: &str, kind: &str| -> std::result::Result<FileValue, String> {
        seen.borrow_mut().push(format!("{path} {kind}"));
        if path.starts_with("./") {
            Ok(file(path.trim_start_matches("./")))
        } else {
            Err(format!("no input file {path}"))
        }
    };
    let mut given = IndexMap::new();
    given.insert("name".to_string(), Val::Str("ada".into()));
    given.insert("pic".to_string(), Val::Str("./a.png".into()));
    let (bound, troubles) = bind_given(&schema, given, &mut read);
    assert!(troubles.is_empty(), "{troubles:?}");
    assert_eq!(
        bound.keys().collect::<Vec<_>>(),
        ["name", "pic", "suffix", "more"]
    );
    assert_eq!(bound["suffix"], Val::Str("!".into()));
    assert_eq!(bound["more"], Val::Null);
    assert!(matches!(&bound["pic"], Val::File(f) if f.name == "a.png"));
    assert_eq!(*seen.borrow(), ["./a.png image"]);

    // A binding failure gives the values back unbound, with the one trouble.
    let mut given = IndexMap::new();
    given.insert("pic".to_string(), Val::Str("lost.png".into()));
    let (bound, troubles) = bind_given(&schema, given.clone(), &mut read);
    assert_eq!(troubles, ["no input file lost.png"]);
    assert_eq!(bound, given);

    // Open values skip validation; files already bound are kept.
    let mut given = IndexMap::new();
    given.insert("pic".to_string(), Val::Failed("draw#1".into()));
    let (_, troubles) = bind_given(&schema, given, &mut read);
    assert!(troubles.is_empty());
    let mut given = IndexMap::new();
    given.insert("pic".to_string(), Val::File(Box::new(file("b.png"))));
    let (bound, troubles) = bind_given(&schema, given, &mut read);
    assert_eq!(troubles.len(), 1, "{troubles:?}");
    assert!(troubles[0].starts_with("inputs: "), "{troubles:?}");
    assert!(matches!(&bound["pic"], Val::File(f) if f.name == "b.png"));
}

// ------------------------------------------------------------------------------ with YAML

/// Tests that read documents with the strict YAML loader.
mod with_yaml {
    use super::*;
    use grida_fx_core::docs::lock::{LockFile, read_lock, render_lock};
    use grida_fx_core::docs::project::Project;
    use grida_fx_core::docs::takes::{TakeChoice, Takes, read_takes, render_takes, takes_path};
    use grida_fx_core::docs::workflow::load_workflow;
    use grida_fx_core::docs::{Schema, read_document};
    use grida_fx_core::inputs::resolve_ref;

    fn cases() -> Vec<PathBuf> {
        let mut cases: Vec<PathBuf> = std::fs::read_dir(conformance())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.join("in").is_dir())
            .collect();
        cases.sort();
        cases
    }

    #[test]
    fn every_conformance_workflow_project_and_lock_reads() {
        let mut workflows = 0;
        for case in cases() {
            let project = Project::find(&case.join("in")).unwrap();
            assert!(project.has_file, "{}", case.display());
            assert_eq!(project.root, case.join("in").canonicalize().unwrap());
            assert!(!project.document.routes.is_empty(), "{}", case.display());
            let folder = case.join("in/workflows");
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                let label = format!("workflows/{}", path.file_name().unwrap().to_string_lossy());
                let loaded = load_workflow(&path, &label, &label)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert_eq!(loaded.source, label);
                assert!(loaded.path.is_absolute());
                assert!(!loaded.workflow.steps.is_empty());
                compile_inputs(&loaded.workflow.inputs, None).unwrap();
                workflows += 1;
            }
            read_lock(&case.join("in")).unwrap();
        }
        assert!(workflows >= 24, "{workflows}");
        let lock = read_lock(&conformance().join("lock-drift/in")).unwrap();
        assert_eq!(
            lock.nodes["nodes/n.py#pinned@2"],
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn the_refused_yaml_strict_files() {
        let bad = conformance().join("yaml-strict/in/bad");
        let error = load_workflow(
            &bad.join("duplicate-step.yaml"),
            "bad/duplicate-step.yaml",
            "x",
        )
        .unwrap_err();
        assert!(error.message.contains("duplicate-step.yaml"), "{error}");
        let error =
            load_workflow(&bad.join("yes-value.yaml"), "bad/yes-value.yaml", "x").unwrap_err();
        assert!(
            error.message.contains("yes-value.yaml") && error.message.contains("quote it"),
            "{error}"
        );
        let error = read_document(
            &bad.join("routes-yes.yaml"),
            "bad/routes-yes.yaml",
            Schema::Routes,
        )
        .unwrap_err();
        assert!(
            error.message.contains("routes-yes.yaml") && error.message.contains("quote it"),
            "{error}"
        );
        // The duplicate-routes fx.yaml of the case's last step.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "fx.yaml",
            "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n  image.generate: img-a@acme\n",
        );
        let error = Project::find(dir.path()).unwrap_err();
        assert!(error.message.contains("fx.yaml"), "{error}");
    }

    #[test]
    fn the_yaml_strict_inputs_files() {
        let case = conformance().join("yaml-strict/in");
        let loaded = load_workflow(
            &case.join("workflows/case.yaml"),
            "workflows/case.yaml",
            "w",
        )
        .unwrap();
        let schema = compile_inputs(&loaded.workflow.inputs, None).unwrap();
        let load = |name: &str| {
            let label = format!("inputs/{name}");
            load_inputs(
                &schema,
                &[(case.join(&label), label)],
                IndexMap::new(),
                &case,
            )
        };
        let quoted = load("quoted.yaml").unwrap();
        assert_eq!(quoted.values["on"], Val::Str("off".into()));
        assert_eq!(quoted.values["answer"], Val::Str("yes".into()));
        assert_eq!(quoted.values["code"], Val::Str("017".into()));
        assert_eq!(quoted.values["when"], Val::Str("2026-10-05".into()));
        let plain = load("plain.yaml").unwrap();
        assert_eq!(plain.values["on"], Val::Str("y".into()));
        assert_eq!(plain.values["code"], Val::Str("0bad".into()));
        for name in [
            "yes.yaml",
            "off.yaml",
            "mixed-case.yaml",
            "octal.yaml",
            "leading-zero.yaml",
            "date.yaml",
            "sexagesimal.yaml",
            "ratio.yaml",
            "infinity.yaml",
        ] {
            let error = load(name).unwrap_err();
            assert!(
                error.message.contains(name) && error.message.contains("quote it"),
                "{name}: {error}"
            );
        }
        for name in ["duplicate.yaml", "anchor.yaml", "tag.yaml"] {
            let error = load(name).unwrap_err();
            assert!(error.message.contains(name), "{name}: {error}");
        }
    }

    #[test]
    fn the_numbers_inputs_mean_one_thing() {
        let case = conformance().join("numbers/in");
        let loaded = load_workflow(
            &case.join("workflows/case.yaml"),
            "workflows/case.yaml",
            "w",
        )
        .unwrap();
        let schema = compile_inputs(&loaded.workflow.inputs, None).unwrap();
        let load = |name: &str| {
            let label = format!("inputs/{name}");
            load_inputs(
                &schema,
                &[(case.join(&label), label)],
                IndexMap::new(),
                &case,
            )
        };
        let int = load("int.yaml").unwrap();
        let float = load("float.yaml").unwrap();
        assert_eq!(int, float);
        assert_eq!(int.values["count"], Val::Number(2.0));
        let error = load("huge.yaml").unwrap_err();
        assert!(error.message.contains("huge.yaml"), "{error}");
    }

    #[test]
    fn takes_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = takes_path(dir.path(), "case");
        // The yaml-strict case's two takes files.
        write(dir.path(), "case.takes.yaml", "draw: { take: yes }\n");
        let error = read_takes(&path, "workflows/case.takes.yaml").unwrap_err();
        assert!(
            error.message.contains("case.takes.yaml") && error.message.contains("quote it"),
            "{error}"
        );
        write(dir.path(), "case.takes.yaml", "");
        assert!(read_takes(&path, "case.takes.yaml").unwrap().is_empty());
        write(dir.path(), "case.takes.yaml", "- draw\n");
        assert_eq!(
            read_takes(&path, "case.takes.yaml").unwrap_err().message,
            "case.takes.yaml: a takes file maps step paths to takes"
        );
        // Written takes read back.
        let mut takes = Takes::new();
        takes.insert(
            "draw".into(),
            TakeChoice {
                take: 3,
                result: None,
            },
        );
        takes.insert(
            "count".into(),
            TakeChoice {
                take: 1,
                result: Some("123e4567aa".repeat(6) + "abcd"),
            },
        );
        takes.insert(
            "entity['it\\'s'].draw".into(),
            TakeChoice {
                take: 2,
                result: None,
            },
        );
        let text = render_takes("case.takes.yaml", &takes);
        assert!(text.starts_with(
            "# case.takes.yaml: written by `grida-fx reroll` and `grida-fx pick`; commit it\n"
        ));
        write(dir.path(), "case.takes.yaml", &text);
        let back = read_takes(&path, "case.takes.yaml").unwrap();
        assert_eq!(
            back.keys().collect::<Vec<_>>(),
            ["count", "draw", "entity['it\\'s'].draw"]
        );
        for (step, choice) in &takes {
            assert_eq!(&back[step], choice);
        }
    }

    #[test]
    fn lock_files_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut lock = LockFile::default();
        lock.nodes
            .insert("nodes/n.py#pinned@2".into(), "123e4567".repeat(8));
        lock.nodes.insert("nodes/a.py#b@1".into(), "0".repeat(64));
        let text = render_lock(&lock);
        assert!(
            text.starts_with("# fx.lock: the source behind each versioned node type; commit it\n")
        );
        write(dir.path(), "fx.lock", &text);
        assert_eq!(read_lock(dir.path()).unwrap(), lock);
        write(
            dir.path(),
            "fx.lock",
            "fx: lock/v1\nnodes:\n  nodes/n.py#x@1: abc\n",
        );
        let error = read_lock(dir.path()).unwrap_err();
        assert!(error.message.starts_with("fx.lock: "), "{error}");
        write(dir.path(), "fx.lock", "nodes: {}\n");
        assert_eq!(
            read_lock(dir.path()).unwrap_err().message,
            "fx.lock: a lock file starts with fx: lock/v1"
        );
    }

    #[test]
    fn find_walks_up_to_the_nearest_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root, "fx.yaml", "fx: project/v1\nruns: out\n");
        write(&root, "sub/fx.yaml", "fx: project/v1\ncache: c\n");
        write(&root, "sub/workflows/w.yaml", "x: 1\n");
        let inner = Project::find(&root.join("sub/workflows/w.yaml")).unwrap();
        assert_eq!(inner.root, root.join("sub"));
        assert_eq!(inner.document.cache, "c");
        let outer = Project::find(&root.join("other/missing")).unwrap();
        assert_eq!(outer.root, root);
        assert_eq!(outer.document.runs, "out");
        write(&root, "fx.yaml", "fx: project/v1\nnodes: ../x\n");
        assert_eq!(
            Project::find(&root).unwrap_err().message,
            "fx.yaml: nodes: nodes is a folder inside the project"
        );
    }

    #[test]
    fn refs_resolve_against_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        write(
            &home,
            "workflows/lib.yaml",
            "fx: workflow/v1\ninputs:\n  tone: { type: string, enum: [calm, wild] }\n  list: [1]\n",
        );
        assert_eq!(
            resolve_ref(&home, "workflows/lib.yaml#/inputs/tone").unwrap(),
            json!({"type": "string", "enum": ["calm", "wild"]})
        );
        assert_eq!(
            resolve_ref(&home, "workflows/lib.yaml#/inputs/list/0")
                .unwrap_err()
                .message,
            "$ref workflows/lib.yaml#/inputs/list/0 names nothing"
        );
        assert_eq!(
            resolve_ref(&home, "workflows/lib.yaml#inputs/tone")
                .unwrap_err()
                .message,
            "$ref workflows/lib.yaml#inputs/tone names nothing"
        );
        assert!(
            resolve_ref(&home, "../outside.yaml#/x")
                .unwrap_err()
                .message
                .contains("is outside the project")
        );
        let missing = resolve_ref(&home, "workflows/none.yaml#/x").unwrap_err();
        assert!(missing.message.contains("workflows/none.yaml"), "{missing}");
        assert!(
            !missing
                .message
                .contains(&home.to_string_lossy().into_owned()),
            "{missing}"
        );
        // Through compile_inputs, as the planner calls it.
        let mut resolver = |reference: &str| resolve_ref(&home, reference);
        let schema = compile_inputs(
            &map(json!({"tone": {"$ref": "workflows/lib.yaml#/inputs/tone", "optional": true}})),
            Some(&mut resolver),
        )
        .unwrap();
        assert_eq!(
            schema["properties"]["tone"],
            json!({"enum": ["calm", "wild"], "type": "string"})
        );
        assert!(schema.get("required").is_none());
    }

    #[test]
    fn inputs_files_merge_anchor_and_refuse_markers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root, "brief.txt", "one\ntwo\n");
        write(&root, "inputs/brief.txt", "inner\n");
        write(
            &root,
            "inputs/a.yaml",
            "brief: brief.txt\ncount: 1\nnames: [x]\n",
        );
        write(&root, "b.yaml", "count: 2\n");
        let schema = compile_inputs(
            &map(json!({
                "brief": {"type": "file"},
                "count": {"type": "integer"},
                "names": {"type": "list", "items": {"type": "string"}},
                "mood": {"type": "string", "optional": true},
            })),
            None,
        )
        .unwrap();
        let files = [
            (root.join("inputs/a.yaml"), "inputs/a.yaml".to_string()),
            (root.join("b.yaml"), "b.yaml".to_string()),
        ];
        let RootInputs { given, values } =
            load_inputs(&schema, &files, map(json!({"count": 3})), &root).unwrap();
        // Paths in a file are relative to that file; later sources replace a name in place.
        assert_eq!(
            given.keys().collect::<Vec<_>>(),
            ["brief", "count", "names"]
        );
        assert_eq!(
            values.keys().collect::<Vec<_>>(),
            ["brief", "count", "names", "mood"]
        );
        assert_eq!(given["count"], Val::Number(3.0));
        assert_eq!(values["mood"], Val::Null);
        let Val::File(brief) = &given["brief"] else {
            panic!("{:?}", given["brief"])
        };
        assert_eq!(brief.name, "brief.txt");
        assert_eq!(brief.size, 6);
        assert_eq!(
            brief.location.as_deref(),
            Some(root.join("inputs/brief.txt").as_path())
        );

        write(&root, "bad.yaml", "- 1\n");
        let error = load_inputs(
            &schema,
            &[(root.join("bad.yaml"), "bad.yaml".into())],
            IndexMap::new(),
            &root,
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "bad.yaml: an inputs file maps input names to values"
        );

        write(&root, "marker.yaml", "names: [{ missing: true }]\n");
        let error = load_inputs(
            &schema,
            &[(root.join("marker.yaml"), "marker.yaml".into())],
            IndexMap::new(),
            &root,
        )
        .unwrap_err();
        assert!(
            error.message.starts_with("marker.yaml: inputs.names[0]: "),
            "{error}"
        );

        write(
            &root,
            "lost.yaml",
            "brief: notes/lost.txt\ncount: 1\nnames: []\n",
        );
        let error = load_inputs(
            &schema,
            &[(root.join("inputs/../lost.yaml"), "./lost.yaml".into())],
            IndexMap::new(),
            &root,
        )
        .unwrap_err();
        assert_eq!(error.message, "no file ./notes/lost.txt");
    }
}

// ------------------------------------------------------------------------------ with files

/// Tests that bind files by content (file kinds and text decoding).
mod with_files {
    use super::*;
    use grida_fx_core::inputs::bind::read_input_file;
    use grida_fx_core::val::FileContent;

    #[test]
    fn files_are_read_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let text = write(&root, "note.txt", "\u{feff}Hello.\r\n");
        let file = read_input_file(&text, "note.txt", "file").unwrap();
        assert_eq!(file.kind, "text/plain");
        assert_eq!(file.name, "note.txt");
        assert_eq!(
            file.digest,
            "280c3a0354d21e33d2877ed5a2234b37f951bc270ad66004e0731b961bde1c96"
        );
        assert_eq!(file.content, Some(FileContent::Text("Hello.\r\n".into())));
        let json = write(&root, "data.json", "{\"b\": 1.0, \"a\": [true]}");
        let file = read_input_file(&json, "data.json", "image").unwrap();
        assert_eq!(file.kind, "json");
        assert_eq!(
            file.content,
            Some(FileContent::Json(json!({"b": 1, "a": [true]})))
        );
        // The declared kind names only what the suffix does not.
        let odd = write(&root, "blob", "x");
        assert_eq!(
            read_input_file(&odd, "blob", "image").unwrap().kind,
            "image"
        );
        let png = write(&root, "pic.png", "not really");
        let file = read_input_file(&png, "pic.png", "text").unwrap();
        assert_eq!(file.kind, "image/png");
        assert_eq!(file.content, None);
        // Reserved markers in JSON content, invalid JSON and invalid UTF-8 are refused.
        let marker = write(&root, "m.json", "{\"missing\": true}");
        let error = read_input_file(&marker, "m.json", "file").unwrap_err();
        assert!(error.message.starts_with("m.json"), "{error}");
        let broken = write(&root, "b.json", "{");
        assert!(read_input_file(&broken, "b.json", "file").is_err());
        let latin = root.join("l.txt");
        std::fs::write(&latin, [0xff, 0xfe, 0x41]).unwrap();
        assert!(read_input_file(&latin, "l.txt", "file").is_err());
        // A large text file is a file without content.
        let big = write(&root, "big.md", &"x".repeat(1_000_001));
        assert_eq!(
            read_input_file(&big, "big.md", "file").unwrap().content,
            None
        );
        let error = read_input_file(&root.join("none.txt"), "none.txt", "file").unwrap_err();
        assert_eq!(error.message, "no file none.txt");
    }

    #[test]
    fn files_inputs_are_keyed_by_stem() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root, "shots/a.png", "a");
        write(&root, "shots/b.png", "b");
        let schema = compile_inputs(
            &map(
                json!({"shots": {"type": "files", "kind": "image", "glob": true},
                        "brief": {"type": "file", "default": "shots/a.png"}}),
            ),
            None,
        )
        .unwrap();
        let inputs =
            load_inputs(&schema, &[], map(json!({"shots": ["shots/*.png"]})), &root).unwrap();
        let Val::List(files) = &inputs.values["shots"] else {
            panic!()
        };
        let keys: Vec<Option<String>> = files
            .iter()
            .map(|f| match f {
                Val::File(f) => f.key.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(keys, [Some("a".into()), Some("b".into())]);
        // A default naming a file is read relative to the working directory, and is not given.
        assert!(matches!(&inputs.values["brief"], Val::File(f) if f.name == "a.png"));
        assert!(!inputs.given.contains_key("brief"));
    }
}

// ------------------------------------------------------------------------------ with text

/// Message texts that quote values the way Python's `repr` does.
mod with_text {
    use super::*;

    #[test]
    fn validation_quotes_values_as_python_did() {
        let schema = compile_inputs(
            &map(json!({
                "count": {"type": "integer"},
                "style": {"tone": {"type": "string"}},
                "table": {"type": "map", "values": {"type": "integer"}},
                "tags": {"type": "list", "items": {"type": "string"}},
            })),
            None,
        )
        .unwrap();
        let value = json!({
            "count": true,
            "style": {"tone": 5, "extra": 1},
            "table": {"a": "x"},
            "tags": [1, "ok", 2],
        });
        assert_eq!(
            validate(&schema, &value).unwrap_err(),
            "inputs.count: True is not of type 'integer'; \
             inputs.style: Additional properties are not allowed ('extra' was unexpected); \
             inputs.style.tone: 5 is not of type 'string'; \
             inputs.table.a: 'x' is not of type 'integer'; \
             inputs.tags[0]: 1 is not of type 'string'; \
             inputs.tags[2]: 2 is not of type 'string'"
        );
    }
}
