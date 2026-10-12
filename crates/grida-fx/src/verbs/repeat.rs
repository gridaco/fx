//! `run_started`'s `repeat`, and the `next` line `reroll` and `pick` print from it (spec/store.md
//! §8 "Repeating a run"; spec/schemas/fx-run-events-v1.schema.json `repeat`).
//!
//! Written by `run` ([`recorded`]): `workflow` (a workflow id as given) or `file` (a workflow
//! file, or a builder's module with `function`), `inputs` and `routes` (left out when empty),
//! `max_usd` (when given) and `live`; every path POSIX and relative to the planning project's
//! root, each part exactly as the file system names it once symbolic links are resolved. None
//! when the record could not repeat the invocation: workflow input flags, `--arg`,
//! `--stand-in`, a target, inputs file or route table outside the planning project or whose path
//! is not UTF-8, a builder run from anywhere but the planning project's root (a builder is
//! called with the working directory), or an SDK's invocation (`GRIDA_FX_CANCEL_SOURCE=sdk`,
//! whose inputs file is temporary; spec/sdk.md).
//!
//! Read by `reroll` and `pick` from the latest `run_started` ([`next_line`]), against the
//! planning project whose run index records the run folder (`takes::RunTakes::open`): `<command>
//! run <target> [--inputs <file>]… [--routes <file>]… [--max-usd <amount>] [--live]`, each path
//! relative to the working directory, `./` before one that would start with `-`, and each word
//! outside `[A-Za-z0-9_./:@%+,-]` (or `=` after its first character) in single quotes. `--live`
//! is that `run_started`'s: a run resumed without `--live` repeats without it. With no
//! `repeat`, one that does not read as the schema says, no planning project the index names, a
//! workflow id or a workflow file other than YAML read from a working directory in another
//! project (planning would find the workflow there), a builder read from anywhere but that
//! project's root, or a path that is not UTF-8: `run <workflow id> again with the same inputs
//! and options`.

use crate::cli::RunArgs;
use crate::print::{labelled, shown_path};
use crate::verbs::takes::is_inside;
use crate::verbs::{event_name, text};
use grida_fx_core::docs::project::Project;
use grida_fx_core::money::Usd;
use grida_fx_core::project::{Planner, Target, is_workflow_file};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The environment variable an SDK sets on the engine's `run` (spec/sdk.md).
const SDK_SOURCE: &str = "GRIDA_FX_CANCEL_SOURCE";

/// The `repeat` of a `run` invocation, or `None` when its record could not repeat it.
pub(crate) fn recorded(args: &RunArgs, planner: &Planner, cwd: &Path) -> Option<Value> {
    let from_sdk = std::env::var(SDK_SOURCE).as_deref() == Ok("sdk");
    if from_sdk || !args.rest.is_empty() || !args.arg.is_empty() || args.stand_in.is_some() {
        return None;
    }
    let root = planner.project.root.canonicalize().ok()?;
    let inside = |typed: &str| exactly_inside(&root, &cwd.join(typed));
    let mut repeat = Map::new();
    match Target::parse(&args.target) {
        Target::Builder { file, function } => {
            // A builder is called with the working directory: only its root repeats it.
            if cwd.canonicalize().ok()? != root {
                return None;
            }
            repeat.insert("file".into(), Value::from(inside(&file)?));
            repeat.insert("function".into(), Value::from(function));
        }
        Target::Workflow(text) if is_workflow_file(&text) => {
            repeat.insert("file".into(), Value::from(inside(&text)?));
        }
        Target::Workflow(_) => {
            repeat.insert(
                "workflow".into(),
                Value::from(planner.workflow.workflow.id.clone()),
            );
        }
    }
    for (member, files) in [("inputs", &args.inputs), ("routes", &args.routes)] {
        if !files.is_empty() {
            let paths = files
                .iter()
                .map(|typed| inside(typed).map(Value::from))
                .collect::<Option<Vec<Value>>>()?;
            repeat.insert(member.into(), Value::Array(paths));
        }
    }
    if let Some(text) = &args.max_usd {
        repeat.insert("max_usd".into(), Usd::parse(text).ok()?.to_value());
    }
    repeat.insert("live".into(), Value::Bool(args.live));
    Some(Value::Object(repeat))
}

/// `path` relative to `root` (canonical) as POSIX, once symbolic links are resolved: `None` when
/// it does not exist, lies outside, or a part is not UTF-8 (never converted lossily).
fn exactly_inside(root: &Path, path: &Path) -> Option<String> {
    let path = path.canonicalize().ok()?;
    let parts = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<&str>>>()?;
    Some(parts.join("/")).filter(|relative| is_inside(relative))
}

/// The `next` line of `reroll` and `pick` (module doc). `root` is the planning project whose run
/// index records the run folder (`None` when none does); `cwd` the working directory;
/// `workflow` the run's workflow id.
pub(crate) fn next_line(
    events: &[Value],
    workflow: &str,
    root: Option<&Path>,
    cwd: &Path,
) -> String {
    let started = events
        .iter()
        .rfind(|event| event_name(event) == Some("run_started"));
    let advice = match started
        .and_then(|event| event.get("repeat"))
        .zip(root)
        .and_then(|(repeat, root)| command(repeat, root, cwd))
    {
        Some(words) => format!("{} {}", crate::invoker::command(), words.join(" ")),
        None => format!("run {workflow} again with the same inputs and options"),
    };
    labelled("next", &advice)
}

/// `run <target> [options]` from a recorded `repeat`, each word quoted for a shell.
fn command(repeat: &Value, root: &Path, cwd: &Path) -> Option<Vec<String>> {
    // Shown paths are made from these: neither may be converted lossily.
    root.to_str()?;
    cwd.to_str()?;
    let shown = |relative: &str| -> Option<String> {
        is_inside(relative).then(|| path_word(&shown_path(&under(root, relative), cwd)))
    };
    let mut words = vec!["run".to_string()];
    let workflow = text(repeat, "workflow");
    let file = text(repeat, "file");
    let function = text(repeat, "function");
    // A workflow id, a workflow file other than YAML, and a builder plan in the project found
    // from the working directory (spec/store.md §8): only from there does the command plan the
    // same workflow. A builder is also called with the working directory: only its root.
    let planning_here = || Project::find(cwd).is_ok_and(|project| project.root == root);
    match (workflow, file, function) {
        (Some(id), None, None) if is_workflow_id(id) && planning_here() => {
            words.push(shell_word(id));
        }
        (None, Some(file), None)
            if is_workflow_file(file) && (found_from_file(file) || planning_here()) =>
        {
            words.push(shown(file)?);
        }
        (None, Some(file), Some(function)) if is_inside(file) && cwd == root => {
            let target = format!("{}:{function}", shown_path(&under(root, file), cwd));
            let Target::Builder { .. } = Target::parse(&target) else {
                return None;
            };
            words.push(path_word(&target));
        }
        _ => return None,
    }
    for (member, option) in [("inputs", "--inputs"), ("routes", "--routes")] {
        let Some(files) = repeat.get(member) else {
            continue;
        };
        for file in files.as_array().filter(|files| !files.is_empty())? {
            words.push(option.to_string());
            words.push(shown(file.as_str()?)?);
        }
    }
    if let Some(amount) = repeat.get("max_usd") {
        let amount = Usd::from_value(amount).ok()?;
        words.push("--max-usd".to_string());
        words.push(amount.to_string());
    }
    if repeat.get("live")?.as_bool()? {
        words.push("--live".to_string());
    }
    let known = [
        "workflow", "file", "function", "inputs", "routes", "max_usd", "live",
    ];
    repeat
        .as_object()?
        .keys()
        .all(|key| known.contains(&key.as_str()))
        .then_some(words)
}

/// Whether planning finds the project from the workflow file (a YAML one), not the working
/// directory (grida_fx_core::project).
fn found_from_file(file: &str) -> bool {
    matches!(
        Path::new(file)
            .extension()
            .and_then(|suffix| suffix.to_str()),
        Some("yaml" | "yml")
    )
}

/// A project-relative POSIX path joined to the project's root.
fn under(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// A workflow id: `^[a-z0-9]+(?:-[a-z0-9]+)*$`, at most 96 characters.
fn is_workflow_id(id: &str) -> bool {
    id.len() <= 96
        && !id.is_empty()
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// A relative path as a word: `./` before one that starts with `-`, which quoting would not keep
/// from reading as an option.
fn path_word(path: &str) -> String {
    if path.starts_with('-') {
        shell_word(&format!("./{path}"))
    } else {
        shell_word(path)
    }
}

/// A word as a POSIX shell (and zsh) reads it back: as it is when every character is plain, else
/// in single quotes (a quote inside written `'\''`). `=` is plain only after the first character:
/// zsh expands a word that starts with one.
fn shell_word(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c);
    if !word.is_empty() && !word.starts_with('=') && word.chars().all(plain) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A log whose one `run_started` records `repeat`.
    fn started(repeat: Value) -> Vec<Value> {
        vec![json!({"event": "run_started", "repeat": repeat})]
    }

    fn next(events: &[Value], cwd: &str) -> String {
        next_line(events, "case", Some(Path::new("/p")), Path::new(cwd))
    }

    #[test]
    fn the_command_repeats_the_run() {
        let events = started(json!({
            "file": "workflows/case.yaml", "inputs": ["inputs/x.yaml", "inputs/my y.yaml"],
            "routes": ["routes.yaml"], "max_usd": 2.5, "live": true
        }));
        assert_eq!(
            next(&events, "/p"),
            "next      grida-fx run workflows/case.yaml --inputs inputs/x.yaml --inputs \
             'inputs/my y.yaml' --routes routes.yaml --max-usd 2.5 --live"
        );
        assert_eq!(
            next(&events, "/p/workflows"),
            "next      grida-fx run case.yaml --inputs ../inputs/x.yaml --inputs \
             '../inputs/my y.yaml' --routes ../routes.yaml --max-usd 2.5 --live"
        );
        assert_eq!(
            next(&started(json!({"workflow": "case", "live": false})), "/p"),
            "next      grida-fx run case"
        );
        assert_eq!(
            next(
                &started(json!({"file": "nodes/build.py", "function": "make", "live": false})),
                "/p"
            ),
            "next      grida-fx run nodes/build.py:make"
        );
    }

    #[test]
    fn the_latest_invocation_decides() {
        let mut events = started(json!({"workflow": "case", "live": false}));
        events.push(json!({"event": "run_finished"}));
        events.extend(started(json!({"workflow": "case", "live": true})));
        assert_eq!(next(&events, "/p"), "next      grida-fx run case --live");
        // An SDK resumed it: its record cannot repeat it.
        events.push(json!({"event": "run_started"}));
        assert_eq!(
            next(&events, "/p"),
            "next      run case again with the same inputs and options"
        );
    }

    #[test]
    fn a_record_that_cannot_repeat_the_run_gives_advice() {
        let advice = "next      run case again with the same inputs and options";
        assert_eq!(next(&[], "/p"), advice);
        for repeat in [
            json!({"workflow": "case"}),
            json!({"workflow": "case", "live": "yes"}),
            json!({"workflow": "Case", "live": false}),
            json!({"workflow": "case", "file": "case.yaml", "live": false}),
            json!({"file": "../case.yaml", "live": false}),
            json!({"file": "/p/case.yaml", "live": false}),
            json!({"file": "case.txt", "live": false}),
            json!({"file": "nodes/build.txt", "function": "make", "live": false}),
            json!({"file": "nodes/build.py", "function": "a-b", "live": false}),
            json!({"workflow": "case", "inputs": [], "live": false}),
            json!({"workflow": "case", "inputs": ["a/../../x.yaml"], "live": false}),
            json!({"workflow": "case", "max_usd": -1, "live": false}),
            json!({"workflow": "case", "args": ["n=v"], "live": false}),
            json!("run case"),
        ] {
            assert_eq!(next(&started(repeat.clone()), "/p"), advice, "{repeat}");
        }
    }

    #[test]
    fn only_the_planning_project_plans_the_same_workflow() {
        let advice = "next      run case again with the same inputs and options";
        // No planning project the run index names.
        let events = started(json!({"file": "workflows/case.yaml", "live": false}));
        assert_eq!(next_line(&events, "case", None, Path::new("/p")), advice);
        // A YAML file plans in its own project, from anywhere.
        assert_eq!(
            next(&events, "/q"),
            "next      grida-fx run ../p/workflows/case.yaml"
        );
        // A workflow id, a JSON file and a builder plan in the working directory's project.
        for repeat in [
            json!({"workflow": "case", "live": false}),
            json!({"file": "workflows/case.json", "live": false}),
            json!({"file": "nodes/build.py", "function": "make", "live": false}),
        ] {
            assert_eq!(next(&started(repeat.clone()), "/q"), advice, "{repeat}");
        }
        // A builder is called with the working directory: only the project's root repeats it.
        assert_eq!(
            next(
                &started(json!({"file": "nodes/build.py", "function": "make", "live": false})),
                "/p/nodes"
            ),
            advice
        );
    }

    #[test]
    fn paths_never_read_as_options() {
        let events = started(json!({
            "file": "-w/case.yaml", "inputs": ["-x.yaml", "=y.yaml"], "live": false
        }));
        assert_eq!(
            next(&events, "/p"),
            "next      grida-fx run ./-w/case.yaml --inputs ./-x.yaml --inputs '=y.yaml'"
        );
        assert_eq!(
            next(
                &started(json!({"file": "-b.py", "function": "make", "live": false})),
                "/p"
            ),
            "next      grida-fx run ./-b.py:make"
        );
    }

    #[test]
    fn shell_words() {
        assert_eq!(shell_word("inputs/x.yaml"), "inputs/x.yaml");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word(""), "''");
        // zsh expands a word that starts with `=`; one inside is plain.
        assert_eq!(shell_word("=x.yaml"), "'=x.yaml'");
        assert_eq!(shell_word("a=b.yaml"), "a=b.yaml");
        // Quoting does not keep a word from reading as an option: paths take `./`.
        assert_eq!(shell_word("-x.yaml"), "-x.yaml");
        assert_eq!(path_word("-x.yaml"), "./-x.yaml");
        assert_eq!(path_word("-my x.yaml"), "'./-my x.yaml'");
        assert_eq!(path_word("../-x.yaml"), "../-x.yaml");
        assert_eq!(path_word("=x.yaml"), "'=x.yaml'");
    }

    #[test]
    fn paths_inside_the_project_are_exact() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("inputs")).unwrap();
        std::fs::write(root.join("inputs/x.yaml"), "").unwrap();
        assert_eq!(
            exactly_inside(&root, &root.join("inputs/../inputs/x.yaml")).as_deref(),
            Some("inputs/x.yaml")
        );
        assert_eq!(
            exactly_inside(&root, &root.join("inputs/missing.yaml")),
            None
        );
        assert_eq!(exactly_inside(&root, &root), None);
        assert_eq!(exactly_inside(&root.join("inputs"), &root), None);
    }

    /// A path that is not UTF-8 is never recorded, lossily or otherwise.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_not_recorded() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let name = std::ffi::OsStr::from_bytes(b"in\xffputs.yaml");
        if std::fs::write(root.join(name), "").is_err() {
            return; // A file system that refuses the name cannot hold it.
        }
        assert_eq!(exactly_inside(&root, &root.join(name)), None);
    }
}
