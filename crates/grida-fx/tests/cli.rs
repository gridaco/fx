//! The `grida-fx` command, run as a process in temporary projects.
//!
//! Tests whose name starts with `integrated_` plan, describe or lock a real project, so they hold
//! the whole engine (documents, inputs, routes, expansion, planning) to the command line; the
//! others pin the command line itself. No test needs Python: the projects here have no node
//! modules.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root (two folders above this crate).
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Runs `grida-fx` in `cwd` with a minimal environment.
fn grida_fx(cwd: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command.args(args).current_dir(cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("HOME", cwd).env("NO_COLOR", "1");
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn status(output: &Output) -> i32 {
    output.status.code().expect("an exit status")
}

/// A temporary project holding only `fx.yaml`.
fn empty_project() -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("fx.yaml"), "fx: project/v1\n").unwrap();
    folder
}

fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temporary copy of a conformance case's project.
fn conformance_project(case: &str) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    copy_folder(
        &repository().join("conformance").join(case).join("in"),
        folder.path(),
    );
    folder
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn no_arguments_is_a_usage_error() {
    let project = empty_project();
    let output = grida_fx(project.path(), &[]);
    assert_eq!(status(&output), 2);
    assert!(stdout(&output).is_empty());
    assert!(!stderr(&output).is_empty());
}

#[test]
fn an_unknown_verb_is_a_usage_error() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["view"]);
    assert_eq!(status(&output), 2);
    assert!(stderr(&output).contains("view"), "{}", stderr(&output));
}

#[test]
fn help_and_version_exit_zero() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["--help"]);
    assert_eq!(status(&output), 0);
    let help = stdout(&output);
    for verb in [
        "plan", "expand", "identity", "price", "schema", "nodes", "doctor", "lock",
    ] {
        assert!(help.contains(verb), "{help}");
    }
    let output = grida_fx(project.path(), &["plan", "--help"]);
    assert_eq!(status(&output), 0);
    assert!(stdout(&output).contains("--max-usd"));
    let output = grida_fx(project.path(), &["--version"]);
    assert_eq!(status(&output), 0);
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn a_planning_verb_needs_its_target() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["plan"]);
    assert_eq!(status(&output), 2);
    assert!(stderr(&output).contains("<TARGET>"), "{}", stderr(&output));
}

#[test]
fn other_verbs_take_no_input_flags() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["schema", "case", "--name", "Ada"]);
    assert_eq!(status(&output), 2);
    let output = grida_fx(project.path(), &["lock", "--nosuch"]);
    assert_eq!(status(&output), 2);
}

#[test]
fn verbs_of_the_runner_are_not_available_yet() {
    let project = empty_project();
    let cases: [(&str, &[&str]); 7] = [
        ("run", &["case", "--live", "--run", "runs/one"]),
        ("reroll", &["runs/one", "draw"]),
        ("pick", &["runs/one", "draw", "2"]),
        ("takes", &["list", "case"]),
        ("jobs", &["--forget", "k0"]),
        ("project", &["runs/one"]),
        ("inspect", &[]),
    ];
    for (verb, args) in cases {
        let mut argv = vec![verb];
        argv.extend_from_slice(args);
        let output = grida_fx(project.path(), &argv);
        assert_eq!(status(&output), 2, "{verb}");
        assert!(stdout(&output).is_empty(), "{verb}");
        assert_eq!(
            stderr(&output),
            format!("grida-fx: {verb} is not available until the runner lands\n")
        );
    }
}

#[test]
fn a_bad_builder_argument_is_a_usage_error() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["plan", "case", "--arg", "novalue"]);
    assert_eq!(status(&output), 2);
    assert_eq!(
        stderr(&output),
        "grida-fx: --arg novalue: write NAME=VALUE\n"
    );
}

#[test]
fn a_ceiling_that_is_not_an_amount_is_a_usage_error() {
    let project = empty_project();
    for amount in ["nan", "inf", "-1", "ten"] {
        let output = grida_fx(
            project.path(),
            &["plan", "case", &format!("--max-usd={amount}")],
        );
        assert_eq!(status(&output), 2, "{amount}");
        assert!(
            stderr(&output).starts_with(&format!("grida-fx: --max-usd {amount}: ")),
            "{}",
            stderr(&output)
        );
    }
}

#[test]
fn integrated_nodes_lists_every_built_in() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["nodes"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 37, "{text}");
    assert_eq!(lines[0], format!("{:34} free", "fx/annotations.filter@1"));
    assert_eq!(lines[36], format!("{:34} judge", "fx/vision.review@1"));
    let kinds = |kind: &str| {
        lines
            .iter()
            .filter(|l| l.ends_with(&format!(" {kind}")))
            .count()
    };
    assert_eq!((kinds("paid"), kinds("judge"), kinds("free")), (11, 5, 21));
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(sorted, lines, "sorted by name");
}

#[test]
fn integrated_nodes_shows_one_type() {
    let project = empty_project();
    let expected = format!(
        "{:34} paid\n\
         \x20 input   references: image[]?\n\
         \x20 setting prompt: {{\"type\": \"string\", \"x-fx-template\": true}}\n\
         \x20 setting size: {{\"type\": \"string\", \"x-fx-optional\": true}}\n\
         \x20 setting background: {{\"default\": \"auto\", \"enum\": [\"opaque\", \"transparent\", \"auto\"]}}\n\
         \x20 setting vars: {{\"default\": {{}}, \"type\": \"object\"}}\n\
         \x20 output  image: image/png\n\
         \x20 routes  none installed\n",
        "fx/image.generate@1"
    );
    for name in ["image.generate", "fx/image.generate@1"] {
        let output = grida_fx(project.path(), &["nodes", name]);
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        assert_eq!(stdout(&output), expected);
    }
    let output = grida_fx(project.path(), &["nodes", "fx/select@1"]);
    assert_eq!(
        stdout(&output),
        format!(
            "{:34} free\n  setting first_of: {{\"type\": \"array\"}}\n  output  value: file?\n  about   fx/select: the first candidate that exists and was not rejected.\n",
            "fx/select@1"
        )
    );
}

#[test]
fn integrated_nodes_shows_the_projects_routes() {
    let project = empty_project();
    std::fs::write(
        project.path().join("fx.yaml"),
        "fx: project/v1\nroute_tables: [routes.yaml]\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join("routes.yaml"),
        "fx: routes/v1\nroutes:\n  - capability: image.generate\n    route: img-b@acme\n    price: { low_usd: 0.01, high_usd: 0.04 }\n  - capability: image.generate\n    route: img-a@acme\n    price: { low_usd: 0.01, high_usd: 0.04 }\n",
    )
    .unwrap();
    let output = grida_fx(project.path(), &["nodes", "image.generate"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).ends_with("  routes  img-a@acme, img-b@acme\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_nodes_of_an_unknown_type_prints_nothing() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["nodes", "nosuch"]);
    assert_eq!(status(&output), 0);
    assert!(stdout(&output).is_empty());
}

#[test]
fn integrated_schema_of_a_workflows_inputs() {
    let project = empty_project();
    std::fs::create_dir(project.path().join("workflows")).unwrap();
    std::fs::write(
        project.path().join("workflows/case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Linear\ninputs:\n  brief: { type: file, kind: text/plain }\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    with: { prompt: \"${{ inputs.brief }}\" }\n",
    )
    .unwrap();
    let expected = "{\n \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n \"additionalProperties\": false,\n \"properties\": {\n  \"brief\": {\n   \"type\": \"string\",\n   \"x-fx-file\": {\n    \"kind\": \"text/plain\"\n   }\n  }\n },\n \"required\": [\n  \"brief\"\n ],\n \"type\": \"object\"\n}\n";
    for target in ["case", "workflows/case.yaml"] {
        let output = grida_fx(project.path(), &["schema", target]);
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        assert_eq!(stdout(&output), expected);
    }
    let output = grida_fx(project.path(), &["schema", "nosuch"]);
    assert_eq!(status(&output), 2);
    assert!(stderr(&output).starts_with("grida-fx: "));
}

#[test]
fn integrated_lock_check_without_node_types() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["lock", "--check"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "fx.lock: 0 versioned node types, all locked\n"
    );
    assert!(!project.path().join("fx.lock").exists());
    let output = grida_fx(project.path(), &["lock"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output), "fx.lock: 0 versioned node types\n");
    let written = std::fs::read_to_string(project.path().join("fx.lock")).unwrap();
    assert!(written.starts_with("# fx.lock: "), "{written}");
}

#[test]
fn integrated_doctor_without_a_target() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["doctor"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(
        lines[0],
        format!("engine    grida-fx {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(lines[1].starts_with("python    "), "{text}");
}

#[test]
fn integrated_plan_text_of_tiered_price() {
    let project = conformance_project("tiered-price");
    let output = grida_fx(project.path(), &["plan", "case", "--routes", "routes.yaml"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "case  ·  1 phase\n\
         phase 1   3 steps   3 provider calls   $2.49 – $6.86\n\
         cached    0 of 3 known steps\n\
         estimate  $2.49 – $6.86\n"
    );
    let output = grida_fx(
        project.path(),
        &[
            "plan",
            "case",
            "--routes",
            "routes.yaml",
            "--max-usd",
            "1",
            "--check",
        ],
    );
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).ends_with(
        "estimate  $2.49 – $6.86   ceiling $1.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it\n"
    ));
}

#[test]
fn integrated_plan_expect_cached_names_what_is_not() {
    let project = conformance_project("tiered-price");
    let output = grida_fx(
        project.path(),
        &["plan", "case", "--routes=routes.yaml", "--expect-cached"],
    );
    assert_eq!(status(&output), 1);
    assert!(
        stdout(&output).ends_with("\nnot cached: small#1, large#1, unknown#1\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_a_refused_plan() {
    let project = conformance_project("tiered-price");
    // Without --routes the catalog is the built-in table, which serves nothing yet.
    let output = grida_fx(project.path(), &["plan", "case"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\nrefused   small.route: "),
        "{}",
        stdout(&output)
    );
    let output = grida_fx(project.path(), &["plan", "case", "--check"]);
    assert_eq!(status(&output), 1);
    let output = grida_fx(project.path(), &["expand", "case"]);
    assert_eq!(status(&output), 1);
    let graph = parse(&stdout(&output));
    assert!(!graph["problems"].as_array().unwrap().is_empty());
}

#[test]
fn integrated_documents_of_tiered_price() {
    let project = conformance_project("tiered-price");
    let routes = ["--routes", "routes.yaml", "--inputs", "inputs.yaml"];
    let run = |verb: &str| {
        let mut argv = vec![verb, "case"];
        argv.extend_from_slice(&routes);
        let output = grida_fx(project.path(), &argv);
        assert_eq!(status(&output), 0, "{verb}: {}", stderr(&output));
        parse(&stdout(&output))
    };
    let price = run("price");
    assert_eq!(
        price,
        json!({
            "phases": [{
                "phase": 1, "steps": 3, "calls": [3, 3],
                "low_usd": 2.49, "high_usd": 6.8625, "then": [],
            }],
            "estimate": {"low_usd": 2.49, "high_usd": 6.8625},
            "ceiling_usd": null,
        })
    );
    let identity = run("identity");
    let ids: Vec<&String> = identity.as_object().unwrap().keys().collect();
    assert_eq!(ids, ["large#1", "small#1", "unknown#1"]);
    let graph = run("expand");
    let schema: Value = parse(include_str!(
        "../../../spec/schemas/fx-graph-v1.schema.json"
    ));
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&graph)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(graph["workflow"]["file"], "workflows/case.yaml");
    assert_eq!(
        graph["types"],
        json!({"fx/video.generate@1": {"identity": "fx/video.generate@1.1"}})
    );
    let mut json_plan = vec!["plan", "case", "--json"];
    json_plan.extend_from_slice(&routes);
    let output = grida_fx(project.path(), &json_plan);
    assert_eq!(
        parse(&stdout(&output)),
        graph,
        "plan --json prints the graph"
    );
}

#[test]
fn integrated_unknown_input_flags_are_refused() {
    let project = conformance_project("tiered-price");
    let output = grida_fx(
        project.path(),
        &["expand", "case", "--routes", "routes.yaml", "--check"],
    );
    assert_eq!(status(&output), 2);
    assert_eq!(
        stderr(&output),
        "grida-fx: unknown input flag --check; lists and maps of case come from --inputs\n"
    );
}

#[test]
fn a_negative_ceiling_reaches_fxs_own_message() {
    let project = empty_project();
    for amount in ["-1", "-0.5", "-1e3"] {
        let output = grida_fx(project.path(), &["plan", "case", "--max-usd", amount]);
        assert_eq!(status(&output), 2, "{amount}");
        assert_eq!(
            stderr(&output),
            format!("grida-fx: --max-usd {amount}: {amount} is negative\n")
        );
    }
}

#[test]
fn integrated_a_repeated_option_takes_its_last_value() {
    let project = conformance_project("tiered-price");
    let plan = |extra: &[&str]| {
        let mut argv = vec!["plan", "case", "--routes", "routes.yaml"];
        argv.extend_from_slice(extra);
        grida_fx(project.path(), &argv)
    };
    for (first, last) in [("1", "2"), ("2", "1")] {
        let output = plan(&["--max-usd", first, "--max-usd", last]);
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        assert!(
            stdout(&output).contains(&format!("ceiling ${last}.00")),
            "{}",
            stdout(&output)
        );
    }
    let output = plan(&["--check", "--check", "--json", "--json"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).starts_with('{'));
    let output = plan(&["--expect-cached", "--expect-cached"]);
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    assert!(stdout(&output).ends_with("\nnot cached: small#1, large#1, unknown#1\n"));
    // Repeatable options still keep every value: the missing first file is read and refused.
    let output = plan(&["--inputs", "missing.yaml", "--inputs", "inputs.yaml"]);
    assert_eq!(status(&output), 2);
    assert!(
        stderr(&output).contains("missing.yaml"),
        "{}",
        stderr(&output)
    );
    let output = grida_fx(project.path(), &["lock", "--check", "--check"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
}

#[test]
fn integrated_lock_finds_the_project_from_where() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("proj");
    std::fs::create_dir_all(project.join("workflows/sub")).unwrap();
    std::fs::write(project.join("fx.yaml"), "fx: project/v1\n").unwrap();
    for place in ["proj", "proj/workflows/sub", "proj/fx.yaml"] {
        let output = grida_fx(parent.path(), &["lock", place, "--check"]);
        assert_eq!(status(&output), 0, "{place}: {}", stderr(&output));
        assert_eq!(
            stdout(&output),
            "fx.lock: 0 versioned node types, all locked\n"
        );
    }
    assert!(!project.join("fx.lock").exists());
    let output = grida_fx(parent.path(), &["lock", "--check", "proj/workflows"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let output = grida_fx(parent.path(), &["lock", "proj/workflows"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output), "fx.lock: 0 versioned node types\n");
    let written = std::fs::read_to_string(project.join("fx.lock")).unwrap();
    assert!(written.starts_with("# fx.lock: "), "{written}");
    assert!(!parent.path().join("fx.lock").exists());
    assert!(!project.join("workflows/fx.lock").exists());
    // Without `where`, the working directory's project.
    let output = grida_fx(&project.join("workflows"), &["lock", "--check"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    // A `where` that is not there is refused, not resolved to a project above it.
    let output = grida_fx(&project, &["lock", "nowhere", "--check"]);
    assert_eq!(status(&output), 2);
    assert_eq!(stderr(&output), "grida-fx: nowhere: no such file\n");
    assert!(stdout(&output).is_empty());
    let output = grida_fx(&project, &["lock", "a", "b"]);
    assert_eq!(status(&output), 2);
}

#[test]
fn integrated_a_workflow_found_by_id_reports_why_it_is_refused() {
    let project = conformance_project("tiered-price");
    let file = project.path().join("workflows/case.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    let broken = text.replacen(
        "      prompt: a lantern sways\n",
        "      prompt: a lantern sways\n      aspect_ratio: 16:9\n",
        1,
    );
    assert_ne!(broken, text);
    std::fs::write(&file, broken).unwrap();
    let message = "workflows/case.yaml:10:21: plain scalar '16:9' is ambiguous (a sexagesimal number); quote it";
    for target in ["case", "workflows/case.yaml"] {
        let output = grida_fx(project.path(), &["plan", target, "--routes", "routes.yaml"]);
        assert_eq!(status(&output), 2, "{target}");
        assert_eq!(
            stderr(&output),
            format!("grida-fx: {message}\n"),
            "{target}"
        );
    }
}

/// A workflow whose first step reads the end of a chain of `length` steps, each reading the one
/// before it: expansion recurses through the whole chain.
fn chain_project(length: usize) -> tempfile::TempDir {
    let folder = empty_project();
    let mut text = String::from(
        "fx: workflow/v1\nid: chain\ntitle: Chain\n\
         inputs:\n  picture: { type: file, kind: image/png }\nsteps:\n",
    );
    let step = |name: &str, source: &str| {
        format!(
            "  {name}:\n    uses: fx/image.resize@1\n    \
             with: {{ image: \"${{{{ {source} }}}}\", longest_side: 64 }}\n"
        )
    };
    text.push_str(&step(
        "last",
        &format!("steps.s{}.outputs.image", length - 1),
    ));
    for index in 0..length {
        let source = match index {
            0 => "inputs.picture".to_string(),
            _ => format!("steps.s{}.outputs.image", index - 1),
        };
        text.push_str(&step(&format!("s{index}"), &source));
    }
    std::fs::create_dir(folder.path().join("workflows")).unwrap();
    std::fs::write(folder.path().join("workflows/chain.yaml"), text).unwrap();
    std::fs::write(folder.path().join("picture.png"), b"x").unwrap();
    folder
}

#[test]
fn integrated_a_long_chain_of_steps_plans_without_overflowing_the_stack() {
    // 250 steps overflowed the main thread's stack in a debug build.
    let project = chain_project(1000);
    let output = grida_fx(
        project.path(),
        &["plan", "chain", "--picture", "picture.png"],
    );
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\nphase 1   1001 steps   0 provider calls   $0.00\n"),
        "{}",
        stdout(&output)
    );
}
