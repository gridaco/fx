//! `grida-fx project <run>`: a run's record projected to its state, as JSON (conformance case
//! `run-project`).
//!
//! Reads only `events.jsonl` (`events::read_events_tolerant`, up to its first unreadable line; a
//! missing log is an error, exit 2), never `plan.json`:
//! - `instances`: by id, each event replacing the whole entry: `node_started` → `{state:
//!   "running", path}`; `node_finished` → `{state: "succeeded", path, cache, facts}` (`facts`
//!   `{}` when the event has none); `node_failed` / `node_skipped` → `{state: "failed" |
//!   "skipped", path, error}` (`error` when it is set and not empty, else `reason`, else `null`).
//!   Other events (`node_retry`, `call`, `budget_*`, `phase_planned`, `problem`) are left out;
//! - `run`: the last `run_started`, `run_finished` and `run_cancelled` event, each whole but for
//!   its `kind`.
//!
//! Printed with `print_json` (keys sorted canonically), exit 0.

use crate::cli::ProjectArgs;
use crate::print::print_json;
use crate::verbs::{event_name, read_log};
use grida_fx_core::Error;
use serde_json::{Map, Value, json};

/// Runs `grida-fx project`.
pub fn run(args: &ProjectArgs) -> Result<u8, Error> {
    let cwd = super::planning::working_directory()?;
    let events = read_log(&cwd.join(&args.run), &args.run, true)?;
    print_json(&projection(&events));
    Ok(0)
}

/// The projection of a run's events (module doc).
pub(crate) fn projection(events: &[Value]) -> Value {
    let mut instances = Map::new();
    let mut run = Map::new();
    for event in events {
        let Some(object) = event.as_object() else {
            continue;
        };
        let name = event_name(event);
        let id = object.get("id").and_then(Value::as_str);
        let path = || object.get("path").cloned().unwrap_or(Value::Null);
        match (name, id) {
            (Some("node_started"), Some(id)) => {
                instances.insert(id.to_string(), json!({"state": "running", "path": path()}));
            }
            (Some("node_finished"), Some(id)) => {
                instances.insert(
                    id.to_string(),
                    json!({
                        "state": "succeeded",
                        "path": path(),
                        "cache": object.get("cache").cloned().unwrap_or(Value::Null),
                        "facts": object.get("facts").cloned().unwrap_or_else(|| json!({})),
                    }),
                );
            }
            (Some(ended @ ("node_failed" | "node_skipped")), Some(id)) => {
                let state = if ended == "node_failed" {
                    "failed"
                } else {
                    "skipped"
                };
                let error = match object.get("error") {
                    Some(error) if truthy(error) => error.clone(),
                    _ => object.get("reason").cloned().unwrap_or(Value::Null),
                };
                instances.insert(
                    id.to_string(),
                    json!({"state": state, "path": path(), "error": error}),
                );
            }
            (Some(summary @ ("run_started" | "run_finished" | "run_cancelled")), _) => {
                let mut kept = object.clone();
                kept.remove("kind");
                run.insert(summary.to_string(), Value::Object(kept));
            }
            _ => {}
        }
    }
    json!({"instances": instances, "run": run})
}

/// Whether a value counts as given: not null, `false`, `0`, an empty string, list or object.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(members) => !members.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(event: &str, fields: Value) -> Value {
        let mut object = json!({
            "kind": "fx-run-events-v1",
            "event": event,
            "invocation_id": "0123456789abcdef",
            "plan": "a".repeat(64),
            "offset_ms": 3,
        });
        for (k, v) in fields.as_object().unwrap() {
            object[k] = v.clone();
        }
        object
    }

    #[test]
    fn each_event_replaces_an_instances_entry() {
        let events = vec![
            envelope(
                "run_started",
                json!({"workflow": "case", "resumed": false, "ceiling_usd": null,
                       "charged_usd": 0, "estimate": {"low_usd": 0, "high_usd": 0}}),
            ),
            envelope(
                "phase_planned",
                json!({"phase": 1, "steps": 4, "high_usd": 0}),
            ),
            envelope("node_started", json!({"id": "ok#1", "path": "ok"})),
            envelope("node_started", json!({"id": "nope#1", "path": "nope"})),
            envelope(
                "node_finished",
                json!({"id": "ok#1", "path": "ok", "cache": "miss",
                       "facts": {"cost_usd": null}, "outputs": {}, "duration_ms": 4}),
            ),
            envelope(
                "node_failed",
                json!({"id": "nope#1", "path": "nope", "error": "refused on purpose",
                       "facts": {"seen": "x"}, "duration_ms": 2}),
            ),
            envelope(
                "node_skipped",
                json!({"id": "after#1", "path": "after", "reason": "something it reads failed",
                       "blocked": true}),
            ),
            envelope(
                "node_retry",
                json!({"id": "bang#1", "attempt": 1, "error": "x"}),
            ),
            envelope("node_started", json!({"id": "bang#1", "path": "bang"})),
            envelope(
                "node_failed",
                json!({"id": "bang#1", "path": "bang", "error": null}),
            ),
            envelope(
                "node_failed",
                json!({"id": "odd#1", "path": "odd", "error": ""}),
            ),
            envelope(
                "node_finished",
                json!({"id": "bare#1", "path": "bare", "cache": "hit", "outputs": {}}),
            ),
            envelope(
                "call",
                json!({"id": "ok#1", "capability": "image.generate", "route": "img-a@acme",
                       "call": "b".repeat(64), "cached": true, "cost_usd": 0}),
            ),
            envelope(
                "run_finished",
                json!({"ok": false, "incomplete": false, "stopped": null, "charged_usd": 0,
                       "failed": ["nope#1", "after#1", "bang#1"], "outputs": {}}),
            ),
        ];
        let projected = projection(&events);
        assert_eq!(
            projected["instances"],
            json!({
                "ok#1": {"state": "succeeded", "path": "ok", "cache": "miss",
                         "facts": {"cost_usd": null}},
                "nope#1": {"state": "failed", "path": "nope", "error": "refused on purpose"},
                "after#1": {"state": "skipped", "path": "after",
                            "error": "something it reads failed"},
                "bang#1": {"state": "failed", "path": "bang", "error": null},
                "odd#1": {"state": "failed", "path": "odd", "error": null},
                "bare#1": {"state": "succeeded", "path": "bare", "cache": "hit", "facts": {}},
            })
        );
        let run = projected["run"].as_object().unwrap();
        assert_eq!(
            run.keys().collect::<Vec<_>>(),
            ["run_started", "run_finished"]
        );
        let finished = &run["run_finished"];
        assert!(finished.get("kind").is_none());
        assert_eq!(finished["event"], "run_finished");
        assert_eq!(finished["invocation_id"], "0123456789abcdef");
        assert_eq!(finished["plan"], "a".repeat(64));
        assert_eq!(finished["offset_ms"], 3);
        assert_eq!(finished["ok"], false);
    }

    #[test]
    fn the_last_summary_of_each_name_wins_and_a_cancelled_step_stays_running() {
        let events = vec![
            envelope("run_started", json!({"resumed": false})),
            envelope("node_started", json!({"id": "s#1", "path": "s"})),
            envelope(
                "run_cancelled",
                json!({"reason": "interrupted", "charged_usd": 0}),
            ),
            envelope("run_started", json!({"resumed": true})),
            envelope("run_finished", json!({"ok": true})),
        ];
        let projected = projection(&events);
        assert_eq!(
            projected["instances"],
            json!({"s#1": {"state": "running", "path": "s"}})
        );
        let run = &projected["run"];
        assert_eq!(run["run_started"]["resumed"], true);
        assert_eq!(run["run_cancelled"]["reason"], "interrupted");
        assert_eq!(run["run_finished"]["ok"], true);
    }

    #[test]
    fn nothing_recorded_projects_to_empty_maps() {
        assert_eq!(
            projection(&[json!("not an event"), json!({"event": "node_started"})]),
            json!({"instances": {}, "run": {}})
        );
    }
}
