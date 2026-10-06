//! Resuming (spec/store.md §8 "Resuming"): the finished steps of a folder's earlier invocations.
//!
//! Walk the events in order, by `id`: `node_started` forgets the id; `node_finished` keeps it as
//! succeeded with its facts and its outputs decoded (`events::decode`, files read through the
//! store) when every file is present; `node_failed` and `node_skipped` forget it. Failed and
//! skipped steps are therefore decided again, and a step whose file left the store runs again.
//! Replayed results emit nothing new and their files are not placed again. Replay is keyed by
//! instance id: every event it reads is of this plan, since the runner refuses a folder whose
//! `plan.json`, or any of whose events, names another plan (both read under `run.lock`).
//!
//! A `node_finished` event whose outputs do not decode (a file missing from the store, a member
//! that is not an encoded value) is skipped. Its facts are the event's `facts` object, empty when
//! it has none.

use crate::store::Store;
use grida_fx_core::expand::{NodeResult, ResultStatus};
use grida_fx_core::val::Val;
use indexmap::IndexMap;
use serde_json::Value;

/// The results the earlier invocations finished (module doc).
pub fn replay(events: &[Value], store: &Store) -> IndexMap<String, NodeResult> {
    replay_with(events, &|value| crate::events::decode(value, store))
}

/// [`replay`] with the decoder given, so the walk can be tested on its own.
fn replay_with(
    events: &[Value],
    decode: &dyn Fn(&Value) -> Result<Val, String>,
) -> IndexMap<String, NodeResult> {
    let mut results: IndexMap<String, NodeResult> = IndexMap::new();
    for event in events {
        let Some(id) = event.get("id").and_then(Value::as_str) else {
            continue;
        };
        match event.get("event").and_then(Value::as_str) {
            Some("node_started" | "node_failed" | "node_skipped") => {
                results.shift_remove(id);
            }
            Some("node_finished") => {
                if let Some(result) = finished(event, decode) {
                    results.shift_remove(id);
                    results.insert(id.to_string(), result);
                }
            }
            _ => {}
        }
    }
    results
}

/// A `node_finished` event as a succeeded result, when every output decodes.
fn finished(event: &Value, decode: &dyn Fn(&Value) -> Result<Val, String>) -> Option<NodeResult> {
    let outputs = match event.get("outputs") {
        Some(Value::Object(outputs)) => outputs
            .iter()
            .map(|(port, encoded)| Ok((port.clone(), decode(encoded)?)))
            .collect::<Result<IndexMap<String, Val>, String>>()
            .ok()?,
        None => IndexMap::new(),
        Some(_) => return None,
    };
    let facts = match event.get("facts") {
        Some(Value::Object(facts)) => facts
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        _ => IndexMap::new(),
    };
    Some(NodeResult {
        status: ResultStatus::Succeeded,
        outputs,
        facts,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Decodes `{"value": v}` and `{"none": true}`; a file decodes only when its digest is
    /// "present".
    fn decode(value: &Value) -> Result<Val, String> {
        if let Some(plain) = value.get("value") {
            return Ok(Val::from_json(plain));
        }
        if value.get("none").is_some() {
            return Ok(Val::Missing);
        }
        if let Some(file) = value.get("file") {
            let digest = file["digest"].as_str().unwrap_or_default();
            return if digest == "present" {
                Ok(Val::Str(format!("file {digest}")))
            } else {
                Err(format!("{digest} is not in the store"))
            };
        }
        Err("not an encoded value".into())
    }

    fn event(name: &str, id: &str) -> Value {
        json!({"kind": "fx-run-events-v1", "event": name, "id": id})
    }

    fn finished_event(id: &str, outputs: Value, facts: Value) -> Value {
        json!({"event": "node_finished", "id": id, "outputs": outputs, "facts": facts,
               "cache": "miss", "duration_ms": 3})
    }

    #[test]
    fn finished_steps_come_back_with_their_outputs_and_facts() {
        let events = vec![
            json!({"event": "run_started", "workflow": "case"}),
            event("node_started", "a#1"),
            finished_event(
                "a#1",
                json!({"text": {"value": "ADA"}}),
                json!({"cost_usd": null}),
            ),
            event("node_started", "b#1"),
            finished_event(
                "b#1",
                json!({"image": {"file": {"digest": "present"}}}),
                json!({}),
            ),
        ];
        let results = replay_with(&events, &decode);
        let ids: Vec<&String> = results.keys().collect();
        assert_eq!(ids, ["a#1", "b#1"]);
        assert_eq!(
            results["a#1"],
            NodeResult {
                status: ResultStatus::Succeeded,
                outputs: [("text".to_string(), Val::Str("ADA".into()))].into(),
                facts: [("cost_usd".to_string(), Value::Null)].into(),
                error: None,
            }
        );
        assert_eq!(
            results["b#1"].outputs["image"],
            Val::Str("file present".into())
        );
    }

    #[test]
    fn failed_skipped_and_restarted_steps_are_decided_again() {
        let events = vec![
            event("node_started", "a#1"),
            finished_event("a#1", json!({}), json!({})),
            event("node_started", "b#1"),
            json!({"event": "node_failed", "id": "b#1", "error": "kaboom"}),
            json!({"event": "node_skipped", "id": "c#1", "reason": "something it reads failed",
                   "blocked": true}),
            // A later invocation started a#1 again and never finished it.
            event("node_started", "a#1"),
            event("node_started", "d#1"),
            finished_event("d#1", json!({}), json!({})),
            json!({"event": "node_skipped", "id": "d#1", "error": "no candidate exists"}),
            event("node_started", "e#1"),
            finished_event("e#1", json!({}), json!({})),
        ];
        let results = replay_with(&events, &decode);
        let ids: Vec<&String> = results.keys().collect();
        assert_eq!(ids, ["e#1"]);
    }

    #[test]
    fn a_missing_file_or_a_bad_member_skips_the_event() {
        let events = vec![
            event("node_started", "a#1"),
            finished_event(
                "a#1",
                json!({"image": {"file": {"digest": "gone"}}, "text": {"value": 1}}),
                json!({}),
            ),
            event("node_started", "b#1"),
            finished_event("b#1", json!({"x": {"what": 1}}), json!({})),
            event("node_started", "c#1"),
            json!({"event": "node_finished", "id": "c#1", "outputs": [1]}),
            // No outputs member: nothing to decode; no facts member: empty facts.
            json!({"event": "node_finished", "id": "d#1"}),
            // Events without an id are not about a step.
            json!({"event": "node_finished", "outputs": {}}),
            json!({"event": "node_finished", "id": 7, "outputs": {}}),
        ];
        let results = replay_with(&events, &decode);
        let ids: Vec<&String> = results.keys().collect();
        assert_eq!(ids, ["d#1"]);
        assert!(results["d#1"].facts.is_empty());
        assert!(results["d#1"].outputs.is_empty());
    }

    #[test]
    fn the_last_finish_wins() {
        let events = vec![
            event("node_started", "a#1"),
            finished_event("a#1", json!({"n": {"value": 1}}), json!({})),
            event("node_started", "b#1"),
            finished_event("b#1", json!({}), json!({})),
            finished_event("a#1", json!({"n": {"value": 2}}), json!({"k": true})),
        ];
        let results = replay_with(&events, &decode);
        let ids: Vec<&String> = results.keys().collect();
        assert_eq!(ids, ["b#1", "a#1"]);
        assert_eq!(results["a#1"].outputs["n"], Val::Number(2.0));
        assert_eq!(results["a#1"].facts["k"], json!(true));
    }
}
