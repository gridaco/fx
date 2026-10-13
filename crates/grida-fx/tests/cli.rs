//! The `grida-fx` command, run as a process in temporary projects.
//!
//! Tests whose name starts with `integrated_` plan, describe, lock or run a real project, so they
//! hold the whole engine (documents, inputs, routes, expansion, planning, running) to the command
//! line; the others pin the command line itself. No test needs the `grida` package or Node: the
//! projects here have no node modules, or programs of their own (shell scripts, and stdlib Python
//! fakes that skip without `python3`) answer as an interpreter or a Node, except the end-to-end
//! runs of conformance projects, which skip without a Python that has the `grida` package
//! (`GRIDA_FX_PYTHON`, else `python/.venv/bin/python`).

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

/// The variables no child inherits: the six provider keys and the five base URLs
/// (spec/providers.md §3).
const PROVIDER_VARIABLES: [&str; 11] = [
    "GG_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "FAL_KEY",
    "TRIPO_API_KEY",
    "ELEVENLABS_API_KEY",
    "OPENAI_BASE_URL",
    "OPENROUTER_BASE_URL",
    "FAL_BASE_URL",
    "ELEVENLABS_BASE_URL",
    "GG_BASE_URL",
];

/// `grida-fx` in `cwd` with a minimal environment: no provider key or base URL, the network off
/// (`--live` builds its adapters over a transport that sends nothing) and the `.env` file off.
fn command(cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command.args(args).current_dir(cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("HOME", cwd)
        .env("NO_COLOR", "1")
        .env("GRIDA_FX_NETWORK", "off")
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        // No Codex, whatever this machine has installed.
        .env("GRIDA_FX_TOOL_CODEX", cwd.join("no-codex"));
    for name in PROVIDER_VARIABLES {
        command.env_remove(name);
    }
    command
}

/// Runs `grida-fx` in `cwd` with a minimal environment ([`command`]).
fn grida_fx(cwd: &Path, args: &[&str]) -> Output {
    command(cwd, args).output().unwrap()
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
    let output = grida_fx(project.path(), &["no-such-verb"]);
    assert_eq!(status(&output), 2);
    assert!(
        stderr(&output).contains("no-such-verb"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn help_and_version_exit_zero() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["--help"]);
    assert_eq!(status(&output), 0);
    let help = stdout(&output);
    for verb in [
        "init", "start", "status", "stop", "logs", "plan", "expand", "identity", "price", "schema",
        "nodes", "doctor", "lock",
    ] {
        assert!(help.contains(verb), "{help}");
    }
    assert!(
        !help
            .lines()
            .any(|line| line.trim_start().starts_with("view ")),
        "{help}"
    );
    let output = grida_fx(project.path(), &["view", "--help"]);
    assert_eq!(status(&output), 0);
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
         \x20 routes  gpt-image-2.5-sunburst@openai, imagegen@codex, \
         openai/gpt-image-2.5-sunburst@gg, openai/gpt-image-2.5-sunburst@openrouter, \
         openai/gpt-image-2.5/sunburst@fal\n",
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
        stdout(&output).ends_with(
            "  routes  gpt-image-2.5-sunburst@openai, imagegen@codex, img-a@acme, img-b@acme, \
             openai/gpt-image-2.5-sunburst@gg, openai/gpt-image-2.5-sunburst@openrouter, \
             openai/gpt-image-2.5/sunburst@fal\n"
        ),
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
    // engine, python, node, a key per provider, a route per built-in route.
    assert_eq!(lines.len(), 3 + 6 + 27, "{text}");
    assert_eq!(
        lines[0],
        format!("engine    grida-fx {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(lines[1].starts_with("python    "), "{text}");
    // What this machine's Node says; never a private path. A project without a package.json
    // starts no TypeScript host.
    assert!(lines[2].starts_with("node      "), "{text}");
    assert!(
        !lines[2].contains("@grida/fx") && !lines[2].contains("cannot start"),
        "{text}"
    );
    assert_eq!(
        lines[3..9],
        [
            "key       GG_API_KEY missing",
            "key       OPENAI_API_KEY missing",
            "key       OPENROUTER_API_KEY missing",
            "key       FAL_KEY missing",
            "key       TRIPO_API_KEY missing",
            "key       ELEVENLABS_API_KEY missing",
        ]
    );
    let routes = &lines[9..];
    assert!(routes.iter().all(|l| l.starts_with("route     ")), "{text}");
    let codex = [
        "route     image.edit imagegen@codex not installed (codex)",
        "route     image.generate imagegen@codex not installed (codex)",
    ];
    assert!(
        routes
            .iter()
            .all(|l| l.contains(" no key (") || codex.contains(l)),
        "{text}"
    );
    for line in codex {
        assert!(routes.contains(&line), "{text}");
    }
    let mut sorted = routes.to_vec();
    sorted.sort();
    assert_eq!(sorted, routes, "sorted by capability, then id");
}

/// A made-up key value, distinctive enough that finding it anywhere is a leak.
const MADE_UP_KEY: &str = "fx-made-up-key-3f9a1c";

/// Asserts that `MADE_UP_KEY` appears in neither output stream.
fn assert_no_key(output: &Output) {
    assert!(!stdout(output).contains(MADE_UP_KEY), "{}", stdout(output));
    assert!(!stderr(output).contains(MADE_UP_KEY), "{}", stderr(output));
}

#[test]
fn integrated_doctor_says_which_keys_are_present_and_never_their_values() {
    let project = empty_project();
    let output = command(project.path(), &["doctor"])
        .env("FAL_KEY", MADE_UP_KEY)
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_no_key(&output);
    let text = stdout(&output);
    let keys: Vec<&str> = text.lines().filter(|l| l.starts_with("key ")).collect();
    assert_eq!(
        keys,
        [
            "key       GG_API_KEY missing",
            "key       OPENAI_API_KEY missing",
            "key       OPENROUTER_API_KEY missing",
            "key       FAL_KEY present (environment)",
            "key       TRIPO_API_KEY missing",
            "key       ELEVENLABS_API_KEY missing",
        ]
    );
    assert!(
        text.contains(
            "\nroute     video.generate google/gemini-omni-flash/v1.1/image-to-video@fal \
             servable\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("\nroute     mesh.rig v1.0-20240301@tripo no key (TRIPO_API_KEY)\n"),
        "{text}"
    );
    // A blank value is no key.
    let output = command(project.path(), &["doctor"])
        .env("FAL_KEY", "   ")
        .output()
        .unwrap();
    assert!(stdout(&output).contains("\nkey       FAL_KEY missing\n"));
}

#[test]
fn integrated_doctor_lists_the_projects_routes() {
    let project = drawing_project(
        "fx: project/v1\nroute_tables: [routes.yaml]\nroutes:\n  image.generate: img-a@acme\n",
    );
    for argv in [&["doctor"][..], &["doctor", "case"][..]] {
        let output = grida_fx(project.path(), argv);
        let text = stdout(&output);
        if argv.len() == 1 {
            assert_eq!(status(&output), 0, "{argv:?}: {}", stderr(&output));
            assert!(!text.contains("\nuses "), "{text}");
        } else {
            // The target's one route leads, and no adapter serves it: a live run could not.
            assert_eq!(status(&output), 1, "{argv:?}: {}", stderr(&output));
            assert_eq!(
                text.lines().nth(3),
                Some("uses      image.generate img-a@acme no adapter (1 step)"),
                "{text}"
            );
        }
        let images: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("route     image.generate "))
            .collect();
        assert_eq!(
            images,
            [
                "route     image.generate gpt-image-2.5-sunburst@openai no key (OPENAI_API_KEY)",
                "route     image.generate imagegen@codex not installed (codex)",
                "route     image.generate img-a@acme no adapter",
                "route     image.generate openai/gpt-image-2.5-sunburst@gg no key \
                 (GG_API_KEY)",
                "route     image.generate openai/gpt-image-2.5-sunburst@openrouter no key \
                 (OPENROUTER_API_KEY)",
                "route     image.generate openai/gpt-image-2.5/sunburst@fal no key (FAL_KEY)",
            ],
            "{argv:?}"
        );
        assert_eq!(text.lines().filter(|l| l.starts_with("route ")).count(), 28);
    }
}

#[test]
fn integrated_doctor_names_the_planning_projects_interpreter_after_the_homes() {
    let project = drawing_project(&format!("{ROUTED}workflows: [library]\n"));
    let root = project.path();
    let home = root.join("library/thing");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::rename(root.join("workflows/case.yaml"), home.join("case.yaml")).unwrap();
    // The planning project's interpreter, which cannot start a host.
    let bin = root.join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("python"), "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(
        bin.join("python"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let output = grida_fx(root, &["doctor", "case"]);
    // `img-a@acme` is in no catalog here: a live run could not serve it.
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert_eq!(
        text.lines().nth(1),
        Some(
            "python    ../../.venv/bin/python cannot start: the Python node host \
             (../../.venv/bin/python) exited with status 3 before it answered initialize; set \
             GRIDA_FX_PYTHON to a Python that has the grida package"
        ),
        "{text}"
    );
    assert_eq!(
        text.lines().nth(3),
        Some("uses      image.generate img-a@acme not in the catalog (1 step)"),
        "{text}"
    );
    // The home's own interpreter comes first.
    let bin = home.join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(root.join(".venv/bin/python"), bin.join("python")).unwrap();
    let text = stdout(&grida_fx(root, &["doctor", "case"]));
    assert!(
        text.lines()
            .nth(1)
            .is_some_and(|line| line.starts_with("python    .venv/bin/python cannot start: ")),
        "{text}"
    );
}

#[test]
fn integrated_doctor_puts_the_targets_routes_first() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(
        root.join("fx.yaml"),
        "fx: project/v1\nroutes:\n  image.generate: imagegen@codex\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("workflows")).unwrap();
    std::fs::write(
        root.join("workflows/case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  draw:\n    uses: \
         fx/image.generate@1\n    with: { prompt: a kite }\n  more:\n    steps:\n      again:\n        \
         uses: fx/image.generate@1\n        with: { prompt: a boat }\n  keyed:\n    uses: \
         fx/image.generate@1\n    route: openai/gpt-image-2.5-sunburst@gg\n    with: { prompt: a \
         cat }\n  inner:\n    uses: ./workflows/inner.yaml\n",
    )
    .unwrap();
    std::fs::write(
        root.join("workflows/inner.yaml"),
        "fx: workflow/v1\nid: inner\ntitle: Inner\nsteps:\n  paint:\n    uses: \
         fx/image.generate@1\n    with: { prompt: a fish }\n",
    )
    .unwrap();
    let doctor = |codex: &Path| {
        command(root, &["doctor", "case"])
            .env("GG_API_KEY", MADE_UP_KEY)
            .env("GRIDA_FX_TOOL_CODEX", codex)
            .output()
            .unwrap()
    };
    // No Codex: three steps, one in a group and one in a used workflow, cannot be served.
    let output = doctor(&root.join("no-codex"));
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    assert_no_key(&output);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[3..5],
        [
            "uses      image.generate imagegen@codex not installed (codex) (3 steps)",
            "uses      image.generate openai/gpt-image-2.5-sunburst@gg servable (1 step)",
        ],
        "{text}"
    );
    assert!(lines[5].starts_with("key       "), "{text}");
    // The full catalog still follows.
    assert!(
        text.contains("\nroute     image.generate imagegen@codex not installed (codex)\n"),
        "{text}"
    );
    // With Codex installed (doctor only looks for it), every route the target uses is servable.
    let codex = root.join("codex");
    grida_fx_providers::testing::write_program(&codex, "#!/bin/sh\nexit 0\n");
    let output = doctor(&codex);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output).lines().nth(3),
        Some("uses      image.generate imagegen@codex servable (3 steps)")
    );
}

/// A project whose workflow uses a Python node, and an interpreter at `python` that answers the
/// version check with `answer` and otherwise, as Python before 3.11 does with `-P`, prints usage
/// text and exits 2, leaving `started.txt` behind.
fn interpreter_project(answer: &str) -> (tempfile::TempDir, PathBuf) {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::create_dir(root.join("nodes")).unwrap();
    std::fs::write(root.join("nodes/shout.py"), "").unwrap();
    std::fs::write(
        root.join("case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  shout:\n    uses: \
         ./nodes/shout.py#shout\n",
    )
    .unwrap();
    let python = root.join("old-python");
    grida_fx_providers::testing::write_program(
        &python,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = -c ]; then echo '{answer}'; exit 0; fi\necho 'Unknown option: \
             -P' >&2\necho usage: python3 >&2\ntouch \"$(dirname \"$0\")/started.txt\"\nexit 2\n"
        ),
    );
    (project, python)
}

#[test]
fn integrated_a_python_before_3_11_is_refused_before_it_starts() {
    let (project, python) = interpreter_project("fx-python 3.9.6 1");
    let root = project.path();
    let output = command(root, &["doctor"])
        .env("GRIDA_FX_PYTHON", &python)
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output).lines().nth(1),
        // Inside the project, the interpreter is named relative to it, on every platform.
        Some("python    old-python too old (3.9.6; FX needs 3.11+)")
    );
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let output = command(root, &["plan", "case.yaml"])
        .env("GRIDA_FX_PYTHON", &python)
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "grida-fx: old-python is Python 3.9.6; FX's node host needs Python 3.11 or later with \
         grida: run uv sync or pip install grida in the project, or set GRIDA_FX_PYTHON\n"
    );
    assert!(!root.join("started.txt").exists(), "the host was started");
}

#[test]
fn integrated_doctor_says_when_the_interpreter_has_no_grida() {
    let (project, python) = interpreter_project("fx-python 3.12.4 0");
    let output = command(project.path(), &["doctor"])
        .env("GRIDA_FX_PYTHON", &python)
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output).lines().nth(1),
        Some("python    old-python no grida")
    );
    assert!(!project.path().join("started.txt").exists());
    // A program that does not exist is still NOT FOUND.
    let missing = project.path().join("no-such-python");
    let output = command(project.path(), &["doctor"])
        .env("GRIDA_FX_PYTHON", &missing)
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output).lines().nth(1),
        Some("python    no-such-python NOT FOUND")
    );
}

/// A project whose workflow uses a TypeScript node (with a `package.json`), and a Node at `node`
/// that answers the version check with `answer` and otherwise prints usage text and exits 2,
/// leaving `started.txt` behind. With `sdk`, the project has `@grida/fx` with its host program.
fn node_project(answer: &str, sdk: bool) -> (tempfile::TempDir, PathBuf) {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(root.join("package.json"), "{\"type\": \"module\"}\n").unwrap();
    std::fs::create_dir(root.join("nodes")).unwrap();
    std::fs::write(root.join("nodes/shout.ts"), "").unwrap();
    std::fs::write(
        root.join("case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  shout:\n    uses: \
         ./nodes/shout.ts#shout\n",
    )
    .unwrap();
    if sdk {
        install_sdk(root, true);
    }
    let node = root.join("old-node");
    grida_fx_providers::testing::write_program(
        &node,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = -e ]; then echo '{answer}'; exit 0; fi\necho 'bad option' \
             >&2\ntouch \"$(dirname \"$0\")/started.txt\"\nexit 2\n"
        ),
    );
    (project, node)
}

/// Writes `@grida/fx` into `folder/node_modules`, with its host program when `entry`.
fn install_sdk(folder: &Path, entry: bool) {
    let package = folder.join("node_modules/@grida/fx");
    std::fs::create_dir_all(package.join("dist")).unwrap();
    std::fs::write(package.join("package.json"), "{\"name\": \"@grida/fx\"}").unwrap();
    if entry {
        std::fs::write(package.join("dist/host-entry.js"), "").unwrap();
    }
}

#[test]
fn integrated_a_node_before_24_is_refused_before_it_starts() {
    let (project, node) = node_project("fx-node 22.12.0 1", true);
    let root = project.path();
    let output = command(root, &["doctor"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output).lines().nth(2),
        // Inside the project, Node is named relative to it.
        Some("node      old-node too old (22.12.0; FX needs 24+)")
    );
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let output = command(root, &["plan", "case.yaml"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "grida-fx: old-node is Node 22.12.0; FX's TypeScript node host needs Node 24 or later: \
         install it, or set GRIDA_FX_NODE\n"
    );
    // A runtime without module.registerHooks (Bun, say) is refused too.
    let (project, node) = node_project("fx-node 24.3.0 0", true);
    let root = project.path();
    let output = command(root, &["plan", "case.yaml"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "grida-fx: old-node is not Node.js 24 or later with module.registerHooks; FX's \
         TypeScript node host needs it: install Node 24 or later, or set GRIDA_FX_NODE\n"
    );
    let output = command(root, &["doctor"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output).lines().nth(2),
        Some("node      old-node not Node.js 24+")
    );
    assert!(!root.join("started.txt").exists(), "the host was started");
}

#[test]
fn integrated_doctor_says_why_node_cannot_host() {
    // No @grida/fx anywhere.
    let (project, node) = node_project("fx-node 24.14.0 1", false);
    let root = project.path();
    let doctor = |node: &Path| {
        let output = command(root, &["doctor"])
            .env("GRIDA_FX_NODE", node)
            .output()
            .unwrap();
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        stdout(&output)
            .lines()
            .nth(2)
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(doctor(&node), "node      old-node no @grida/fx");
    let output = command(root, &["plan", "case.yaml"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "grida-fx: no @grida/fx for the TypeScript node host: run npm install @grida/fx in the \
         project\n"
    );
    // A program that does not exist is NOT FOUND.
    assert_eq!(
        doctor(&root.join("no-such-node")),
        "node      no-such-node NOT FOUND"
    );
    // A @grida/fx too old to host.
    install_sdk(root, false);
    assert_eq!(
        doctor(&node),
        "node      old-node cannot start: the project's @grida/fx has no TypeScript node host: \
         upgrade @grida/fx in the project"
    );
    // The host program is there, but this Node cannot run it.
    install_sdk(root, true);
    assert_eq!(
        doctor(&node),
        "node      old-node cannot start: the TypeScript node host (old-node) exited with status \
         2 before it answered initialize; set GRIDA_FX_NODE to Node 24 or later"
    );
    assert!(root.join("started.txt").exists());
}

#[test]
fn integrated_doctor_starts_no_typescript_host_without_a_package_json() {
    // A project with no package.json, below a folder whose node_modules holds a @grida/fx: its
    // host program is never started, and the line says what Node answered.
    let (project, node) = node_project("fx-node 24.14.0 1", false);
    let base = project.path();
    install_sdk(base, true);
    let root = base.join("acme");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    let doctor = |node: &Path, args: &[&str]| {
        let output = command(&root, args)
            .env("GRIDA_FX_NODE", node)
            .output()
            .unwrap();
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        stdout(&output)
            .lines()
            .nth(2)
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        doctor(&node, &["doctor"]),
        format!("node      {} (Node 24.14.0)", node.display())
    );
    assert!(!base.join("started.txt").exists(), "the host was started");
    // With a target too.
    std::fs::write(
        root.join("case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    \
         with: { prompt: a kite }\n",
    )
    .unwrap();
    let output = command(&root, &["doctor", "case.yaml"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output).lines().nth(2),
        Some(format!("node      {} (Node 24.14.0)", node.display()).as_str()),
        "{}",
        stderr(&output)
    );
    assert!(!base.join("started.txt").exists(), "the host was started");
    // A Node that gives no answer is named alone; one that cannot host still says why.
    let odd = base.join("odd-node");
    grida_fx_providers::testing::write_program(&odd, "#!/bin/sh\necho hello\n");
    assert_eq!(
        doctor(&odd, &["doctor"]),
        format!("node      {}", odd.display())
    );
    let (_old, old_node) = node_project("fx-node 22.12.0 1", false);
    assert_eq!(
        doctor(&old_node, &["doctor"]),
        format!(
            "node      {} too old (22.12.0; FX needs 24+)",
            old_node.display()
        )
    );
    // With a package.json, the host starts.
    std::fs::write(root.join("package.json"), "{}\n").unwrap();
    assert_eq!(
        doctor(&node, &["doctor"]),
        format!(
            "node      {} cannot start: the TypeScript node host ({}) exited with status 2 \
             before it answered initialize; set GRIDA_FX_NODE to Node 24 or later",
            node.display(),
            node.display()
        )
    );
    assert!(base.join("started.txt").exists());
}

/// A fake TypeScript host: answers `initialize` as `@grida/fx` 0.6.0 and `describe` with no
/// modules, then takes `shutdown` and `exit`.
const FAKE_TYPESCRIPT_HOST: &str = r#"
import json, sys
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, _, value = line.decode("ascii").partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))
def write(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None or message.get("method") == "exit":
        sys.exit(0)
    method = message.get("method")
    if method == "initialize":
        result = {"protocol": message["params"]["protocol"],
                  "host": {"language": "typescript", "version": "24.14.0", "sdk_version": "0.6.0"}}
    elif method == "describe":
        result = {"modules": [], "builtins": []}
    else:
        result = None
    write({"jsonrpc": "2.0", "id": message["id"], "result": result})
"#;

#[test]
fn integrated_doctor_names_the_projects_grida_fx() {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
        return;
    }
    let (project, _) = node_project("fx-node 24.14.0 1", true);
    let root = project.path();
    std::fs::write(root.join("fake_host.py"), FAKE_TYPESCRIPT_HOST).unwrap();
    let node = root.join("bin/node");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    grida_fx_providers::testing::write_program(
        &node,
        "#!/bin/sh\nif [ \"$1\" = -e ]; then echo 'fx-node 24.14.0 1'; exit 0; fi\nexec python3 \
         \"$(dirname \"$0\")/../fake_host.py\" \"$@\"\n",
    );
    let output = command(root, &["doctor"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output).lines().nth(2),
        Some("node      bin/node (@grida/fx 0.6.0)")
    );
    // The Node of the SDK that started the engine, when GRIDA_FX_NODE is not set.
    let output = command(root, &["doctor"])
        .env("GRIDA_FX_SDK_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output).lines().nth(2),
        Some("node      bin/node (@grida/fx 0.6.0)")
    );
}

#[test]
fn integrated_a_workflow_kept_outside_the_planning_project_finds_its_grida_fx() {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipped: python3 is not on PATH");
        return;
    }
    // `app` plans; its workflows: folder lies in `shared`, the workflow's home, which has no
    // @grida/fx of its own (gridaco/fx#68 decision 3: the home, then the planning project).
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let app = base.join("app");
    let shared = base.join("shared");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(shared.join("nodes")).unwrap();
    std::fs::create_dir_all(shared.join("workflows")).unwrap();
    std::fs::write(
        app.join("fx.yaml"),
        "fx: project/v1\nworkflows: [../shared/workflows]\n",
    )
    .unwrap();
    std::fs::write(app.join("package.json"), "{\"type\": \"module\"}\n").unwrap();
    install_sdk(&app, true);
    std::fs::write(shared.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(shared.join("nodes/say.ts"), "").unwrap();
    std::fs::write(
        shared.join("workflows/hello.yaml"),
        "fx: workflow/v1\nid: hello\ntitle: Hello\nsteps:\n  say:\n    uses: ./nodes/say.ts#say\n",
    )
    .unwrap();
    std::fs::write(base.join("fake_host.py"), FAKE_TYPESCRIPT_HOST).unwrap();
    let node = base.join("bin/node");
    std::fs::create_dir_all(base.join("bin")).unwrap();
    grida_fx_providers::testing::write_program(
        &node,
        "#!/bin/sh\nif [ \"$1\" = -e ]; then echo 'fx-node 24.14.0 1'; exit 0; fi\necho \"$@\" >> \
         \"$(dirname \"$0\")/../argv.txt\"\nexec python3 \"$(dirname \"$0\")/../fake_host.py\" \"$@\"\n",
    );
    let output = command(&app, &["doctor", "hello"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output).lines().nth(2),
        Some(format!("node      {} (@grida/fx 0.6.0)", node.display()).as_str()),
        "{}",
        stderr(&output)
    );
    let entry = app.join("node_modules/@grida/fx/dist/host-entry.js");
    let argv = std::fs::read_to_string(base.join("argv.txt")).unwrap();
    assert_eq!(argv.lines().last(), Some(entry.to_str().unwrap()));
    // Planning starts the same host program.
    std::fs::remove_file(base.join("argv.txt")).unwrap();
    let output = command(&app, &["plan", "hello"])
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert!(
        !stderr(&output).contains("no @grida/fx"),
        "{}",
        stderr(&output)
    );
    let argv = std::fs::read_to_string(base.join("argv.txt")).unwrap();
    assert_eq!(argv.lines().last(), Some(entry.to_str().unwrap()));
}

#[test]
fn integrated_a_typescript_stand_in_needs_a_node_host() {
    let project = drawing_project(ROUTED);
    let root = project.path();
    std::fs::write(root.join("stand_in.ts"), "export function answer() {}\n").unwrap();
    let argv = [
        "run",
        "case",
        "--routes",
        "routes.yaml",
        "--stand-in",
        "stand_in.ts#answer",
    ];
    let output = grida_fx(root, &argv);
    assert_eq!(status(&output), 2, "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "grida-fx: the stand-in stand_in.ts#answer could not be loaded: no @grida/fx for the \
         TypeScript node host: run npm install @grida/fx in the project\n"
    );
    // With @grida/fx, the stand-in host is Node: here one too old.
    install_sdk(root, true);
    let node = root.join("old-node");
    grida_fx_providers::testing::write_program(
        &node,
        "#!/bin/sh\nif [ \"$1\" = -e ]; then echo 'fx-node 22.12.0 1'; exit 0; fi\nexit 2\n",
    );
    let output = command(root, &argv)
        .env("GRIDA_FX_NODE", &node)
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}", stdout(&output));
    assert_eq!(
        stderr(&output),
        "grida-fx: the stand-in stand_in.ts#answer could not be loaded: old-node is Node \
         22.12.0; FX's TypeScript node host needs Node 24 or later: install it, or set \
         GRIDA_FX_NODE\n"
    );
    assert!(!root.join("runs").exists());
}

#[test]
fn integrated_a_failing_planning_interpreter_leaves_no_private_path_in_the_record() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    std::fs::write(
        root.join("fx.yaml"),
        "fx: project/v1\nworkflows: [library]\n",
    )
    .unwrap();
    let home = root.join("library/thing");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(
        home.join("case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  pack:\n    uses: fx/package@1\n    \
         with: { files: {}, manifest: { n: 1 } }\n",
    )
    .unwrap();
    // Only the planning project has an interpreter, and it cannot start a host.
    let bin = root.join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("python"), "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(
        bin.join("python"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let output = grida_fx(&root, &["run", "case", "--run", "runs/one"]);
    assert_eq!(status(&output), 1, "{}{}", stdout(&output), stderr(&output));
    let log = std::fs::read_to_string(root.join("runs/one/events.jsonl")).unwrap();
    let failed = events(&root.join("runs/one"))
        .into_iter()
        .find(|event| event["event"] == "node_failed")
        .expect("pack failed");
    let error = failed["error"].as_str().unwrap();
    assert!(
        error.starts_with("the Python node host (../../.venv/bin/python) exited with status 3"),
        "{error}"
    );
    for private in [root.clone(), folder.path().to_path_buf()] {
        assert!(!log.contains(&*private.to_string_lossy()), "{log}");
    }
}

#[test]
fn integrated_doctor_reads_the_projects_key_file() {
    let project = empty_project();
    std::fs::write(
        project.path().join(".env"),
        format!(
            "# provider keys\nOPENAI_API_KEY={MADE_UP_KEY}\nexport FAL_KEY='{MADE_UP_KEY}'\n\
             UNRELATED_SECRET=\"not read\n"
        ),
    )
    .unwrap();
    let doctor = |process: &[(&str, &str)]| {
        let mut command = command(project.path(), &["doctor"]);
        command.env_remove("GRIDA_FX_DISABLE_DOTENV");
        for (name, value) in process {
            command.env(name, value);
        }
        command.output().unwrap()
    };
    let output = doctor(&[]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_no_key(&output);
    let text = stdout(&output);
    assert!(
        text.contains("\nkey       OPENAI_API_KEY present (.env)\n"),
        "{text}"
    );
    assert!(
        text.contains("\nkey       FAL_KEY present (.env)\n"),
        "{text}"
    );
    assert!(
        text.contains("\nkey       TRIPO_API_KEY missing\n"),
        "{text}"
    );
    assert!(
        text.contains("\nroute     image.generate gpt-image-2.5-sunburst@openai servable\n"),
        "{text}"
    );
    // A process value wins over the file's.
    let output = doctor(&[("FAL_KEY", "fx-made-up-process-key")]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\nkey       FAL_KEY present (environment)\n"),
        "{}",
        stdout(&output)
    );
    assert!(!stdout(&output).contains("fx-made-up-process-key"));
    // GRIDA_FX_DISABLE_DOTENV=1 turns the file off.
    let output = doctor(&[("GRIDA_FX_DISABLE_DOTENV", "1")]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\nkey       OPENAI_API_KEY missing\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_doctor_refuses_a_malformed_key_file() {
    let project = empty_project();
    std::fs::write(
        project.path().join(".env"),
        format!("OPENAI_API_KEY={MADE_UP_KEY}\nFAL_KEY=\"{MADE_UP_KEY}\n"),
    )
    .unwrap();
    let output = command(project.path(), &["doctor"])
        .env_remove("GRIDA_FX_DISABLE_DOTENV")
        .env("TRIPO_API_KEY", MADE_UP_KEY)
        .output()
        .unwrap();
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    assert_no_key(&output);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[3].starts_with("keys      .env"), "{text}");
    assert!(lines[3].contains("FAL_KEY"), "{text}");
    assert!(lines[3].contains("line 2"), "{text}");
    // The rest is told from the process environment alone.
    assert_eq!(lines[4], "key       GG_API_KEY missing", "{text}");
    assert_eq!(lines[5], "key       OPENAI_API_KEY missing", "{text}");
    assert!(
        text.contains("\nkey       TRIPO_API_KEY present (environment)\n"),
        "{text}"
    );
    assert!(text.contains("\nroute     mesh.rig v1.0-20240301@tripo servable\n"));
    // Missing keys alone do not change the exit status; the file off, all is well again.
    let output = command(project.path(), &["doctor"]).output().unwrap();
    assert_eq!(status(&output), 0, "{}", stderr(&output));
}

#[cfg(unix)]
#[test]
fn integrated_doctor_refuses_a_symlinked_key_file() {
    let project = empty_project();
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("keys");
    std::fs::write(&target, format!("FAL_KEY={MADE_UP_KEY}\n")).unwrap();
    std::os::unix::fs::symlink(&target, project.path().join(".env")).unwrap();
    let output = command(project.path(), &["doctor"])
        .env_remove("GRIDA_FX_DISABLE_DOTENV")
        .output()
        .unwrap();
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    assert_no_key(&output);
    assert!(
        stdout(&output).contains("\nkeys      .env must be a regular file\n"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("\nkey       FAL_KEY missing\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_plan_text_of_tiered_price() {
    let project = conformance_project("tiered-price");
    let output = grida_fx(project.path(), &["plan", "case", "--routes", "routes.yaml"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "case  ·  1 phase\n\
         phase 1   4 steps   4 provider calls   $2.61 – $8.36\n\
         cached    0 of 4 known steps\n\
         estimate  $2.61 – $8.36\n"
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
        "estimate  $2.61 – $8.36   ceiling $1.00   ⚠ the worst case exceeds the ceiling; the run stops before crossing it\n"
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
        stdout(&output).ends_with("\nnot cached: small#1, large#1, unknown#1, unlisted#1\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_a_refused_plan() {
    let project = conformance_project("tiered-price");
    // Without --routes the catalog is the built-in table, which holds none of the case's routes.
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
                "phase": 1, "steps": 4, "calls": [4, 4],
                "low_usd": 2.61, "high_usd": 8.3625, "then": [],
            }],
            "estimate": {"low_usd": 2.61, "high_usd": 8.3625},
            "ceiling_usd": null,
        })
    );
    let identity = run("identity");
    let ids: Vec<&String> = identity.as_object().unwrap().keys().collect();
    assert_eq!(ids, ["large#1", "small#1", "unknown#1", "unlisted#1"]);
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
        json!({"fx/video.generate@1": {
            "identity": "fx/video.generate@1.1",
            "ports": {
                "inputs": {"first_frame": "image?", "last_frame": "image?"},
                "outputs": {"video": "video/mp4"},
                "params": {
                    "aspect_ratio": {"type": "string", "x-fx-optional": true},
                    "duration": {"type": "number", "x-fx-optional": true},
                    "prompt": {"type": "string", "x-fx-template": true},
                    "resolution": {"type": "string", "x-fx-optional": true},
                },
            },
        }})
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
    assert!(stdout(&output).ends_with("\nnot cached: small#1, large#1, unknown#1, unlisted#1\n"));
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

// ------------------------------------------------------------------------------- the run verbs

/// A project whose workflow `case` draws one picture with a paid built-in and declares the
/// outputs `image` and `again`; planning it needs no Python. `fx_yaml` is its project file.
fn drawing_project(fx_yaml: &str) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    std::fs::write(root.join("fx.yaml"), fx_yaml).unwrap();
    std::fs::write(
        root.join("routes.yaml"),
        "fx: routes/v1\nroutes:\n  - { capability: image.generate, route: img-a@acme, price: \
         { low_usd: 0.01, high_usd: 0.04 } }\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("workflows")).unwrap();
    std::fs::write(
        root.join("workflows/case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\ninputs:\n  prompt: { type: string, default: a \
         kite }\nsteps:\n  draw:\n    uses: fx/image.generate@1\n    with: { prompt: \"${{ \
         inputs.prompt }}\" }\noutputs:\n  image: ${{ steps.draw.outputs.image }}\n  again: ${{ \
         steps.draw.outputs.image }}\n",
    )
    .unwrap();
    folder
}

const ROUTED: &str = "fx: project/v1\nroutes:\n  image.generate: img-a@acme\n";

#[test]
fn run_help_names_its_options() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["run", "--help"]);
    assert_eq!(status(&output), 0);
    let help = stdout(&output);
    for option in [
        "--live",
        "--max-usd",
        "--yes-up-to",
        "--deliver",
        "--run",
        "--inputs",
        "--routes",
        "--arg",
    ] {
        assert!(help.contains(option), "{option}: {help}");
    }
    let output = grida_fx(project.path(), &["--help"]);
    for verb in [
        "run", "reroll", "pick", "takes", "jobs", "project", "inspect", "runs",
    ] {
        assert!(stdout(&output).contains(verb), "{verb}");
    }
}

#[test]
fn run_refuses_bad_amounts_before_anything_is_read_or_written() {
    let project = drawing_project(ROUTED);
    let cases: [(&[&str], &str); 5] = [
        (&["--max-usd", "-1"], "--max-usd -1: -1 is negative"),
        (&["--max-usd", "nan"], "--max-usd nan: "),
        (
            &["--yes-up-to", "-0.5"],
            "--yes-up-to -0.5: -0.5 is negative",
        ),
        (&["--yes-up-to", "inf"], "--yes-up-to inf: "),
        (
            &["--yes-up-to", "0.0000001"],
            "--yes-up-to 0.0000001: 0.0000001 has more than 6 decimal places",
        ),
    ];
    for (extra, message) in cases {
        let mut argv = vec!["run", "case", "--routes", "routes.yaml"];
        argv.extend_from_slice(extra);
        let output = grida_fx(project.path(), &argv);
        assert_eq!(status(&output), 2, "{extra:?}");
        assert!(stdout(&output).is_empty(), "{extra:?}");
        assert!(
            stderr(&output).starts_with(&format!("grida-fx: {message}")),
            "{extra:?}: {}",
            stderr(&output)
        );
    }
    assert!(!project.path().join("runs").exists());
    assert!(!project.path().join(".fx").exists());
}

#[test]
fn integrated_run_checks_deliver_names_before_planning() {
    let project = drawing_project(ROUTED);
    let run = |extra: &[&str]| {
        let mut argv = vec!["run", "case", "--routes", "routes.yaml"];
        argv.extend_from_slice(extra);
        grida_fx(project.path(), &argv)
    };
    let output = run(&["--deliver", "nosuch=out/x.png"]);
    assert_eq!(status(&output), 2);
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "grida-fx: --deliver nosuch=out/x.png: name one of again, image\n"
    );
    for pair in ["image", "=out.png", "image="] {
        let output = run(&["--deliver", pair]);
        assert_eq!(status(&output), 2, "{pair}");
        assert_eq!(
            stderr(&output),
            format!("grida-fx: --deliver {pair}: write OUTPUT=PATH\n")
        );
    }
    // The workflow's own input flags still reach planning after run's options.
    let output = run(&["--deliver", "image=out/x.png", "--nosuch", "1"]);
    assert_eq!(status(&output), 2);
    assert!(
        stderr(&output).contains("unknown input flag --nosuch"),
        "{}",
        stderr(&output)
    );
    assert!(!project.path().join("runs").exists());
    assert!(!project.path().join("out").exists());
}

#[test]
fn integrated_a_refused_plan_runs_nothing() {
    // No route serves image.generate: the plan has a problem, so no folder is made.
    let project = drawing_project("fx: project/v1\n");
    let output = grida_fx(
        project.path(),
        &[
            "run",
            "case",
            "--routes",
            "routes.yaml",
            "--run",
            "runs/one",
        ],
    );
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with("case  ·  1 phase\n"), "{text}");
    assert!(
        text.contains("\nrefused   draw.route: no route for image.generate"),
        "{text}"
    );
    assert!(!text.contains("\nrun       "), "{text}");
    assert!(!project.path().join("runs").exists());
}

#[test]
fn pick_refuses_a_take_out_of_bounds_before_reading_the_run() {
    let project = empty_project();
    for (take, message) in [
        ("0", "a take is 1 or more"),
        ("-1", "a take is 1 or more"),
        ("1001", "a take is at most 1000"),
        ("2.5", "a take is a whole number, not 2.5"),
        ("x", "a take is a whole number, not x"),
    ] {
        let output = grida_fx(project.path(), &["pick", "runs/one", "draw", take]);
        assert_eq!(status(&output), 2, "{take}");
        assert!(stdout(&output).is_empty());
        assert_eq!(stderr(&output), format!("grida-fx: {message}\n"), "{take}");
    }
}

#[test]
fn reroll_and_pick_need_a_run_folder() {
    let project = empty_project();
    for argv in [
        &["reroll", "runs/one", "draw"][..],
        &["pick", "runs/one", "draw", "2"],
        &["inspect", "runs/one"],
    ] {
        let output = grida_fx(project.path(), argv);
        assert_eq!(status(&output), 2, "{argv:?}");
        assert_eq!(
            stderr(&output),
            "grida-fx: runs/one is not a run folder\n",
            "{argv:?}"
        );
    }
    // A folder whose plan names no takes file was not started by `grida-fx run`.
    let folder = project.path().join("runs/one");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("plan.json"),
        r#"{"kind": "fx-graph-v1", "workflow": {"id": "case"}}"#,
    )
    .unwrap();
    let output = grida_fx(project.path(), &["reroll", "runs/one", "draw"]);
    assert_eq!(status(&output), 2);
    assert_eq!(
        stderr(&output),
        "grida-fx: runs/one was not started by grida-fx run\n"
    );
}

#[test]
fn project_needs_a_log() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["project", "runs/one"]);
    assert_eq!(status(&output), 2);
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "grida-fx: runs/one/events.jsonl: no such file\n"
    );
}

#[test]
fn inspect_of_a_workflow_without_runs() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["inspect", "case"]);
    assert_eq!(status(&output), 2);
    assert_eq!(
        stderr(&output),
        "grida-fx: no run folder case, and no runs of a workflow case here\n"
    );
}

#[test]
fn jobs_forget_needs_a_digest() {
    let project = empty_project();
    for key in ["k0", "../x", "ABCDEF"] {
        let output = grida_fx(project.path(), &["jobs", "--forget", key]);
        assert_eq!(status(&output), 2, "{key}");
        assert_eq!(stderr(&output), format!("grida-fx: not a digest: {key}\n"));
    }
}

/// `workflows/case.takes.yaml` of a drawing project.
fn write_takes(project: &Path, text: &str) -> PathBuf {
    let path = project.join("workflows/case.takes.yaml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn integrated_takes_list_sorts_by_step_path() {
    let project = drawing_project(ROUTED);
    let output = grida_fx(project.path(), &["takes", "list", "case"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    let digest = "5637c1b4a68923781eb2e9b563f97e0bf346245040371b2a3c3c2365798ba24d";
    write_takes(
        project.path(),
        &format!("draw: {{ take: 3 }}\n\"a['x'].b\": {{ take: 1, result: \"{digest}\" }}\n"),
    );
    for target in ["case", "workflows/case.yaml"] {
        let output = grida_fx(project.path(), &["takes", "list", target]);
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        assert_eq!(
            stdout(&output),
            format!("a['x'].b  take 1  {digest}\ndraw  take 3\n")
        );
    }
    let output = grida_fx(project.path(), &["takes", "list", "build.py:make"]);
    assert_eq!(status(&output), 2);
    let output = grida_fx(project.path(), &["takes", "list", "nosuch"]);
    assert_eq!(status(&output), 2);
    let output = grida_fx(project.path(), &["takes"]);
    assert_eq!(status(&output), 2);
}

#[test]
fn integrated_takes_mv_refusals_leave_the_file_alone() {
    let project = drawing_project(ROUTED);
    let text = "draw: { take: 3 }\nold: { take: 2 }\n";
    let path = write_takes(project.path(), text);
    for (old, new, message) in [
        ("zz", "draw", "case.takes.yaml has no entry zz"),
        ("old", "draw", "case.takes.yaml already has an entry draw"),
        ("old", "Not A Path", "Not A Path is not a step path"),
    ] {
        let output = grida_fx(project.path(), &["takes", "mv", "case", old, new]);
        assert_eq!(status(&output), 2, "{old} {new}");
        assert_eq!(stderr(&output), format!("grida-fx: {message}\n"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

#[test]
fn integrated_takes_mv_moves_an_entry() {
    let project = drawing_project(ROUTED);
    let path = write_takes(project.path(), "# mine\nold: { take: 2 }\n");
    let output = grida_fx(project.path(), &["takes", "mv", "case", "old", "draw"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output), "case.takes.yaml: old is now draw\n");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.starts_with(
        "# case.takes.yaml: written by `grida-fx reroll` and `grida-fx pick`; commit it\n"
    ));
    let output = grida_fx(project.path(), &["takes", "list", "case"]);
    assert_eq!(stdout(&output), "draw  take 2\n");
}

/// A canonical event line with the envelope.
fn event_line(event: &str, fields: Value) -> String {
    let mut object = json!({
        "kind": "fx-run-events-v1",
        "event": event,
        "invocation_id": "0123456789abcdef",
        "plan": "a".repeat(64),
        "offset_ms": 1,
    });
    for (key, value) in fields.as_object().unwrap() {
        object[key] = value.clone();
    }
    let mut line = serde_json::to_string(&object).unwrap();
    line.push('\n');
    line
}

/// A run folder written by hand at `relative`, a run of `case` whose `draw` finished take 1 and
/// whose `other` failed, with one placed file.
fn hand_made_run(project: &Path, relative: &str) -> (PathBuf, String) {
    let folder = project.join(relative);
    std::fs::create_dir_all(folder.join("files/draw")).unwrap();
    let bytes = b"PNG take 1";
    std::fs::write(folder.join("files/draw/image.png"), bytes).unwrap();
    let digest = digest_of(bytes);
    std::fs::write(
        folder.join("plan.json"),
        r#"{"kind": "fx-graph-v1", "workflow": {"id": "case", "title": "Case"},
            "takes_file": "workflows/case.takes.yaml",
            "instances": [
              {"id": "draw#1", "path": "draw", "step": "draw", "state": "planned"},
              {"id": "other#1", "path": "other", "step": "other", "state": "planned"},
              {"id": "gone#1", "path": "gone", "step": "gone", "state": "absent"}
            ]}"#,
    )
    .unwrap();
    let file = json!({"file": {"digest": digest, "kind": "image/png", "name": "draw/image",
                               "size": bytes.len()}});
    let log = [
        event_line(
            "run_started",
            json!({"workflow": "case", "resumed": false, "ceiling_usd": null,
                   "charged_usd": 0, "estimate": {"low_usd": 0, "high_usd": 0}}),
        ),
        event_line(
            "node_started",
            json!({"id": "draw#1", "path": "draw", "step": "draw", "take": [1],
                   "identity": null, "uses": "fx/image.generate@1", "reads": [],
                   "routes": {}, "with": {}}),
        ),
        event_line(
            "node_finished",
            json!({"id": "draw#1", "path": "draw", "cache": "miss", "outputs": {"image": file},
                   "facts": {"cost_usd": 0.04}, "duration_ms": 5}),
        ),
        event_line(
            "node_started",
            json!({"id": "other#1", "path": "other", "step": "other", "take": [1],
                   "identity": null, "uses": "fx/image.generate@1", "reads": [],
                   "routes": {}, "with": {}}),
        ),
        event_line(
            "node_failed",
            json!({"id": "other#1", "path": "other", "error": "refused on purpose",
                   "facts": {}, "duration_ms": 1}),
        ),
        event_line(
            "run_finished",
            json!({"ok": false, "incomplete": false, "stopped": null, "charged_usd": 0.04,
                   "failed": ["other#1"], "outputs": {"image": file}}),
        ),
    ]
    .concat();
    std::fs::write(folder.join("events.jsonl"), log).unwrap();
    (folder, digest)
}

/// Appends a live invocation's `run_started` that records `repeat` to a hand-made run's log.
fn add_repeat(folder: &Path) {
    let log = folder.join("events.jsonl");
    let mut text = std::fs::read_to_string(&log).unwrap();
    text.push_str(&event_line(
        "run_started",
        json!({"workflow": "case", "resumed": true, "ceiling_usd": 2, "charged_usd": 0.04,
               "estimate": {"low_usd": 0, "high_usd": 0},
               "repeat": {"file": "workflows/case.yaml", "inputs": ["inputs/x y.yaml"],
                          "max_usd": 2, "live": true}}),
    ));
    std::fs::write(&log, text).unwrap();
}

/// Records a hand-made run's folder in its project's run index, as `run` does (spec/store.md §9).
fn index_run(project: &Path, relative: &str) {
    let index = project.join(".fx/runs");
    std::fs::create_dir_all(&index).unwrap();
    std::fs::write(
        index.join(format!("{}.json", digest_of(relative.as_bytes()))),
        format!("{}\n", json!({"folder": relative})),
    )
    .unwrap();
}

/// A run of another project's workflow, rerolled from this one: the takes file and the command
/// are the planning project's, the one whose run index records the run folder.
#[test]
fn integrated_reroll_of_another_projects_run() {
    let here = drawing_project(ROUTED);
    let there = drawing_project(ROUTED);
    hand_made_run(there.path(), "runs/one");
    add_repeat(&there.path().join("runs/one"));
    index_run(there.path(), "runs/one");
    let there_name = there.path().canonicalize().unwrap();
    let there_name = there_name.file_name().unwrap().to_str().unwrap();
    let here_root = here.path().canonicalize().unwrap();
    let run = format!("../{there_name}/runs/one");
    let output = grida_fx(&here_root, &["reroll", &run, "draw"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "case.takes.yaml: draw uses take 2 from now on\n\
             next      grida-fx run ../{there_name}/workflows/case.yaml --inputs \
             '../{there_name}/inputs/x y.yaml' --max-usd 2 --live\n"
        )
    );
    assert!(there.path().join("workflows/case.takes.yaml").is_file());
    assert!(!here.path().join("workflows/case.takes.yaml").exists());
}

/// The SHA-256 of some bytes, as the engine names files (the CLI crate has no hasher of its
/// own, so the test asks the system's `shasum` or `sha256sum`).
fn digest_of(bytes: &[u8]) -> String {
    use std::io::Write as _;
    let mut child = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .or_else(|_| {
            Command::new("sha256sum")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
        })
        .expect("shasum or sha256sum");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout).unwrap()[..64].to_string()
}

#[test]
fn integrated_project_of_a_hand_written_log() {
    let project = drawing_project(ROUTED);
    let (_, digest) = hand_made_run(project.path(), "runs/one");
    let output = grida_fx(project.path(), &["project", "runs/one"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let projected = parse(&stdout(&output));
    assert_eq!(
        projected["instances"],
        json!({
            "draw#1": {"state": "succeeded", "path": "draw", "cache": "miss",
                       "facts": {"cost_usd": 0.04}},
            "other#1": {"state": "failed", "path": "other", "error": "refused on purpose"},
        })
    );
    let run = &projected["run"];
    assert!(run["run_started"].get("kind").is_none());
    assert_eq!(run["run_finished"]["plan"], "a".repeat(64));
    assert_eq!(
        run["run_finished"]["outputs"]["image"]["file"]["digest"],
        digest
    );
    assert!(run.get("run_cancelled").is_none());
    // A torn last line is not read.
    let log = project.path().join("runs/one/events.jsonl");
    let mut text = std::fs::read_to_string(&log).unwrap();
    text.push_str("{\"kind\": \"fx-run-events-v1\", \"event\": \"node_st");
    std::fs::write(&log, text).unwrap();
    let again = grida_fx(project.path(), &["project", "runs/one"]);
    assert_eq!(status(&again), 0, "{}", stderr(&again));
    assert_eq!(parse(&stdout(&again)), projected);
}

#[test]
fn integrated_inspect_of_a_hand_written_run() {
    let project = drawing_project(ROUTED);
    let (folder, digest) = hand_made_run(project.path(), "runs/case/one");
    let expected = "case  ·  one\n\
                    state     failed   2 steps: 1 failed, 1 succeeded\n\
                    spent     $0.04\n\
                    failed    other#1: refused on purpose\n";
    for given in ["runs/case/one", "case"] {
        let output = grida_fx(project.path(), &["inspect", given]);
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        assert_eq!(stdout(&output), expected, "{given}");
    }
    let inspect = |extra: &[&str]| {
        let mut argv = vec!["inspect", "runs/case/one"];
        argv.extend_from_slice(extra);
        grida_fx(project.path(), &argv)
    };
    let output = inspect(&["--verify"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).ends_with("verified  1 files\n"));
    let output = grida_fx(project.path(), &["inspect", "case", "--json"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let document = parse(&stdout(&output));
    assert_eq!(document["run"]["state"], "failed");
    assert_eq!(document["run"]["folder"], "runs/case/one");
    assert_eq!(
        document["run"]["steps"][0]["files"],
        json!([{"path": "files/draw/image.png", "digest": digest, "size": 10}])
    );
    std::fs::write(folder.join("files/draw/image.png"), b"PNG take 2").unwrap();
    let output = inspect(&["--verify", "--json"]);
    assert_eq!(status(&output), 1);
    assert_eq!(
        parse(&stdout(&output))["verification"],
        json!({"verified": false,
               "problems": ["draw#1: files/draw/image.png differs from its recorded digest"]})
    );
    std::fs::remove_file(folder.join("files/draw/image.png")).unwrap();
    let output = inspect(&["--verify"]);
    assert_eq!(status(&output), 1);
    assert!(
        stdout(&output).ends_with("differs   draw#1: files/draw/image.png is missing\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_reroll_and_pick_over_a_hand_made_run() {
    let project = drawing_project(ROUTED);
    let (_, digest) = hand_made_run(project.path(), "runs/one");
    let takes = project.path().join("workflows/case.takes.yaml");
    // Its record holds no `repeat`: advice, whatever `--live` (kept, and ignored) says.
    let output = grida_fx(project.path(), &["reroll", "runs/one", "draw", "--live"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "case.takes.yaml: draw uses take 2 from now on\n\
         next      run case again with the same inputs and options\n"
    );
    let output = grida_fx(project.path(), &["takes", "list", "case"]);
    assert_eq!(stdout(&output), "draw  take 2\n");
    let output = grida_fx(project.path(), &["reroll", "runs/one", "zzz"]);
    assert_eq!(status(&output), 2);
    assert_eq!(stderr(&output), "grida-fx: runs/one never ran a step zzz\n");
    // A later, live invocation that records `repeat`: the command, its paths relative to the
    // working directory, once the project's run index records the folder.
    add_repeat(&project.path().join("runs/one"));
    let output = grida_fx(project.path(), &["pick", "runs/one", "draw", "1"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).ends_with("next      run case again with the same inputs and options\n"),
        "{}",
        stdout(&output)
    );
    index_run(project.path(), "runs/one");
    let output = grida_fx(
        &project.path().join("workflows"),
        &["pick", "../runs/one", "draw", "1"],
    );
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "case.takes.yaml: draw uses take 1\n\
         next      grida-fx run case.yaml --inputs '../inputs/x y.yaml' --max-usd 2 --live\n"
    );
    let output = grida_fx(project.path(), &["takes", "list", "case"]);
    assert_eq!(stdout(&output), format!("draw  take 1  {digest}\n"));
    // A take that never finished here is picked without a result.
    let output = grida_fx(project.path(), &["pick", "runs/one", "other", "7"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let output = grida_fx(project.path(), &["pick", "runs/one", "nosuch", "1"]);
    assert_eq!(status(&output), 2);
    assert_eq!(stderr(&output), "grida-fx: runs/one has no step nosuch\n");
    let text = std::fs::read_to_string(&takes).unwrap();
    assert!(text.starts_with("# case.takes.yaml: written by"), "{text}");
    let output = grida_fx(project.path(), &["takes", "list", "case"]);
    assert_eq!(
        stdout(&output),
        format!("draw  take 1  {digest}\nother  take 7\n")
    );
}

#[test]
fn integrated_run_started_records_how_to_repeat_the_run() {
    let project = empty_project();
    let root = project.path();
    std::fs::write(
        root.join("case.yaml"),
        "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  pack:\n    uses: fx/package@1\n    \
         with: { files: {}, manifest: { n: 1 } }\n",
    )
    .unwrap();
    let repeat = |folder: &str, args: &[&str], sdk: bool| {
        let mut argv = vec!["run"];
        argv.extend_from_slice(args);
        argv.extend_from_slice(&["--run", folder, "--no-view"]);
        let mut command = command(root, &argv);
        if sdk {
            command.env("GRIDA_FX_CANCEL_SOURCE", "sdk");
        }
        // Whether the step's host starts does not matter: the record starts before it.
        let output = command.output().unwrap();
        assert!(
            status(&output) <= 1,
            "{}{}",
            stdout(&output),
            stderr(&output)
        );
        events(&root.join(folder))
            .into_iter()
            .find(|event| event["event"] == "run_started")
            .expect("a run_started")
            .get("repeat")
            .cloned()
    };
    assert_eq!(
        repeat("runs/id", &["case"], false),
        Some(json!({"workflow": "case", "live": false}))
    );
    assert_eq!(
        repeat(
            "runs/live",
            &["./case.yaml", "--live", "--max-usd", "1"],
            false
        ),
        Some(json!({"file": "case.yaml", "max_usd": 1, "live": true}))
    );
    // An SDK's inputs file is temporary: nothing to repeat from.
    assert_eq!(repeat("runs/sdk", &["case"], true), None);
}

/// A job record as the store writes it (fx-job-record-v1).
fn job_record(key: &str, state: &str, take: &[u32]) -> String {
    let handle = if state == "submitting" {
        Value::Null
    } else {
        json!({"job": "j-1"})
    };
    serde_json::to_string(&json!({
        "kind": "fx-job-record-v1",
        "key": key,
        "capability": "video.generate",
        "route": {"id": "vid@acme", "fingerprint": "f".repeat(64)},
        "request": {"prompt": "a kite"},
        "take": take,
        "state": state,
        "handle": handle,
    }))
    .unwrap()
}

#[test]
fn integrated_jobs_lists_and_forgets_job_records() {
    let project = empty_project();
    let output = grida_fx(project.path(), &["jobs"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    let jobs = project.path().join(".fx/cache/jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    let (first, second) = ("1".repeat(64), "2".repeat(64));
    std::fs::write(
        jobs.join(format!("{second}.json")),
        job_record(&second, "settled", &[2, 1]),
    )
    .unwrap();
    std::fs::write(
        jobs.join(format!("{first}.json")),
        job_record(&first, "submitting", &[1]),
    )
    .unwrap();
    let third = "3".repeat(64);
    let mut noted: Value = serde_json::from_str(&job_record(&third, "submitting", &[1])).unwrap();
    noted["note"] = json!("fal took the job but returned no handle (request req-7)");
    std::fs::write(jobs.join(format!("{third}.json")), noted.to_string()).unwrap();
    let output = grida_fx(project.path(), &["jobs"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "{first}  submitting  video.generate on vid@acme, take 1\n\
             {second}  settled  video.generate on vid@acme, take 2.1\n\
             {third}  submitting  video.generate on vid@acme, take 1  fal took the job but \
             returned no handle (request req-7)\n"
        )
    );
    let output = grida_fx(project.path(), &["jobs", "--forget", &first]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!("forgot job {first}; its call is submitted again on the next run\n")
    );
    assert!(!jobs.join(format!("{first}.json")).exists());
    let output = grida_fx(project.path(), &["jobs", "--forget", &first]);
    assert_eq!(status(&output), 2);
    assert_eq!(
        stderr(&output),
        format!("grida-fx: the cache holds no job {first}\n")
    );
    std::fs::write(jobs.join(format!("{first}.json")), "{not json").unwrap();
    let output = grida_fx(project.path(), &["jobs"]);
    assert_eq!(status(&output), 2);
    assert!(
        stderr(&output).contains(&format!("jobs/{first}.json")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn integrated_a_live_run_needs_a_ceiling() {
    let project = conformance_project("cache-replay");
    let output = grida_fx(
        project.path(),
        &["run", "case", "--routes", "routes.yaml", "--live"],
    );
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\nrefused: a live run needs a ceiling"),
        "{}",
        stdout(&output)
    );
    assert!(!project.path().join("runs").exists());
}

/// A project like [`drawing_project`] whose `image.generate` is bound to a built-in route.
const BUILT_IN: &str = "fx: project/v1\nroutes:\n  image.generate: gpt-image-2.5-sunburst@openai\n";

#[test]
fn integrated_a_plan_without_routes_files_is_priced_from_the_built_in_table() {
    let project = drawing_project(BUILT_IN);
    let output = grida_fx(project.path(), &["price", "case"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let price = parse(&stdout(&output));
    // No size: the route's whole range.
    assert_eq!(
        price["estimate"],
        json!({"low_usd": 0.09, "high_usd": 0.7}),
        "{price}"
    );
    assert_eq!(price["phases"][0]["calls"], json!([1, 1]), "{price}");
    let output = grida_fx(project.path(), &["plan", "case", "--check"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).ends_with("estimate  $0.09 – $0.70\n"),
        "{}",
        stdout(&output)
    );
    // A size the route's tiers list is priced at its tier; any other at the whole range.
    for (size, estimate) in [
        ("1024x1024", json!({"low_usd": 0.21, "high_usd": 0.29})),
        ("2048x2048", json!({"low_usd": 0.09, "high_usd": 0.7})),
        ("auto", json!({"low_usd": 0.09, "high_usd": 0.7})),
    ] {
        std::fs::write(
            project.path().join("workflows/sized.yaml"),
            format!(
                "fx: workflow/v1\nid: sized\ntitle: Sized\nsteps:\n  draw:\n    uses: \
                 fx/image.generate@1\n    with: {{ prompt: a kite, size: {size} }}\n"
            ),
        )
        .unwrap();
        let output = grida_fx(project.path(), &["price", "sized"]);
        assert_eq!(status(&output), 0, "{}", stderr(&output));
        let price = parse(&stdout(&output));
        assert_eq!(price["estimate"], estimate, "{size}: {price}");
    }
    // Any --routes file leaves the built-in table out.
    let output = grida_fx(project.path(), &["plan", "case", "--routes", "routes.yaml"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("\nrefused   draw.route: "),
        "{}",
        stdout(&output)
    );
}

/// Every event of a run's log.
fn events(folder: &Path) -> Vec<Value> {
    std::fs::read_to_string(folder.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(parse)
        .collect()
}

/// Every file under `folder`, read as bytes, with its path.
fn every_file(folder: &Path, found: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            every_file(&path, found);
        } else {
            found.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
}

/// Asserts that a run spent nothing: every hold settled at $0, and the run charged $0.
fn assert_spent_nothing(folder: &Path) {
    let events = events(folder);
    let settled: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == "budget_settled")
        .collect();
    let reserved = events
        .iter()
        .filter(|e| e["event"] == "budget_reserved")
        .count();
    assert_eq!(settled.len(), reserved, "{events:#?}");
    for event in settled {
        assert_eq!(event["charged_usd"], json!(0), "{event}");
    }
    let finished = events
        .iter()
        .find(|e| e["event"] == "run_finished")
        .unwrap();
    assert_eq!(finished["charged_usd"], json!(0), "{finished}");
}

#[test]
fn integrated_a_live_call_without_its_key_is_refused_for_nothing() {
    let project = drawing_project(BUILT_IN);
    let output = grida_fx(
        project.path(),
        &[
            "run",
            "case",
            "--live",
            "--max-usd",
            "1",
            "--run",
            "runs/one",
        ],
    );
    assert_eq!(status(&output), 1, "{}{}", stdout(&output), stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("\nresult    failed   spent $0.00\n"),
        "{text}"
    );
    // Refused before sending (capability_refused), so settled at $0.
    assert!(
        text.contains(
            "\nfailed    draw#1: image.generate on gpt-image-2.5-sunburst@openai was refused: \
             OPENAI_API_KEY is not set\n"
        ),
        "{text}"
    );
    let folder = project.path().join("runs/one");
    assert_spent_nothing(&folder);
    let events = events(&folder);
    let node_failed = events.iter().find(|e| e["event"] == "node_failed").unwrap();
    assert!(
        node_failed
            .to_string()
            .contains("OPENAI_API_KEY is not set"),
        "{node_failed}"
    );
}

#[test]
fn integrated_a_live_call_with_the_network_off_sends_nothing() {
    let project = drawing_project(BUILT_IN);
    let output = command(
        project.path(),
        &[
            "run",
            "case",
            "--live",
            "--max-usd",
            "1",
            "--run",
            "runs/one",
        ],
    )
    .env("OPENAI_API_KEY", MADE_UP_KEY)
    .output()
    .unwrap();
    assert_eq!(status(&output), 1, "{}{}", stdout(&output), stderr(&output));
    assert_no_key(&output);
    let text = stdout(&output);
    assert!(
        text.contains("\nresult    failed   spent $0.00\n"),
        "{text}"
    );
    let failed = text
        .lines()
        .find(|l| l.starts_with("failed    draw#1: "))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        failed.starts_with(
            "failed    draw#1: image.generate on gpt-image-2.5-sunburst@openai was refused: "
        ),
        "{text}"
    );
    assert!(failed.contains("GRIDA_FX_NETWORK"), "{text}");
    assert_spent_nothing(&project.path().join("runs/one"));
    // The key is in no record: not in the run, not in the cache.
    let mut files = Vec::new();
    every_file(project.path(), &mut files);
    for (path, bytes) in files {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(MADE_UP_KEY),
            "{}",
            path.display()
        );
    }
}

#[test]
fn integrated_a_live_run_refuses_a_malformed_key_file_before_planning() {
    let project = drawing_project(BUILT_IN);
    std::fs::write(
        project.path().join(".env"),
        format!("OPENAI_API_KEY={MADE_UP_KEY}\nOPENAI_API_KEY={MADE_UP_KEY}\n"),
    )
    .unwrap();
    let live = ["run", "case", "--live", "--max-usd", "1"];
    let output = command(project.path(), &live)
        .env_remove("GRIDA_FX_DISABLE_DOTENV")
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}{}", stdout(&output), stderr(&output));
    assert_no_key(&output);
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    assert!(
        stderr(&output).starts_with("grida-fx: .env"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("OPENAI_API_KEY"),
        "{}",
        stderr(&output)
    );
    assert!(!project.path().join("runs").exists());
    // A base URL that holds a key is refused the same way.
    let output = command(project.path(), &live)
        .env("OPENAI_API_KEY", MADE_UP_KEY)
        .env(
            "OPENAI_BASE_URL",
            format!("https://proxy.example.test/{MADE_UP_KEY}"),
        )
        .output()
        .unwrap();
    assert_eq!(status(&output), 2, "{}{}", stdout(&output), stderr(&output));
    assert_no_key(&output);
    assert!(
        stderr(&output).starts_with("grida-fx: OPENAI_BASE_URL"),
        "{}",
        stderr(&output)
    );
    // Without --live neither is read: the uncached call is refused as not live.
    let output = command(project.path(), &["run", "case", "--run", "runs/one"])
        .env_remove("GRIDA_FX_DISABLE_DOTENV")
        .output()
        .unwrap();
    assert_eq!(status(&output), 1, "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).contains("run with --live"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_run_replays_a_recorded_call_and_delivers_it() {
    let project = conformance_project("cache-replay");
    let stored = std::fs::read(project.path().join(
        ".fx/cache/files/f3/f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4",
    ))
    .unwrap();
    let run = |extra: &[&str]| {
        let mut argv = vec!["run", "case", "--routes", "routes.yaml"];
        argv.extend_from_slice(extra);
        grida_fx(project.path(), &argv)
    };
    let output = run(&["--run", "runs/one", "--deliver", "image=out/lantern.png"]);
    assert_eq!(status(&output), 0, "{}{}", stdout(&output), stderr(&output));
    let text = stdout(&output);
    assert!(
        text.ends_with(
            "run       runs/one\nresult    ok   spent $0.00\ndelivered out/lantern.png\n"
        ),
        "{text}"
    );
    assert_eq!(
        std::fs::read(project.path().join("out/lantern.png")).unwrap(),
        stored
    );
    assert_eq!(
        std::fs::read(project.path().join("runs/one/outputs/image.png")).unwrap(),
        stored
    );
    // The same folder again resumes it; the delivered file holds the same bytes already.
    let output = run(&["--run", "runs/one", "--deliver", "image=out/lantern.png"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        !stdout(&output).contains("delivered"),
        "{}",
        stdout(&output)
    );
    // A one-file output cannot be delivered per key.
    let output = run(&["--run", "runs/one", "--deliver", "image=out/{key}.png"]);
    assert_eq!(status(&output), 2);
    let diagnostics = stderr(&output);
    let lines: Vec<_> = diagnostics.lines().collect();
    assert_eq!(lines.len(), 3);
    let invocation = lines[0].strip_prefix("invocation ").unwrap();
    assert_eq!(invocation.len(), 16);
    assert!(invocation.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(matches!(
        lines[1],
        "control   available" | "control   unavailable"
    ));
    assert_eq!(
        lines[2],
        "grida-fx: --deliver image=out/{key}.png: image is one file, so no {key}"
    );
    // Without --run, a new folder under the project's runs folder, named from here.
    let output = run(&[]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let line = stdout(&output)
        .lines()
        .find(|l| l.starts_with("run       "))
        .unwrap()
        .to_string();
    let named = line.trim_start_matches("run       ");
    assert!(named.starts_with("runs/case/"), "{named}");
    assert!(named.ends_with("-1"), "{named}");
    assert!(project.path().join(named).join("plan.json").is_file());
    let plan =
        parse(&std::fs::read_to_string(project.path().join(named).join("plan.json")).unwrap());
    assert_eq!(plan["takes_file"], "workflows/case.takes.yaml");
    // project and inspect read the run.
    let output = grida_fx(project.path(), &["project", "runs/one"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let projected = parse(&stdout(&output));
    assert_eq!(projected["instances"]["draw#1"]["state"], "succeeded");
    assert_eq!(projected["run"]["run_started"]["resumed"], true);
    let output = grida_fx(project.path(), &["inspect", "runs/one", "--verify"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).ends_with("verified  1 files\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn integrated_a_folder_of_another_plan_is_refused() {
    let project = conformance_project("cache-replay");
    let argv = [
        "run",
        "case",
        "--routes",
        "routes.yaml",
        "--run",
        "runs/one",
    ];
    let output = grida_fx(project.path(), &argv);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let file = project.path().join("workflows/case.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replace("title: A paid call", "title: Another paid call"),
    )
    .unwrap();
    let output = grida_fx(project.path(), &argv);
    assert_eq!(status(&output), 1, "{}", stderr(&output));
    assert!(
        stdout(&output).ends_with(
            "refused: runs/one holds a run of another workflow or other inputs; choose a new \
             folder\n"
        ),
        "{}",
        stdout(&output)
    );
}

/// The Python that hosts node bodies in end-to-end runs, when one is there.
fn python_host() -> Option<PathBuf> {
    if let Some(python) = std::env::var_os("GRIDA_FX_PYTHON").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(python));
    }
    let venv = repository().join("python/.venv/bin/python");
    venv.is_file().then_some(venv)
}

/// Runs `grida-fx` with a Python node host.
fn grida_fx_with_python(cwd: &Path, python: &Path, args: &[&str]) -> Output {
    command(cwd, args)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("GRIDA_FX_PYTHON", python)
        .output()
        .unwrap()
}

#[test]
fn integrated_run_project_end_to_end() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    let project = conformance_project("run-project");
    let run = [
        "run",
        "case",
        "--routes",
        "routes.yaml",
        "--inputs",
        "inputs.yaml",
        "--run",
        "runs/one",
    ];
    let output = grida_fx_with_python(project.path(), &python, &run);
    assert_eq!(status(&output), 0, "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).ends_with("run       runs/one\nresult    ok   spent $0.00\n"));
    assert_eq!(
        std::fs::read_to_string(project.path().join("runs/one/outputs/all.txt")).unwrap(),
        "ada=ADA|bo=BO"
    );
    let output = grida_fx_with_python(project.path(), &python, &["project", "runs/one"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let projected = parse(&stdout(&output));
    for id in ["loud['ada']#1", "loud['bo']#1", "joined#1"] {
        assert_eq!(projected["instances"][id]["state"], "succeeded", "{id}");
        assert_eq!(projected["instances"][id]["cache"], "miss", "{id}");
    }
    // A second folder is answered by the result cache.
    let mut again = run;
    again[7] = "runs/two";
    let output = grida_fx_with_python(project.path(), &python, &again);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    let output = grida_fx_with_python(project.path(), &python, &["project", "runs/two"]);
    let projected = parse(&stdout(&output));
    for id in ["loud['ada']#1", "loud['bo']#1", "joined#1"] {
        assert_eq!(projected["instances"][id]["cache"], "hit", "{id}");
    }
}

#[test]
fn integrated_inspect_verifies_a_run_of_another_take() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    let project = conformance_project("run-takes");
    let run = |folder: &str| {
        let argv = ["run", "case", "--routes", "routes.yaml", "--run", folder];
        let output = grida_fx_with_python(project.path(), &python, &argv);
        assert_eq!(status(&output), 0, "{}{}", stdout(&output), stderr(&output));
    };
    run("runs/one");
    let output = grida_fx_with_python(project.path(), &python, &["reroll", "runs/one", "a"]);
    assert_eq!(status(&output), 0, "{}", stderr(&output));
    run("runs/two");
    // a#2's files are placed in a folder of their own, and inspect finds them there.
    let output = grida_fx_with_python(
        project.path(),
        &python,
        &["inspect", "runs/two", "--verify", "--json"],
    );
    assert_eq!(status(&output), 0, "{}{}", stdout(&output), stderr(&output));
    let document = parse(&stdout(&output));
    assert_eq!(
        document["verification"],
        json!({"verified": true, "problems": []})
    );
    let paths: Vec<&str> = document["run"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|step| step["files"].as_array().unwrap())
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["files/a#2/text.txt", "files/b/text.txt"]);
    assert_eq!(
        std::fs::read_to_string(project.path().join("runs/two/files/a#2/text.txt")).unwrap(),
        "take 2: hello"
    );
}

#[test]
fn run_refuses_a_stand_in_it_cannot_use_before_planning() {
    let project = drawing_project(ROUTED);
    std::fs::write(
        project.path().join("stand_in.py"),
        "def answer(call):\n    pass\n",
    )
    .unwrap();
    std::fs::write(project.path().join("stand_in.txt"), "").unwrap();
    std::fs::write(project.path().join("stand_in.cjs"), "").unwrap();
    std::fs::write(project.path().join("types.d.ts"), "").unwrap();
    let cases: [(&[&str], &str); 12] = [
        (
            &["--live", "--stand-in", "stand_in.py#answer"],
            "--stand-in cannot be used with --live",
        ),
        (
            &["--yes-up-to", "1", "--stand-in", "stand_in.py#answer"],
            "--stand-in cannot be used with --yes-up-to",
        ),
        (
            &["--stand-in", "stand_in.py"],
            "--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -",
        ),
        (
            &["--stand-in", "stand_in.txt#answer"],
            "--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -",
        ),
        (
            &["--stand-in", "stand_in.py#1answer"],
            "--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -",
        ),
        (
            &["--stand-in", "stand_in.py#"],
            "--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -",
        ),
        (
            &["--stand-in", "stand_in.cjs#answer"],
            "--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -",
        ),
        (
            &["--stand-in", "types.d.ts#answer"],
            "--stand-in takes <file>#<function> for a .py, .ts, .mts, .js or .mjs file, or -",
        ),
        (
            &["--stand-in", "missing.py#answer"],
            "no stand-in file missing.py",
        ),
        (
            &["--stand-in", "missing.ts#answer"],
            "no stand-in file missing.ts",
        ),
        (
            &["--stand-in", "-"],
            "--stand-in - needs the stand-in's socket on standard input",
        ),
        (
            &["--stand-in=-"],
            "--stand-in - needs the stand-in's socket on standard input",
        ),
    ];
    for (extra, message) in cases {
        let mut argv = vec!["run", "case", "--routes", "routes.yaml"];
        argv.extend_from_slice(extra);
        let output = grida_fx(project.path(), &argv);
        assert_eq!(status(&output), 2, "{extra:?}");
        assert!(stdout(&output).is_empty(), "{extra:?}");
        assert_eq!(
            stderr(&output),
            format!("grida-fx: {message}\n"),
            "{extra:?}"
        );
    }
    assert!(!project.path().join("runs").exists());
    assert!(!project.path().join(".fx").exists());
    let help = stdout(&grida_fx(project.path(), &["run", "--help"]));
    assert!(help.contains("--stand-in"), "{help}");
}

#[test]
fn integrated_a_stand_in_fault_names_no_private_path_in_the_record() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let outer = folder.path().canonicalize().unwrap();
    let project = outer.join("proj");
    copy_folder(
        &repository().join("conformance/stand-in-flags/in"),
        &project,
    );
    // The stand-in lives beside the project and names itself and the working directory.
    std::fs::create_dir(outer.join("tools")).unwrap();
    std::fs::write(
        outer.join("tools/oops.py"),
        "import os\n\ndef answer(call):\n    raise RuntimeError(\"cannot read \" + \
         os.path.abspath(__file__) + \" from \" + os.getcwd())\n",
    )
    .unwrap();
    let argv = [
        "run",
        "case",
        "--routes",
        "routes.yaml",
        "--run",
        "runs/one",
        "--stand-in",
        "../tools/oops.py#answer",
    ];
    let output = grida_fx_with_python(&project, &python, &argv);
    assert_eq!(status(&output), 1, "{}{}", stdout(&output), stderr(&output));
    let stopped = "the stand-in failed: RuntimeError: cannot read ../tools/oops.py from .";
    assert!(
        stdout(&output).contains(&format!("stopped   {stopped}\n")),
        "{}",
        stdout(&output)
    );
    let mut files = Vec::new();
    every_file(&project.join("runs/one"), &mut files);
    for (path, bytes) in files {
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(&*outer.to_string_lossy()),
            "{}: {text}",
            path.display()
        );
    }
    let finished = events(&project.join("runs/one"))
        .into_iter()
        .find(|event| event["event"] == "run_finished")
        .expect("the run finished");
    assert_eq!(finished["stopped"], stopped);
}

#[test]
fn integrated_a_stand_in_that_exits_mid_run_stops_the_run() {
    let Some(python) = python_host() else {
        eprintln!("skipped: no Python with the grida package");
        return;
    };
    // A step that sleeps 30 seconds and makes no call; the stand-in host exits a second after it
    // was loaded, while the step still runs.
    let project = conformance_project("run-timeout");
    let root = project.path();
    let workflow = std::fs::read_to_string(root.join("workflows/case.yaml")).unwrap();
    std::fs::write(
        root.join("workflows/case.yaml"),
        workflow.replace("timeout: 1", "timeout: 25"),
    )
    .unwrap();
    std::fs::write(
        root.join("gone.py"),
        "import os\nimport threading\n\nthreading.Timer(1.0, os._exit, (3,)).start()\n\n\n\
         def answer(call):\n    return None\n",
    )
    .unwrap();
    let started = std::time::Instant::now();
    let argv = [
        "run",
        "case",
        "--routes",
        "routes.yaml",
        "--run",
        "runs/one",
        "--stand-in",
        "gone.py#answer",
    ];
    let output = grida_fx_with_python(root, &python, &argv);
    assert_eq!(status(&output), 1, "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).ends_with("stopped   the stand-in answerer exited\n"),
        "{}",
        stdout(&output)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let events = events(&root.join("runs/one"));
    let problem = events
        .iter()
        .find(|event| event["event"] == "problem")
        .expect("a problem");
    assert_eq!(problem["where"], "stand_in");
    assert_eq!(problem["message"], "the stand-in answerer exited");
    let finished = events.last().unwrap();
    assert_eq!(finished["event"], "run_finished");
    assert_eq!(finished["stopped"], "the stand-in answerer exited");
    assert_eq!(finished["incomplete"], true);
}

/// A 1x1 PNG.
#[cfg(unix)]
const DOT_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// Serves a stand-in on `socket` until the engine ends the session: `initialize`, an answer of
/// [`DOT_PNG`] to every `stand_in.answer`, `shutdown`. Returns the methods it was sent, in order.
#[cfg(unix)]
fn serve_stand_in(socket: std::os::unix::net::UnixStream) -> Vec<String> {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut writer = socket.try_clone().unwrap();
    let mut reader = BufReader::new(socket);
    let mut methods = Vec::new();
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                return methods;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length: ") {
                length = Some(n.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; length.unwrap()];
        reader.read_exact(&mut body).unwrap();
        let message: Value = serde_json::from_slice(&body).unwrap();
        let method = message["method"].as_str().unwrap_or_default().to_string();
        methods.push(method.clone());
        let result = match method.as_str() {
            "initialize" => json!({"protocol": "fx-node-protocol-v1",
                "host": {"language": "test", "version": "1", "sdk_version": "1"}}),
            "stand_in.answer" => json!({"files": {"image": {"base64": DOT_PNG}}, "data": null}),
            "shutdown" => Value::Null,
            _ => continue,
        };
        let body =
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
                .unwrap();
        writer
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .unwrap();
        writer.write_all(&body).unwrap();
        writer.flush().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn integrated_a_stand_in_on_standard_input_answers_the_run() {
    let project = drawing_project(ROUTED);
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let child = command(
        project.path(),
        &[
            "run",
            "case",
            "--routes",
            "routes.yaml",
            "--run",
            "runs/one",
            "--stand-in",
            "-",
        ],
    )
    .stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
        theirs,
    )))
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .unwrap();
    let served = std::thread::spawn(move || serve_stand_in(ours));
    let output = child.wait_with_output().unwrap();
    let methods = served.join().unwrap();
    assert_eq!(status(&output), 0, "{}{}", stdout(&output), stderr(&output));
    assert_eq!(
        methods,
        ["initialize", "stand_in.answer", "shutdown", "exit"]
    );
    let text = stdout(&output);
    assert!(text.contains("run       runs/one\nstand-in  -\n"), "{text}");
    assert!(text.contains("result    ok   spent $0.00"), "{text}");
    // Kept apart: the stand-in store has the answer, the store has nothing.
    let store = project.path().join(".fx/cache");
    assert!(store.join("stand-in/calls").is_dir());
    assert!(!store.join("calls").exists());
    let plan = parse(&std::fs::read_to_string(project.path().join("runs/one/plan.json")).unwrap());
    assert_eq!(plan["stand_in"], json!(true));

    // The folder refuses a run without a stand-in, and the takes file is not changed for it.
    let output = grida_fx(
        project.path(),
        &[
            "run",
            "case",
            "--routes",
            "routes.yaml",
            "--run",
            "runs/one",
        ],
    );
    assert_eq!(status(&output), 1);
    assert!(
        stdout(&output).contains(
            "refused: runs/one holds a stand-in run; resume it with --stand-in, or choose a new \
             folder"
        ),
        "{}",
        stdout(&output)
    );
    for argv in [
        &["pick", "runs/one", "draw", "1"][..],
        &["reroll", "runs/one", "draw"][..],
    ] {
        let output = grida_fx(project.path(), argv);
        assert_eq!(status(&output), 1, "{argv:?}");
        assert_eq!(
            stdout(&output),
            "refused: runs/one holds a stand-in run, and stand-in runs never change the \
             workflow's takes file\n"
        );
    }
    assert!(!project.path().join("workflows/case.takes.yaml").exists());

    // inspect says so.
    let output = grida_fx(project.path(), &["inspect", "runs/one"]);
    assert_eq!(status(&output), 0);
    assert!(
        stdout(&output).starts_with("case  \u{b7}  one  \u{b7}  stand-in\n"),
        "{}",
        stdout(&output)
    );
    let output = grida_fx(project.path(), &["inspect", "runs/one", "--json"]);
    assert_eq!(parse(&stdout(&output))["run"]["stand_in"], json!(true));
}
