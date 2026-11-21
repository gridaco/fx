//! Offline checks for the inputs of a workflow-authored waiting step.

use crate::val::Val;
use indexmap::IndexMap;
use serde_json::{Value, json};

/// The same offline draft and reference resolver are used to plan a gate and accept its answer.
pub fn answer_validator(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::draft202012::options()
        .with_retriever(LocalReferences)
        .build(schema)
        .map_err(|_| {
            "schema must be a valid draft 2020-12 JSON Schema with only internal references".into()
        })
}

/// Check the intersection of the declared answer schema with the known candidate keys.
/// A top-level key is a complete answer. For a named field, other properties can still
/// be supplied by the responder, so only reject constraints that adding fields cannot fix.
pub fn validate_choice_keys<'a>(
    schema: &Value,
    validator: &jsonschema::Validator,
    mut keys: impl Iterator<Item = &'a str>,
    choice_field: Option<&str>,
) -> Result<(), &'static str> {
    let Some(field) = choice_field else {
        return keys
            .any(|key| validator.is_valid(&Value::String(key.into())))
            .then_some(())
            .ok_or("no choice key satisfies the answer schema");
    };
    let possible = keys.any(|key| {
        let partial = json!({(field): key});
        !validator.iter_errors(&partial).any(|error| {
            // Only keywords reached directly from the root impose unconditional constraints
            // on this particular field. A conditional may choose a different branch once
            // a sibling exists. Use the evaluation path to preserve that distinction for refs.
            let keyword = error.evaluation_path().as_str().split('/').nth(1);
            matches!(
                keyword,
                Some(
                    "type"
                        | "properties"
                        | "patternProperties"
                        | "additionalProperties"
                        | "propertyNames"
                        | "maxProperties"
                )
            )
        })
    });
    if schema != &Value::Bool(false) && possible {
        Ok(())
    } else {
        Err("no choice key can be supplied in choice_field under the answer schema")
    }
}

struct LocalReferences;

impl jsonschema::Retrieve for LocalReferences {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("waiting-step schemas cannot retrieve external resources".into())
    }
}

/// Each known setting is checked as soon as it has a value, including settings produced at run time.
pub fn validate(values: &IndexMap<String, Val>) -> Result<(), String> {
    let known = |name: &str| {
        values
            .get(name)
            .filter(|v| !v.contains_pending() && !v.contains_failed())
    };
    if let Some(message) = known("message")
        && !matches!(message, Val::Str(s) if !s.trim().is_empty())
    {
        return Err("message must be nonempty text".into());
    }
    // A literal map can have known keys even if its artifact values are still pending.
    let choice_keys: Option<Vec<&str>> = match values.get("choices") {
        Some(Val::Object(members)) => Some(members.keys().map(String::as_str).collect()),
        Some(Val::Collection(collection)) => Some(
            collection
                .items
                .iter()
                .map(|(key, _)| key.as_str())
                .collect(),
        ),
        Some(choices) if !choices.contains_pending() && !choices.contains_failed() => {
            return Err("choices must be a nonempty keyed object or collection".into());
        }
        _ => None,
    };
    if choice_keys
        .as_ref()
        .is_some_and(|keys| keys.is_empty() || keys.iter().any(|key| key.trim().is_empty()))
    {
        return Err("choices must be a nonempty keyed object or collection".into());
    }
    if let Some(field) = known("choice_field") {
        if !matches!(field, Val::Str(s) if !s.trim().is_empty()) {
            return Err("choice_field must name a nonempty answer property".into());
        }
        if !values.contains_key("choices") {
            return Err("choice_field requires choices".into());
        }
    }
    if let Some(schema) = known("schema") {
        let schema = schema
            .plain()
            .filter(|s| s.is_object() || s.is_boolean())
            .ok_or("schema must be a JSON Schema")?;
        let validator = answer_validator(&schema)?;
        if let Some(keys) = choice_keys {
            match values.get("choice_field") {
                None => validate_choice_keys(&schema, &validator, keys.into_iter(), None)?,
                Some(Val::Str(field)) => {
                    validate_choice_keys(&schema, &validator, keys.into_iter(), Some(field))?
                }
                _ => {} // A dynamic choice_field will be checked once evaluated.
            }
        }
    }
    if let Some(timeout) = known("timeout_seconds")
        && !matches!(timeout, Val::Number(n) if n.fract() == 0.0 && *n >= 1.0 && *n <= 9_007_199_254_740_991.0)
    {
        return Err("timeout_seconds must be a positive integer".into());
    }
    if let Some(policy) = known("on_timeout")
        && !matches!(policy, Val::Str(s) if s == "fail" || s == "continue")
    {
        return Err("on_timeout must be fail or continue".into());
    }
    Ok(())
}
