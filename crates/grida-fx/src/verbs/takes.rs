//! `reroll`, `pick` and `takes list|mv` (docs/guide/04-cost-and-cache.md "Takes";
//! spec/schemas/fx-takes-v1.schema.json).
//!
//! The takes file of a run is the one its `plan.json` names (`takes_file`, relative to the project
//! found from the working directory, `Project::find`; `plan.json` holds no absolute path). A
//! folder with no readable fx-graph-v1 `plan.json` is `<folder> is not a run folder`; one whose
//! `plan.json` names no takes file inside the project is `<folder> was not started by grida-fx
//! run` (both exit 2). The run's log is read as `project` reads it (up to its first unreadable
//! line); a folder with no log has run nothing.
//!
//! - `reroll <run> <step>`: the latest take of `<step>` (a step path, keys included) any
//!   `node_started` of the run's log names (the last number of its `take`); none: `<run> never
//!   ran a step <step>` (exit 2); a step already at take 1000 cannot draw another (`<step> has
//!   used take 1000, the most takes a step may have`, exit 2). Writes `<step>: {take: latest + 1}`
//!   (an earlier `result` is dropped), prints `<takes file name>: <step> uses take <n> from now
//!   on` and `next      grida-fx run <workflow id>[ --live]`. Nothing runs and nothing is spent.
//! - `pick <run> <step> <take>`: `take` a whole number from 1 to 1000 (`a take is 1 or more` / `a
//!   take is at most 1000` / `a take is a whole number, not <text>`, exit 2, checked before the
//!   folder is read); `<step>` must be a step path the run's plan or log names (`<run> has no
//!   step <step>`, exit 2). The result is the digest of the first port holding exactly one file
//!   (ports in the event's order, which is the record's canonical order) in the outputs of the
//!   `node_finished` whose id is `<step>#<take>`, or `<step>#<a.….take>` for a nested take whose
//!   last number is the take; the last such event wins, and none leaves the entry without a
//!   `result`. Writes `{take, result?}` (a bare digest) and prints `<takes file name>: <step> uses
//!   take <n>`.
//! - `takes list <target>`: the target's takes file (found as planning finds it: next to the
//!   workflow file, or in the planning project's root for a workflow of another project); one
//!   line per entry, sorted by step path: `<step>  take <n>[  <result>]`. No file prints nothing.
//!   A builder target is refused: its takes file sits next to the module that builds it, which
//!   only running the builder can tell.
//! - `takes mv <target> <old> <new>`: `<file name> has no entry <old>` (exit 2); an existing
//!   `<new>` is refused (`<file name> already has an entry <new>`); `<new>` must be a step path
//!   (`<new> is not a step path`); prints `<file name>: <old> is now <new>`.
//!
//! Every write renders the whole file with `docs::takes::render_takes` (comments and unknown
//! members of a hand-edited file are not kept) and replaces it atomically (`store::atomic_write`).

use crate::cli::{PickArgs, RerollArgs, TakesArgs, TakesVerb};
use crate::print::{labelled, print_line};
use crate::verbs::{event_name, read_log, read_plan, text, workflow_id};
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::takes::{MAX_TAKE, TakeChoice, Takes, read_takes, render_takes};
use grida_fx_core::docs::{Schema, validate};
use grida_fx_core::project::{Target, load_target};
use grida_fx_core::{Error, ErrorKind};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

/// `grida-fx reroll`.
pub fn reroll(args: &RerollArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let run = RunTakes::open(&args.run, &cwd)?;
    let events = read_log(&run.folder, &args.run, false)?;
    let latest = latest_take(&events, &args.step)
        .ok_or_else(|| Error::usage(format!("{} never ran a step {}", args.run, args.step)))?;
    if latest >= MAX_TAKE {
        return Err(Error::usage(format!(
            "{} has used take {MAX_TAKE}, the most takes a step may have",
            args.step
        )));
    }
    let take = latest + 1;
    let mut takes = run.file.read()?;
    takes.insert(args.step.clone(), TakeChoice { take, result: None });
    run.file.write(&takes)?;
    print_line(&format!(
        "{}: {} uses take {take} from now on",
        run.file.name, args.step
    ));
    let live = if args.live { " --live" } else { "" };
    print_line(&labelled(
        "next",
        &format!("grida-fx run {}{live}", run.workflow),
    ));
    Ok(0)
}

/// `grida-fx pick`.
pub fn pick(args: &PickArgs) -> Result<u8, Error> {
    let take = parse_take(&args.take)?;
    let cwd = super::planning::working_directory()?;
    let run = RunTakes::open(&args.run, &cwd)?;
    let events = read_log(&run.folder, &args.run, false)?;
    if !names_step(&run.plan, &events, &args.step) {
        return Err(Error::usage(format!(
            "{} has no step {}",
            args.run, args.step
        )));
    }
    let result = picked_result(&events, &args.step, take);
    let mut takes = run.file.read()?;
    takes.insert(args.step.clone(), TakeChoice { take, result });
    run.file.write(&takes)?;
    print_line(&format!(
        "{}: {} uses take {take}",
        run.file.name, args.step
    ));
    Ok(0)
}

/// `grida-fx takes list|mv`.
pub fn takes(args: &TakesArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    match &args.verb {
        TakesVerb::List { target } => {
            let file = TakesFile::of_target(target, &cwd)?;
            for line in list_lines(&file.read()?) {
                print_line(&line);
            }
        }
        TakesVerb::Mv { target, old, new } => {
            let file = TakesFile::of_target(target, &cwd)?;
            let mut takes = file.read()?;
            if move_entry(&mut takes, &file.name, old, new)? {
                file.write(&takes)?;
            }
            print_line(&format!("{}: {old} is now {new}", file.name));
        }
    }
    Ok(0)
}

/// A takes file: where it is, and how messages name it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TakesFile {
    path: PathBuf,
    /// Its file name (`case.takes.yaml`): its header and the verbs' lines name it so.
    name: String,
}

impl TakesFile {
    fn at(path: PathBuf) -> TakesFile {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        TakesFile { path, name }
    }

    /// The takes file of a workflow file or id, found as planning finds it.
    fn of_target(target: &str, cwd: &Path) -> Result<TakesFile, Error> {
        if let Target::Builder { .. } = Target::parse(target) {
            return Err(Error::usage(format!(
                "{target} is a builder; takes list and takes mv take a workflow file or id"
            )));
        }
        let mut host = crate::print::host();
        let (project, workflow) = load_target(target, cwd, &IndexMap::new(), &mut host)?;
        let home = Project::find(&workflow.path)?;
        let folder = if home.root != project.root {
            project.root.clone()
        } else {
            workflow
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.root.clone())
        };
        Ok(TakesFile::at(grida_fx_core::docs::takes::takes_path(
            &folder,
            &workflow.workflow.id,
        )))
    }

    fn read(&self) -> Result<Takes, Error> {
        read_takes(&self.path, &self.name)
    }

    fn write(&self, takes: &Takes) -> Result<(), Error> {
        let text = render_takes(&self.name, takes);
        grida_fx_runtime::store::atomic_write(&self.path, text.as_bytes(), false)
            .map_err(|error| Error::io(&self.name, &error))
    }
}

/// A run folder and the takes file its plan names.
struct RunTakes {
    folder: PathBuf,
    plan: Value,
    workflow: String,
    file: TakesFile,
}

impl RunTakes {
    fn open(given: &str, cwd: &Path) -> Result<RunTakes, Error> {
        let folder = cwd.join(given);
        let plan = read_plan(&folder, given)?;
        let relative = plan
            .get("takes_file")
            .and_then(Value::as_str)
            .filter(|relative| is_inside(relative))
            .ok_or_else(|| Error::usage(format!("{given} was not started by grida-fx run")))?;
        let project = Project::find(cwd)?;
        let path = relative
            .split('/')
            .fold(project.root.clone(), |path, part| path.join(part));
        Ok(RunTakes {
            workflow: workflow_id(&plan).to_string(),
            folder,
            plan,
            file: TakesFile::at(path),
        })
    }
}

/// Whether a recorded path stays inside the project: relative, POSIX, no `.`/`..` parts.
fn is_inside(relative: &str) -> bool {
    !relative.is_empty()
        && !relative.contains('\\')
        && relative.split('/').all(|part| !part.is_empty())
        && Path::new(relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// The take number of `pick` (module doc).
fn parse_take(text: &str) -> Result<u32, Error> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::usage(format!(
            "a take is a whole number, not {text}"
        )));
    }
    if text.starts_with('-') || digits.bytes().all(|b| b == b'0') {
        return Err(Error::usage("a take is 1 or more"));
    }
    match digits.parse::<u32>() {
        Ok(take) if take <= MAX_TAKE => Ok(take),
        _ => Err(Error::usage(format!("a take is at most {MAX_TAKE}"))),
    }
}

/// The latest take of `step` any `node_started` names (the last number of its take path).
fn latest_take(events: &[Value], step: &str) -> Option<u32> {
    events
        .iter()
        .filter(|e| event_name(e) == Some("node_started") && text(e, "path") == Some(step))
        .map(|e| {
            e.get("take")
                .and_then(Value::as_array)
                .and_then(|take| take.last())
                .and_then(Value::as_u64)
                .map_or(1, |n| u32::try_from(n).unwrap_or(u32::MAX))
        })
        .max()
}

/// Whether the run's plan or log names the step path.
fn names_step(plan: &Value, events: &[Value], step: &str) -> bool {
    let planned = plan
        .get("instances")
        .and_then(Value::as_array)
        .is_some_and(|instances| instances.iter().any(|i| text(i, "path") == Some(step)));
    planned
        || events.iter().any(|e| {
            event_name(e).is_some_and(|name| name.starts_with("node_"))
                && text(e, "path") == Some(step)
        })
}

/// Whether an instance id is `<step>#<takes>` whose last take number is `take`.
fn is_take_of(id: &str, step: &str, take: u32) -> bool {
    let Some((path, takes)) = id.rsplit_once('#') else {
        return false;
    };
    let numbers: Vec<&str> = takes.split('.').collect();
    path == step
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && numbers.last().and_then(|n| n.parse::<u32>().ok()) == Some(take)
}

/// The digest a pick records (module doc).
fn picked_result(events: &[Value], step: &str, take: u32) -> Option<String> {
    let mut result = None;
    for event in events {
        let finished = event_name(event) == Some("node_finished")
            && text(event, "path") == Some(step)
            && text(event, "id").is_some_and(|id| is_take_of(id, step, take));
        if finished {
            result = event
                .get("outputs")
                .and_then(Value::as_object)
                .and_then(|outputs| outputs.values().find_map(one_file_digest));
        }
    }
    result
}

/// The digest of an encoded value that is exactly one file.
fn one_file_digest(encoded: &Value) -> Option<String> {
    encoded
        .get("file")
        .and_then(|file| file.get("digest"))
        .and_then(Value::as_str)
        .filter(|digest| grida_fx_core::value::is_digest(digest))
        .map(str::to_string)
}

/// `takes list`'s lines.
fn list_lines(takes: &Takes) -> Vec<String> {
    let mut entries: Vec<(&String, &TakeChoice)> = takes.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .map(|(step, choice)| match &choice.result {
            Some(result) => format!("{step}  take {}  {result}", choice.take),
            None => format!("{step}  take {}", choice.take),
        })
        .collect()
}

/// `takes mv` on the entries: `Ok(true)` when the file changes.
fn move_entry(takes: &mut Takes, name: &str, old: &str, new: &str) -> Result<bool, Error> {
    if !takes.contains_key(old) {
        return Err(Error::usage(format!("{name} has no entry {old}")));
    }
    if old == new {
        return Ok(false);
    }
    if takes.contains_key(new) {
        return Err(Error::usage(format!("{name} already has an entry {new}")));
    }
    if validate(Schema::Takes, &json!({ new: {"take": 1} }), name).is_err() {
        return Err(Error::new(
            ErrorKind::Usage,
            format!("{new} is not a step path"),
        ));
    }
    if let Some(choice) = takes.shift_remove(old) {
        takes.insert(new.to_string(), choice);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_core::ErrorKind;

    fn event(name: &str, fields: Value) -> Value {
        let mut object = json!({"kind": "fx-run-events-v1", "event": name});
        for (k, v) in fields.as_object().unwrap() {
            object[k] = v.clone();
        }
        object
    }

    fn file(digest: &str) -> Value {
        json!({"file": {"digest": digest, "kind": "text/plain", "name": "a/text", "size": 3}})
    }

    #[test]
    fn take_numbers() {
        assert_eq!(parse_take("1").unwrap(), 1);
        assert_eq!(parse_take("1000").unwrap(), 1000);
        assert_eq!(parse_take("007").unwrap(), 7);
        let message = |text: &str| {
            let error = parse_take(text).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Usage);
            error.message
        };
        assert_eq!(message("0"), "a take is 1 or more");
        assert_eq!(message("-1"), "a take is 1 or more");
        assert_eq!(message("-99999999999999999999"), "a take is 1 or more");
        assert_eq!(message("1001"), "a take is at most 1000");
        assert_eq!(message("99999999999999999999"), "a take is at most 1000");
        assert_eq!(message("x"), "a take is a whole number, not x");
        assert_eq!(message("1.5"), "a take is a whole number, not 1.5");
        assert_eq!(message(""), "a take is a whole number, not ");
        assert_eq!(message("-"), "a take is a whole number, not -");
    }

    #[test]
    fn the_latest_take_comes_from_every_start_of_the_step() {
        let events = vec![
            event(
                "node_started",
                json!({"id": "a#1", "path": "a", "take": [1]}),
            ),
            event(
                "node_started",
                json!({"id": "a#3", "path": "a", "take": [3]}),
            ),
            event(
                "node_started",
                json!({"id": "a#2", "path": "a", "take": [2]}),
            ),
            event(
                "node_started",
                json!({"id": "g.a#2.7", "path": "g.a", "take": [2, 7]}),
            ),
            event("node_finished", json!({"id": "b#9", "path": "b"})),
        ];
        assert_eq!(latest_take(&events, "a"), Some(3));
        assert_eq!(latest_take(&events, "g.a"), Some(7));
        assert_eq!(latest_take(&events, "b"), None);
        assert_eq!(
            latest_take(&[event("node_started", json!({"path": "c"}))], "c"),
            Some(1)
        );
    }

    #[test]
    fn a_pick_records_the_first_single_file_port_of_its_take() {
        let one = "1".repeat(64);
        let two = "2".repeat(64);
        let three = "3".repeat(64);
        let events = vec![
            event(
                "node_finished",
                json!({"id": "a#1", "path": "a", "outputs": {"text": file(&one)}}),
            ),
            event(
                "node_finished",
                json!({"id": "a#2", "path": "a", "outputs": {
                    "items": {"list": [file(&three)]},
                    "text": file(&two),
                    "zz": file(&three),
                }}),
            ),
            event(
                "node_finished",
                json!({"id": "a#12", "path": "a", "outputs": {"text": file(&three)}}),
            ),
            event(
                "node_finished",
                json!({"id": "g.a#3.2", "path": "g.a", "outputs": {"text": file(&three)}}),
            ),
            event(
                "node_finished",
                json!({"id": "only#1", "path": "only", "outputs": {"items": {"collection": []}}}),
            ),
        ];
        assert_eq!(picked_result(&events, "a", 1), Some(one.clone()));
        assert_eq!(picked_result(&events, "a", 2), Some(two));
        assert_eq!(picked_result(&events, "g.a", 2), Some(three.clone()));
        assert_eq!(picked_result(&events, "g.a", 3), None);
        assert_eq!(picked_result(&events, "a", 7), None);
        assert_eq!(picked_result(&events, "only", 1), None);
        // The last event of the take wins.
        let mut again = events.clone();
        again.push(event(
            "node_finished",
            json!({"id": "a#1", "path": "a", "outputs": {"text": file(&three)}}),
        ));
        assert_eq!(picked_result(&again, "a", 1), Some(three));
    }

    #[test]
    fn take_ids() {
        assert!(is_take_of("a#2", "a", 2));
        assert!(is_take_of("g.a#3.2", "g.a", 2));
        assert!(is_take_of("s['x#1']#4", "s['x#1']", 4));
        assert!(!is_take_of("a#12", "a", 2));
        assert!(!is_take_of("ba#2", "a", 2));
        assert!(!is_take_of("a#2.", "a", 2));
        assert!(!is_take_of("a", "a", 1));
    }

    #[test]
    fn a_step_is_named_by_the_plan_or_the_log() {
        let plan = json!({"instances": [{"id": "a#1", "path": "a"}]});
        let events = vec![event(
            "node_started",
            json!({"id": "late['k']#1", "path": "late['k']"}),
        )];
        assert!(names_step(&plan, &events, "a"));
        assert!(names_step(&plan, &events, "late['k']"));
        assert!(!names_step(&plan, &events, "late"));
        assert!(!names_step(&json!({}), &[], "a"));
    }

    #[test]
    fn recorded_takes_files_stay_inside_the_project() {
        assert!(is_inside("workflows/case.takes.yaml"));
        assert!(is_inside("case.takes.yaml"));
        for outside in [
            "",
            "/abs/x.yaml",
            "../x.yaml",
            "a/../x",
            "./x",
            "a//b",
            "a\\b",
            "a/",
        ] {
            assert!(!is_inside(outside), "{outside}");
        }
    }

    fn choice(take: u32, result: Option<&str>) -> TakeChoice {
        TakeChoice {
            take,
            result: result.map(str::to_string),
        }
    }

    #[test]
    fn listing_sorts_by_step_path() {
        let digest = "a".repeat(64);
        let takes: Takes = [
            ("b".to_string(), choice(1, Some(&digest))),
            ("a".to_string(), choice(7, None)),
            ("a['é']".to_string(), choice(2, None)),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            list_lines(&takes),
            [
                "a  take 7".to_string(),
                "a['é']  take 2".to_string(),
                format!("b  take 1  {digest}"),
            ]
        );
        assert!(list_lines(&Takes::new()).is_empty());
    }

    #[test]
    fn moving_an_entry() {
        let mut takes: Takes = [
            ("a".to_string(), choice(2, None)),
            ("b".to_string(), choice(1, Some(&"c".repeat(64)))),
        ]
        .into_iter()
        .collect();
        let name = "case.takes.yaml";
        let message = |takes: &mut Takes, old: &str, new: &str| {
            let error = move_entry(takes, name, old, new).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Usage);
            error.message
        };
        assert_eq!(
            message(&mut takes, "zz", "c"),
            "case.takes.yaml has no entry zz"
        );
        assert_eq!(
            message(&mut takes, "a", "b"),
            "case.takes.yaml already has an entry b"
        );
        assert_eq!(
            message(&mut takes, "a", "Not A Path"),
            "Not A Path is not a step path"
        );
        assert!(!move_entry(&mut takes, name, "a", "a").unwrap());
        assert!(move_entry(&mut takes, name, "b", "entity['ada'].draw").unwrap());
        assert_eq!(
            takes.keys().collect::<Vec<_>>(),
            ["a", "entity['ada'].draw"]
        );
        assert_eq!(
            takes["entity['ada'].draw"],
            choice(1, Some(&"c".repeat(64)))
        );
    }

    /// A project whose workflow `case` lives in `workflows/`, and a run folder `runs/one` of it.
    fn project_with_a_run() -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(
            root.join("workflows/case.yaml"),
            "fx: workflow/v1\nid: case\ntitle: Case\nsteps:\n  draw:\n    uses: \
             fx/image.generate@1\n    with: { prompt: a kite }\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("runs/one")).unwrap();
        std::fs::write(
            root.join("runs/one/plan.json"),
            r#"{"kind": "fx-graph-v1", "workflow": {"id": "case"},
                "takes_file": "workflows/case.takes.yaml",
                "instances": [{"id": "draw#1", "path": "draw"}]}"#,
        )
        .unwrap();
        folder
    }

    #[test]
    fn a_runs_takes_file_is_the_one_its_plan_names() {
        let project = project_with_a_run();
        let root = project.path().canonicalize().unwrap();
        let run = RunTakes::open("runs/one", &root).unwrap();
        assert_eq!(run.workflow, "case");
        assert_eq!(
            run.file.path,
            root.join("workflows").join("case.takes.yaml")
        );
        assert_eq!(run.file.name, "case.takes.yaml");
        // From a folder inside the project, the same file.
        let inner = root.join("runs");
        let run = RunTakes::open("one", &inner).unwrap();
        assert_eq!(
            run.file.path,
            root.join("workflows").join("case.takes.yaml")
        );
        std::fs::write(
            root.join("runs/one/plan.json"),
            r#"{"kind": "fx-graph-v1", "workflow": {"id": "case"}, "takes_file": "../x.yaml"}"#,
        )
        .unwrap();
        let error = RunTakes::open("runs/one", &root).err().unwrap();
        assert_eq!(error.message, "runs/one was not started by grida-fx run");
        let error = RunTakes::open("runs/two", &root).err().unwrap();
        assert_eq!(error.message, "runs/two is not a run folder");
    }

    #[test]
    fn a_targets_takes_file_is_found_as_planning_finds_it() {
        let project = project_with_a_run();
        let root = project.path().canonicalize().unwrap();
        let file = TakesFile::of_target("case", &root).unwrap();
        assert_eq!(file.path, root.join("workflows").join("case.takes.yaml"));
        let file = TakesFile::of_target("workflows/case.yaml", &root).unwrap();
        assert_eq!(file.name, "case.takes.yaml");
        assert!(file.read().unwrap().is_empty());
        let error = TakesFile::of_target("build.py:make", &root).unwrap_err();
        assert_eq!(
            error.message,
            "build.py:make is a builder; takes list and takes mv take a workflow file or id"
        );
        let error = TakesFile::of_target("nosuch", &root).unwrap_err();
        assert!(error.message.contains("nosuch"), "{}", error.message);
    }

    #[test]
    fn a_written_takes_file_reads_back() {
        let project = project_with_a_run();
        let root = project.path().canonicalize().unwrap();
        let file = TakesFile::of_target("case", &root).unwrap();
        let takes: Takes = [("draw".to_string(), choice(2, Some(&"d".repeat(64))))]
            .into_iter()
            .collect();
        file.write(&takes).unwrap();
        assert_eq!(file.read().unwrap(), takes);
        let text = std::fs::read_to_string(&file.path).unwrap();
        assert!(text.starts_with(
            "# case.takes.yaml: written by `grida-fx reroll` and `grida-fx pick`; commit it\n"
        ));
    }
}
