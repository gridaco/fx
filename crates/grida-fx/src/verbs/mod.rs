//! One module per verb. Each returns `Ok(exit status)` or an error (exit 2).
//!
//! The verbs that read a run folder (`reroll`, `pick`, `project`, `inspect`) share [`read_plan`]
//! and [`read_log`]: a folder is named as the user typed it, relative to the working directory,
//! and messages name it that way (spec/store.md §8 "Nothing private").

pub mod cache;
pub mod control;
pub mod doctor;
pub mod inspect;
pub mod jobs;
pub mod lock;
pub mod nodes;
pub mod observe;
pub mod planning;
pub mod project;
pub mod run;
mod run_catalog;
mod run_set;
pub mod runs;
pub mod schema;
pub mod service;
pub mod takes;
pub mod view;

use grida_fx_core::{Error, ErrorKind};
use serde_json::Value;
use std::path::Path;

/// The `kind` of a run folder's `plan.json`.
const GRAPH_KIND: &str = "fx-graph-v1";

/// `<label>/<name>`, for messages about a file in a run folder.
fn inside(label: &str, name: &str) -> String {
    let label = label.trim_end_matches('/');
    if label.is_empty() {
        name.to_string()
    } else {
        format!("{label}/{name}")
    }
}

/// A run folder's `plan.json`: an fx-graph-v1 object naming its workflow's id. Anything else
/// (missing, unreadable, not JSON, another kind) is `<label> is not a run folder` (exit 2).
pub(crate) fn read_plan(folder: &Path, label: &str) -> Result<Value, Error> {
    let refused = || Error::usage(format!("{label} is not a run folder"));
    let bytes = std::fs::read(folder.join("plan.json")).map_err(|_| refused())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| refused())?;
    let document = grida_fx_core::value::parse_json(text).map_err(|_| refused())?;
    let is_graph = document.get("kind").and_then(Value::as_str) == Some(GRAPH_KIND);
    let has_id = document
        .get("workflow")
        .and_then(|w| w.get("id"))
        .and_then(Value::as_str)
        .is_some();
    if is_graph && has_id {
        Ok(document)
    } else {
        Err(refused())
    }
}

/// The workflow id `plan.json` records ([`read_plan`] checked it is there).
pub(crate) fn workflow_id(plan: &Value) -> &str {
    plan.get("workflow")
        .and_then(|w| w.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A run folder's events, oldest first, read up to the first line that cannot be read
/// (`events::read_events_tolerant`; nothing is written). A missing log is an error naming
/// `<label>/events.jsonl` when `required`, else no events.
pub(crate) fn read_log(folder: &Path, label: &str, required: bool) -> Result<Vec<Value>, Error> {
    let path = folder.join("events.jsonl");
    let named = inside(label, "events.jsonl");
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(Error::new(ErrorKind::Io, format!("{named}: not a file")));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(Error::io(&named, &error)),
    }
    grida_fx_runtime::events::read_events_tolerant(&path).map_err(|error| Error::io(&named, &error))
}

/// The event name of a record line.
pub(crate) fn event_name(event: &Value) -> Option<&str> {
    event.get("event").and_then(Value::as_str)
}

/// A string member of an event.
pub(crate) fn text<'a>(event: &'a Value, member: &str) -> Option<&'a str> {
    event.get(member).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_must_be_a_graph_document() {
        let folder = tempfile::tempdir().unwrap();
        let refused = |folder: &Path| read_plan(folder, "runs/one").unwrap_err();
        let error = refused(folder.path());
        assert_eq!(error.kind, ErrorKind::Usage);
        assert_eq!(error.message, "runs/one is not a run folder");
        for text in [
            "not json",
            "[]",
            "{\"kind\": \"fx-graph-v1\"}",
            "{\"kind\": \"other\", \"workflow\": {\"id\": \"case\"}}",
        ] {
            std::fs::write(folder.path().join("plan.json"), text).unwrap();
            assert_eq!(
                refused(folder.path()).message,
                "runs/one is not a run folder"
            );
        }
        std::fs::write(
            folder.path().join("plan.json"),
            "{\"kind\": \"fx-graph-v1\", \"workflow\": {\"id\": \"case\"}}",
        )
        .unwrap();
        let plan = read_plan(folder.path(), "runs/one").unwrap();
        assert_eq!(workflow_id(&plan), "case");
    }

    #[test]
    fn a_missing_log_is_empty_unless_it_is_required() {
        let folder = tempfile::tempdir().unwrap();
        assert!(
            read_log(folder.path(), "runs/one", false)
                .unwrap()
                .is_empty()
        );
        let error = read_log(folder.path(), "runs/one/", true).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Io);
        assert_eq!(error.message, "runs/one/events.jsonl: no such file");
        std::fs::create_dir(folder.path().join("events.jsonl")).unwrap();
        let error = read_log(folder.path(), "runs/one", false).unwrap_err();
        assert_eq!(error.message, "runs/one/events.jsonl: not a file");
    }

    #[test]
    fn a_log_is_read_up_to_its_first_broken_line() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("events.jsonl"),
            "{\"event\":\"run_started\"}\n{\"event\":\"run_finished\"}\n{\"event\":",
        )
        .unwrap();
        let events = read_log(folder.path(), "r", true).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(event_name(&events[1]), Some("run_finished"));
    }
}
