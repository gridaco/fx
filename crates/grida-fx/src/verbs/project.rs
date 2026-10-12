//! `grida-fx project <run>`: a run's record projected to its state, as JSON (conformance case
//! `run-project`).
//!
//! Reads only `events.jsonl` (`events::read_events_tolerant`, up to its first unreadable line; a
//! missing log is an error, exit 2), never `plan.json`:
//! - `instances`: by id, each event replacing the whole entry: `node_started` → `{state:
//!   "running", path}`; `node_finished` → `{state: "succeeded", path, cache, facts}` (`facts`
//!   `{}` when the event has none); `node_failed` / `node_skipped` → `{state: "failed" |
//!   "skipped", path, error}` (`error` when it is set and not empty, else `reason`, else `null`).
//!   A step waiting for its route's slot (spec/store.md §8): a `call_waiting` for an entry that
//!   is `running` or `waiting` opens a wait, keyed by its `call`, and the `call_admitted` of the
//!   same `call` closes it; the step's next `node_*` or `node_retry` event, or any
//!   `run_started`, closes all its waits. An entry with an open wait is `{state: "waiting",
//!   path, waiting_for}` (the `route` of its earliest open wait, else `null`), and a `waiting`
//!   entry whose last wait closes is `{state: "running", path}`. Other events (`call`,
//!   `budget_*`, `phase_planned`, `problem`) are left out;
//! - `run`: the last `run_started`, `run_finished` and `run_cancelled` event, each whole but for
//!   its `kind`.
//!
//! Printed with `print_json` (keys sorted canonically), exit 0.

use crate::cli::ProjectArgs;
use crate::print::print_json;
use crate::verbs::{event_name, read_log};
use grida_fx_core::Error;
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

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
    // Each started step's open waits, in the order they began: call key → route.
    let mut waits: HashMap<String, IndexMap<Option<String>, Value>> = HashMap::new();
    for event in events {
        let Some(object) = event.as_object() else {
            continue;
        };
        let name = event_name(event);
        let id = object.get("id").and_then(Value::as_str);
        let path = || object.get("path").cloned().unwrap_or(Value::Null);
        if let (
            Some("node_started" | "node_finished" | "node_failed" | "node_skipped" | "node_retry"),
            Some(id),
        ) = (name, id)
        {
            // A start, an end or a new attempt leaves the step's waits behind.
            waits.remove(id);
        }
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
            (Some("node_retry"), Some(id)) => settle(&mut instances, id, None),
            (Some(call @ ("call_waiting" | "call_admitted")), Some(id)) => {
                if !matches!(
                    instances
                        .get(id)
                        .and_then(|entry| entry.get("state"))
                        .and_then(Value::as_str),
                    Some("running" | "waiting")
                ) {
                    continue;
                }
                let open = waits.entry(id.to_string()).or_default();
                let key = object
                    .get("call")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if call == "call_waiting" {
                    let route = object.get("route").cloned().unwrap_or(Value::Null);
                    open.entry(key).or_insert(route);
                } else {
                    open.shift_remove(&key);
                }
                settle(&mut instances, id, open.values().next());
            }
            (Some(summary @ ("run_started" | "run_finished" | "run_cancelled")), _) => {
                if summary == "run_started" {
                    // A new invocation leaves every earlier wait behind.
                    for id in std::mem::take(&mut waits).into_keys() {
                        settle(&mut instances, &id, None);
                    }
                }
                let mut kept = object.clone();
                kept.remove("kind");
                run.insert(summary.to_string(), Value::Object(kept));
            }
            _ => {}
        }
    }
    json!({"instances": instances, "run": run})
}

/// Shows a started step `waiting` for `route`, the route of its earliest open wait, or `running`
/// once it has none; any other entry stays as it is.
fn settle(instances: &mut Map<String, Value>, id: &str, route: Option<&Value>) {
    let Some(entry) = instances.get_mut(id) else {
        return;
    };
    if !matches!(
        entry.get("state").and_then(Value::as_str),
        Some("running" | "waiting")
    ) {
        return;
    }
    let path = entry.get("path").cloned().unwrap_or(Value::Null);
    *entry = match route {
        Some(route) => json!({"state": "waiting", "path": path, "waiting_for": route}),
        None => json!({"state": "running", "path": path}),
    };
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
    fn a_call_waiting_for_its_route_makes_its_step_waiting_until_admitted() {
        let call = |name: &str, id: &str, key: char, route: &str| {
            envelope(
                name,
                json!({"id": id, "capability": "image.generate", "route": route,
                       "call": key.to_string().repeat(64)}),
            )
        };
        let events = vec![
            envelope("node_started", json!({"id": "a#1", "path": "a"})),
            envelope("node_started", json!({"id": "b#1", "path": "b"})),
            envelope("node_started", json!({"id": "e#1", "path": "e"})),
            // Two calls of one body wait at once: one admitted, the step still waits.
            call("call_waiting", "a#1", '1', "img-a@acme"),
            call("call_waiting", "a#1", '2', "img-b@acme"),
            call("call_admitted", "a#1", '1', "img-a@acme"),
            call("call_waiting", "b#1", '3', "img-a@acme"),
            call("call_admitted", "b#1", '3', "img-a@acme"),
            // An admission of no open wait closes nothing; a new attempt closes them all.
            call("call_waiting", "e#1", '4', "img-a@acme"),
            call("call_admitted", "e#1", '5', "img-a@acme"),
            envelope(
                "node_retry",
                json!({"id": "e#1", "attempt": 2, "error": "e"}),
            ),
            call("call_admitted", "e#1", '4', "img-a@acme"),
            // Neither an unstarted nor an ended step waits.
            call("call_waiting", "c#1", '6', "img-a@acme"),
            envelope(
                "node_finished",
                json!({"id": "d#1", "path": "d", "cache": "miss", "outputs": {}}),
            ),
            call("call_waiting", "d#1", '7', "img-a@acme"),
            call("call_admitted", "d#1", '7', "img-a@acme"),
        ];
        let instances = projection(&events)["instances"].clone();
        assert_eq!(
            instances,
            json!({
                "a#1": {"state": "waiting", "path": "a", "waiting_for": "img-b@acme"},
                "b#1": {"state": "running", "path": "b"},
                "e#1": {"state": "running", "path": "e"},
                "d#1": {"state": "succeeded", "path": "d", "cache": "miss", "facts": {}},
            })
        );
        // The stray admission left e#1's wait open until its new attempt.
        let stray = projection(&events[..10])["instances"]["e#1"].clone();
        assert_eq!(stray["state"], "waiting");
        // The other call's admission ends the step's wait.
        let mut admitted = events.clone();
        admitted.push(call("call_admitted", "a#1", '2', "img-b@acme"));
        assert_eq!(
            projection(&admitted)["instances"]["a#1"],
            json!({"state": "running", "path": "a"})
        );
        // A new invocation leaves the waits of the last one behind, for good.
        let mut resumed = events.clone();
        resumed.push(envelope("run_started", json!({"resumed": true})));
        resumed.push(call("call_admitted", "a#1", '2', "img-b@acme"));
        assert_eq!(
            projection(&resumed)["instances"]["a#1"],
            json!({"state": "running", "path": "a"})
        );
    }

    #[test]
    fn nothing_recorded_projects_to_empty_maps() {
        assert_eq!(
            projection(&[json!("not an event"), json!({"event": "node_started"})]),
            json!({"instances": {}, "run": {}})
        );
    }
}
