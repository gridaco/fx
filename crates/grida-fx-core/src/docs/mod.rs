//! FX documents: reading, schema validation and typed forms (spec/schemas, spec/yaml.md).
//!
//! Every authored document is read with the strict YAML loader ([`crate::yaml`]), validated
//! against its JSON Schema with the `jsonschema` crate (schemas embedded below), and then
//! deserialized into typed structs. Refusals are [`crate::ErrorKind::Document`] errors (exit 2)
//! reading `<file>: <loc>: <message>` with several joined by `"; "` (`<loc>` the instance path
//! joined by `.`, or `(document)` at the top). A document of the wrong kind reads
//! `<file>: a workflow file starts with fx: workflow/v1` (likewise `project`, `routes`, `lock`).
//! For workflow steps, the step-shape sentences of stage-gen's engine (listed in [`workflow`])
//! are produced first, in its order, so authors see the same sentences.
//!
//! The reserved-marker rule (identity.md §3) applies to values read from a workflow (`with`,
//! `tables`, `let`, `outputs`, `for_each`, `matrix`, defaults) and from inputs files.

pub mod lock;
pub mod project;
pub mod takes;
pub mod workflow;

use crate::error::{Error, Result};
use jsonschema::error::ValidationErrorKind;
use jsonschema::paths::LocationSegment;
use jsonschema::{ValidationError, Validator};
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;

/// The documents FX defines a schema for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Schema {
    Workflow,
    Project,
    Routes,
    Lock,
    Takes,
    Graph,
    NodeProtocol,
}

impl Schema {
    /// The schema's JSON text, embedded from spec/schemas.
    pub fn text(self) -> &'static str {
        match self {
            Schema::Workflow => include_str!("../../../../spec/schemas/fx-workflow-v1.schema.json"),
            Schema::Project => include_str!("../../../../spec/schemas/fx-project-v1.schema.json"),
            Schema::Routes => include_str!("../../../../spec/schemas/fx-routes-v1.schema.json"),
            Schema::Lock => include_str!("../../../../spec/schemas/fx-lock-v1.schema.json"),
            Schema::Takes => include_str!("../../../../spec/schemas/fx-takes-v1.schema.json"),
            Schema::Graph => include_str!("../../../../spec/schemas/fx-graph-v1.schema.json"),
            Schema::NodeProtocol => {
                include_str!("../../../../spec/schemas/fx-node-protocol-v1.schema.json")
            }
        }
    }

    /// The `fx:` value an authored document of this kind starts with (`workflow/v1`, …), for the
    /// documents that have one.
    pub fn discriminator(self) -> Option<&'static str> {
        match self {
            Schema::Workflow => Some("workflow/v1"),
            Schema::Project => Some("project/v1"),
            Schema::Routes => Some("routes/v1"),
            Schema::Lock => Some("lock/v1"),
            _ => None,
        }
    }

    /// The word for this kind of document in `a <word> file starts with fx: …`.
    fn word(self) -> &'static str {
        match self {
            Schema::Workflow => "workflow",
            Schema::Project => "project",
            Schema::Routes => "routes",
            Schema::Lock => "lock",
            Schema::Takes => "takes",
            Schema::Graph => "graph",
            Schema::NodeProtocol => "node protocol",
        }
    }

    /// The cell holding this schema's compiled validator.
    fn cell(self) -> &'static OnceLock<std::result::Result<Validator, String>> {
        static WORKFLOW: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        static PROJECT: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        static ROUTES: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        static LOCK: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        static TAKES: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        static GRAPH: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        static NODE_PROTOCOL: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
        match self {
            Schema::Workflow => &WORKFLOW,
            Schema::Project => &PROJECT,
            Schema::Routes => &ROUTES,
            Schema::Lock => &LOCK,
            Schema::Takes => &TAKES,
            Schema::Graph => &GRAPH,
            Schema::NodeProtocol => &NODE_PROTOCOL,
        }
    }

    /// The compiled validator, built on first use. The schemas are embedded and hold no remote
    /// `$ref`, so a failure here is a fault in the engine itself.
    fn validator(self) -> Result<&'static Validator> {
        self.cell()
            .get_or_init(|| {
                let schema: Value = serde_json::from_str(self.text())
                    .map_err(|e| format!("the embedded {:?} schema is not JSON: {e}", self))?;
                jsonschema::validator_for(&schema)
                    .map_err(|e| format!("the embedded {:?} schema does not compile: {e}", self))
            })
            .as_ref()
            .map_err(|message| Error::internal(message.clone()))
    }
}

/// An instance location as messages print it: the path's segments joined by `.`, or
/// `(document)` at the top.
fn location<'a>(segments: impl Iterator<Item = LocationSegment<'a>>) -> String {
    let parts: Vec<String> = segments.map(|segment| segment.to_string()).collect();
    if parts.is_empty() {
        "(document)".into()
    } else {
        parts.join(".")
    }
}

/// The lines that explain one schema error. An `anyOf`/`oneOf` that no branch matched says only
/// that; when exactly one branch is plausible it is explained instead: branches refused for the
/// value's type right where the choice is are dropped, and among the rest the one whose errors
/// all lie deeper wins (or the only one left).
fn explain(error: &ValidationError<'_>, out: &mut Vec<String>) {
    let branches = match error.kind() {
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            context
        }
        _ => {
            out.push(format!(
                "{}: {error}",
                location(error.instance_path().segments())
            ));
            return;
        }
    };
    let here = error.instance_path().as_str();
    let plausible: Vec<&Vec<ValidationError<'static>>> = branches
        .iter()
        .filter(|branch| {
            !branch.iter().any(|e| {
                e.instance_path().as_str() == here
                    && matches!(e.kind(), ValidationErrorKind::Type { .. })
            })
        })
        .collect();
    let deeper: Vec<&&Vec<ValidationError<'static>>> = plausible
        .iter()
        .filter(|branch| {
            !branch.is_empty()
                && branch
                    .iter()
                    .all(|e| e.instance_path().as_str().len() > here.len())
        })
        .collect();
    let chosen = match (deeper.as_slice(), plausible.as_slice()) {
        ([one], _) => Some(**one),
        (_, [one]) if !one.is_empty() => Some(*one),
        _ => None,
    };
    match chosen {
        Some(branch) => branch.iter().for_each(|e| explain(e, out)),
        None => out.push(format!(
            "{}: {error}",
            location(error.instance_path().segments())
        )),
    }
}

/// Validates a value against a schema (compiled once per schema and cached). `file` labels the
/// messages.
pub fn validate(schema: Schema, value: &Value, file: &str) -> Result<()> {
    let validator = schema.validator()?;
    let mut problems: Vec<String> = Vec::new();
    for error in validator.iter_errors(value) {
        let mut lines = Vec::new();
        explain(&error, &mut lines);
        for line in lines {
            if !problems.contains(&line) {
                problems.push(line);
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::document(format!("{file}: {}", problems.join("; "))))
    }
}

/// Refuses a document that is not a mapping, or whose `fx:` is not this kind's discriminator
/// (for the kinds that have one).
pub(crate) fn check_kind(schema: Schema, value: &Value, file: &str) -> Result<()> {
    let Some(expected) = schema.discriminator() else {
        return Ok(());
    };
    let found = value
        .as_object()
        .and_then(|map| map.get("fx"))
        .and_then(Value::as_str);
    if found == Some(expected) {
        Ok(())
    } else {
        Err(Error::document(format!(
            "{file}: a {} file starts with fx: {expected}",
            schema.word()
        )))
    }
}

/// Reads a YAML document (strict subset) and checks its `fx:` discriminator, then validates it.
/// Returns the value as authored.
pub fn read_document(path: &Path, label: &str, schema: Schema) -> Result<Value> {
    let value = crate::yaml::load_file(path, label)?;
    check_kind(schema, &value, label)?;
    validate(schema, &value, label)?;
    Ok(value)
}

/// Python's truthiness of a JSON value, as gnode's checks read optional fields: `null`, `false`,
/// `0`, `""`, `[]` and `{}` are false.
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => crate::value::as_f64(n) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_embedded_schema_compiles() {
        for schema in [
            Schema::Workflow,
            Schema::Project,
            Schema::Routes,
            Schema::Lock,
            Schema::Takes,
            Schema::Graph,
            Schema::NodeProtocol,
        ] {
            schema
                .validator()
                .unwrap_or_else(|e| panic!("{schema:?}: {e}"));
        }
    }

    #[test]
    fn messages_name_the_file_and_the_location() {
        let lock = json!({"fx": "lock/v1", "nodes": {"nodes/n.py#a@1": "xyz"}});
        let error = validate(Schema::Lock, &lock, "fx.lock").unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Document);
        assert!(
            error.message.starts_with("fx.lock: nodes.nodes/n.py#a@1: "),
            "{}",
            error.message
        );
        let error = validate(Schema::Lock, &json!({"fx": "lock/v1"}), "fx.lock").unwrap_err();
        assert!(
            error.message.starts_with("fx.lock: (document): "),
            "{}",
            error.message
        );
        assert!(validate(Schema::Lock, &json!({"fx": "lock/v1", "nodes": {}}), "x").is_ok());
    }

    #[test]
    fn several_problems_are_joined() {
        let project = json!({"fx": "project/v1", "runs": "", "extra": 1});
        let error = validate(Schema::Project, &project, "fx.yaml").unwrap_err();
        assert!(error.message.contains("; "), "{}", error.message);
        assert!(error.message.contains("runs: "), "{}", error.message);
    }

    #[test]
    fn a_choice_with_one_plausible_branch_explains_that_branch() {
        let message = |document: Value| {
            validate(Schema::Workflow, &document, "w.yaml")
                .unwrap_err()
                .message
        };
        let step = |extra: Value| {
            let mut step = json!({"uses": "fx/x@1"});
            for (k, v) in extra.as_object().unwrap() {
                step[k] = v.clone();
            }
            json!({"fx": "workflow/v1", "id": "w", "title": "W", "steps": {"s": step}})
        };
        // The null branch is refused for the value's type; the other one is explained.
        let mut budget = step(json!({}));
        budget["budget"] = json!({"max_usd": -1});
        assert_eq!(
            message(budget),
            "w.yaml: budget.max_usd: -1 is less than the minimum of 0"
        );
        assert!(
            message(step(json!({"route": "nope"}))).starts_with("w.yaml: steps.s.route: \"nope\" "),
            "{}",
            message(step(json!({"route": "nope"})))
        );
        assert!(
            message(step(json!({"on_reject": "maybe", "judges": "a"})))
                .starts_with("w.yaml: steps.s.on_reject: \"maybe\" is not one of "),
        );
        // Of several plausible branches, the one whose errors all lie deeper.
        let mut inputs = step(json!({}));
        inputs["inputs"] = json!({"a": {"type": "strin"}});
        assert!(
            message(inputs.clone()).starts_with("w.yaml: inputs.a.type: \"strin\" is not one of "),
            "{}",
            message(inputs)
        );
        // Otherwise the choice itself is named.
        let mut glob = step(json!({}));
        glob["inputs"] = json!({"a": {"type": "string", "glob": true}});
        assert!(message(glob).starts_with("w.yaml: inputs.a: "));
    }

    #[test]
    fn the_discriminator_names_the_kind() {
        let message = |schema, value: Value| check_kind(schema, &value, "f.yaml").unwrap_err();
        assert_eq!(
            message(Schema::Workflow, json!([])).message,
            "f.yaml: a workflow file starts with fx: workflow/v1"
        );
        assert_eq!(
            message(Schema::Project, json!({"fx": "workflow/v1"})).message,
            "f.yaml: a project file starts with fx: project/v1"
        );
        assert_eq!(
            message(Schema::Routes, json!({})).message,
            "f.yaml: a routes file starts with fx: routes/v1"
        );
        assert_eq!(
            message(Schema::Lock, json!({"fx": 1})).message,
            "f.yaml: a lock file starts with fx: lock/v1"
        );
        assert!(check_kind(Schema::Takes, &json!([]), "t").is_ok());
    }
}
