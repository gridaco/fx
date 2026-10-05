//! Takes files, `<workflow id>.takes.yaml` (spec/schemas/fx-takes-v1.schema.json; step paths as
//! spec/identity.md §11 writes them).
//!
//! Location: in the folder of the workflow file, or, for a builder, of its `takes_anchor`
//! (spec/protocol.md §5.2); a workflow whose home project is not the planning project (a used
//! published workflow) keeps it in the planning project's root. Read in the strict subset and
//! validated: keys are step paths, `take` an integer from 1 to [`MAX_TAKE`] (a larger one is
//! refused with `<step>.take: <n> is above 1000, …`), `result` a bare digest. An absent file
//! is empty. The writer (reroll/pick, step 3) writes
//! `# <name>: written by \`grida-fx reroll\` and \`grida-fx pick\`; commit it\n` + sorted YAML.

use super::{Schema, validate};
use crate::error::{Error, Result};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The largest take a takes file may name (fx-takes-v1 `maximum`). A step makes every take up to
/// the one it uses, so the bound keeps a hand-edited number from planning without end.
pub const MAX_TAKE: u32 = 1000;

/// One entry: the take a step path uses, and the picked take's first output digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeChoice {
    pub take: u32,
    pub result: Option<String>,
}

/// Entries by step path, in file order.
pub type Takes = IndexMap<String, TakeChoice>;

/// `<folder>/<workflow id>.takes.yaml`.
pub fn takes_path(folder: &Path, workflow_id: &str) -> PathBuf {
    folder.join(format!("{workflow_id}.takes.yaml"))
}

/// Reads a takes file; absent is empty. `label` names it in errors.
pub fn read_takes(path: &Path, label: &str) -> Result<Takes> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Takes::new()),
        Err(error) => return Err(Error::io(label, &error)),
        Ok(_) => {}
    }
    let document = crate::yaml::load_file(path, label)?;
    parse_takes(&document, label)
}

/// Validates and types a takes document (fx-takes-v1).
pub(crate) fn parse_takes(document: &Value, label: &str) -> Result<Takes> {
    let Some(entries) = document.as_object() else {
        return Err(Error::document(format!(
            "{label}: a takes file maps step paths to takes"
        )));
    };
    // Before the schema, whose `maximum` would say the same less plainly.
    for (step, entry) in entries {
        let take = entry
            .get("take")
            .and_then(Value::as_number)
            .map(crate::value::as_f64);
        if let Some(take) = take
            && take > f64::from(MAX_TAKE)
        {
            return Err(Error::document(format!(
                "{label}: {step}.take: {} is above {MAX_TAKE}, the most takes a step may have",
                crate::value::format_number(take)
            )));
        }
    }
    validate(Schema::Takes, document, label)?;
    let mut takes = Takes::with_capacity(entries.len());
    for (step, entry) in entries {
        let take = entry
            .get("take")
            .and_then(Value::as_number)
            .map(crate::value::as_f64)
            .filter(|x| x.fract() == 0.0 && (1.0..=f64::from(MAX_TAKE)).contains(x))
            .ok_or_else(|| {
                Error::document(format!(
                    "{label}: {step}.take: not a take number FX can hold"
                ))
            })?;
        let result = entry
            .get("result")
            .and_then(Value::as_str)
            .map(String::from);
        takes.insert(
            step.clone(),
            TakeChoice {
                take: take as u32,
                result,
            },
        );
    }
    Ok(takes)
}

/// The text of a takes file.
pub fn render_takes(name: &str, takes: &Takes) -> String {
    let header = format!("# {name}: written by `grida-fx reroll` and `grida-fx pick`; commit it\n");
    if takes.is_empty() {
        return header;
    }
    let mut steps: Vec<(&String, &TakeChoice)> = takes.iter().collect();
    steps.sort_by(|a, b| a.0.cmp(b.0));
    let mut document = Map::new();
    for (step, choice) in steps {
        let mut entry = Map::new();
        if let Some(result) = &choice.result {
            entry.insert("result".into(), Value::String(result.clone()));
        }
        entry.insert("take".into(), Value::from(choice.take));
        document.insert(step.clone(), Value::Object(entry));
    }
    format!(
        "{header}{}",
        crate::yaml::write_yaml(&Value::Object(document))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn takes_documents_are_typed_in_file_order() {
        let digest = "5637c1b4a68923781eb2e9b563f97e0bf346245040371b2a3c3c2365798ba24d";
        let takes = parse_takes(
            &json!({
                "draw": {"take": 3},
                "entity['it\\'s'].draw": {"take": 1, "result": digest},
                "count": {"take": 2.0},
            }),
            "case.takes.yaml",
        )
        .unwrap();
        assert_eq!(
            takes.keys().collect::<Vec<_>>(),
            ["draw", "entity['it\\'s'].draw", "count"]
        );
        assert_eq!(
            takes["draw"],
            TakeChoice {
                take: 3,
                result: None
            }
        );
        assert_eq!(
            takes["entity['it\\'s'].draw"].result.as_deref(),
            Some(digest)
        );
        assert_eq!(takes["count"].take, 2);
        assert!(parse_takes(&json!({}), "t").unwrap().is_empty());
    }

    #[test]
    fn takes_refusals() {
        let message = |document: Value| {
            parse_takes(&document, "case.takes.yaml")
                .unwrap_err()
                .message
        };
        assert_eq!(
            message(json!([1])),
            "case.takes.yaml: a takes file maps step paths to takes"
        );
        assert_eq!(
            message(json!("x")),
            "case.takes.yaml: a takes file maps step paths to takes"
        );
        for refused in [
            json!({"draw": {"take": 0}}),
            json!({"draw": {"take": true}}),
            json!({"draw": {"take": "2"}}),
            json!({"draw": 2}),
            json!({"draw": {"take": 1, "result": "sha256:5637"}}),
            json!({"draw": {"take": 1, "extra": 1}}),
            json!({"Draw": {"take": 1}}),
            json!({"entity[ada].draw": {"take": 1}}),
        ] {
            let text = message(refused.clone());
            assert!(text.starts_with("case.takes.yaml: "), "{refused}: {text}");
        }
    }

    #[test]
    fn a_take_above_the_bound_is_refused() {
        let at_bound = parse_takes(&json!({"draw": {"take": 1000}}), "case.takes.yaml").unwrap();
        assert_eq!(at_bound["draw"].take, MAX_TAKE);
        for (take, shown) in [
            (json!(1001), "1001"),
            (json!(100000), "100000"),
            (json!(4000000000u64), "4000000000"),
            (json!(1e300), "1e+300"),
        ] {
            let error = parse_takes(&json!({"g['a'].draw": {"take": take}}), "case.takes.yaml")
                .unwrap_err();
            assert_eq!(
                error.message,
                format!(
                    "case.takes.yaml: g['a'].draw.take: {shown} is above 1000, the most takes a \
                     step may have"
                )
            );
        }
    }

    #[test]
    fn the_schema_bounds_a_take() {
        let error = validate(Schema::Takes, &json!({"draw": {"take": 1001}}), "t").unwrap_err();
        assert!(error.message.starts_with("t: "), "{}", error.message);
        assert!(validate(Schema::Takes, &json!({"draw": {"take": 1000}}), "t").is_ok());
    }

    #[test]
    fn an_absent_takes_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = takes_path(dir.path(), "case");
        assert!(path.ends_with("case.takes.yaml"));
        assert!(read_takes(&path, "case.takes.yaml").unwrap().is_empty());
    }

    #[test]
    fn an_empty_takes_file_is_a_header() {
        assert_eq!(
            render_takes("case.takes.yaml", &Takes::new()),
            "# case.takes.yaml: written by `grida-fx reroll` and `grida-fx pick`; commit it\n"
        );
    }
}
