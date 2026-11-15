//! Recorded documentation snapshots. Presentation never fabricates executable members.

use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::OnceLock;

fn schema() -> &'static jsonschema::Validator {
    static SCHEMA: OnceLock<jsonschema::Validator> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let document: Value = serde_json::from_str(include_str!(
            "../../../spec/schemas/fx-viewer-run-v1.schema.json"
        ))
        .expect("embedded viewer schema");
        jsonschema::validator_for(&json!({
            "$defs": document["$defs"],
            "type": "array", "items": {"$ref": "#/$defs/note"}
        }))
        .expect("embedded note schema compiles")
    })
}

fn valid(value: &Value) -> bool {
    if !schema().is_valid(value) {
        return false;
    }
    let mut ids = BTreeSet::new();
    value.as_array().is_some_and(|notes| {
        notes.iter().all(|note| {
            let owner = note["scope_id"].as_str().unwrap_or("root");
            let name = note["name"].as_str().unwrap_or_default();
            note["id"] == format!("note:{owner}:{name}")
                && ids.insert(note["id"].as_str().unwrap_or_default())
                && note.get("source").is_none_or(|source| {
                    source["path"].as_str().is_some_and(|path| {
                        !path.is_empty()
                            && !path.chars().any(char::is_control)
                            && !path.contains(['\\', ':'])
                            && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                    })
                })
        })
    })
}

fn owned(note: &Value, scopes: Option<&Value>) -> bool {
    note["scope_id"].is_null()
        || scopes
            .and_then(Value::as_array)
            .is_some_and(|scopes| scopes.iter().any(|scope| scope["id"] == note["scope_id"]))
}

pub(crate) fn validate_plan(plan: &Value) -> Result<(), &'static str> {
    if let Some(notes) = plan.get("notes")
        && (!valid(notes)
            || notes
                .as_array()
                .is_none_or(|notes| notes.iter().any(|note| !owned(note, plan.get("scopes")))))
    {
        return Err("Note metadata has an unsupported shape or unknown owner.");
    }
    Ok(())
}

/// First evidence for an occurrence wins: a resume cannot rewrite an earlier captured note.
#[derive(Default)]
pub(crate) struct Captured {
    present: bool,
    entries: IndexMap<String, Value>,
}

impl Captured {
    pub(crate) fn from_plan(plan: &Value, warnings: &mut Vec<String>) -> Self {
        let mut captured = Self::default();
        captured.capture(plan.get("notes"), warnings);
        captured
    }

    pub(crate) fn capture(&mut self, notes: Option<&Value>, warnings: &mut Vec<String>) {
        let Some(notes) = notes else { return };
        if !valid(notes) {
            warnings.push("Recorded notes have an unsupported shape and were ignored.".into());
            return;
        }
        self.present = true;
        for note in notes.as_array().expect("validated notes") {
            self.entries
                .entry(note["id"].as_str().expect("validated id").to_owned())
                .or_insert_with(|| note.clone());
        }
    }

    pub(crate) fn project(self, scopes: Option<&Value>) -> Option<Value> {
        self.present.then(|| {
            Value::Array(
                self.entries
                    .into_values()
                    .filter(|note| owned(note, scopes))
                    .collect(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(name: &str, text: &str) -> Value {
        json!({"id":format!("note:root:{name}"),"name":name,"scope_id":null,"about":null,"format":"text","text":text})
    }

    #[test]
    fn first_capture_is_immutable_but_new_occurrences_can_join() {
        let mut warnings = Vec::new();
        let mut notes =
            Captured::from_plan(&json!({"notes":[note("help", "original")]}), &mut warnings);
        notes.capture(
            Some(&json!([
                note("help", "changed"),
                note("later", "new occurrence")
            ])),
            &mut warnings,
        );
        let projected = notes.project(None).unwrap();
        assert_eq!(projected[0]["text"], "original");
        assert_eq!(projected[1]["text"], "new occurrence");
        assert!(warnings.is_empty());
    }

    #[test]
    fn malformed_metadata_cannot_smuggle_a_source_or_fabricate_an_owner() {
        let mut bad = note("help", "body");
        bad["source"] = json!({"path":"/private/root/help.md","digest":"a".repeat(64)});
        assert!(validate_plan(&json!({"notes":[bad]})).is_err());
        let mut orphan = note("help", "body");
        orphan["scope_id"] = json!("scope:unknown#1");
        orphan["id"] = json!("note:scope:unknown#1:help");
        assert!(validate_plan(&json!({"notes":[orphan]})).is_err());
        let mut warnings = Vec::new();
        let mut captured = Captured::default();
        let mut wrong = note("help", "body");
        wrong["id"] = json!("draw#1");
        captured.capture(Some(&json!([wrong])), &mut warnings);
        assert!(captured.project(None).is_none());
        assert_eq!(warnings.len(), 1);
    }
}
