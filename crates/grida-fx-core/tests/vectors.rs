//! The value layer against the spec's vectors.

use grida_fx_core::value::{canon, check_markers, digest, file_digest, parse_json, write_json};
use serde_json::Value;
use std::path::PathBuf;

fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/vectors")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn cases<'a>(doc: &'a Value, field: &str) -> &'a Vec<Value> {
    doc[field].as_array().unwrap()
}

#[test]
fn jcs_vectors() {
    let doc = vectors("jcs/cases.json");
    for case in cases(&doc, "cases") {
        let name = case["name"].as_str().unwrap();
        let input = case["input"].as_str().unwrap();
        match (case.get("canonical"), case.get("refuse")) {
            (Some(want), _) => {
                let value = parse_json(input).unwrap_or_else(|e| panic!("{name}: refused: {e}"));
                assert_eq!(canon(&value), want.as_str().unwrap(), "{name}");
            }
            // Any implementation must refuse; this one also names the vector's rule, and never
            // panics on the way (a refusal inside a multibyte character once did).
            (None, Some(rule)) => match parse_json(input) {
                Ok(value) => panic!("{name}: should be refused, read {}", canon(&value)),
                Err(refused) => assert_eq!(refused.code, rule.as_str().unwrap(), "{name}"),
            },
            _ => panic!("{name}: neither canonical nor refuse"),
        }
    }
}

#[test]
fn json_output_vectors() {
    let doc = vectors("identity/json_output.json");
    for case in cases(&doc, "cases") {
        let name = case["name"].as_str().unwrap();
        let value = parse_json(case["value_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            write_json(&value),
            case["bytes_utf8"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn marker_vectors() {
    let doc = vectors("identity/markers.json");
    for case in cases(&doc, "cases") {
        let name = case["name"].as_str().unwrap();
        let value = parse_json(case["json"].as_str().unwrap()).unwrap();
        let refused = check_markers(&value, "value").is_err();
        assert_eq!(refused, case["refuse"].as_bool().unwrap(), "{name}");
    }
}

#[test]
fn identity_examples() {
    let doc = vectors("identity/examples.json");
    for example in cases(&doc, "examples") {
        let name = example["name"].as_str().unwrap();
        if let Some(text) = example.get("file_utf8") {
            assert_eq!(
                file_digest(text.as_str().unwrap().as_bytes()),
                example["digest"].as_str().unwrap(),
                "{name}"
            );
            continue;
        }
        let object = match (example.get("object"), example.get("object_source")) {
            (Some(object), _) => parse_json(&serde_json::to_string(object).unwrap()).unwrap(),
            (None, Some(source)) => parse_json(source.as_str().unwrap()).unwrap(),
            _ => continue,
        };
        if let Some(want) = example.get("canonical") {
            assert_eq!(canon(&object), want.as_str().unwrap(), "{name}");
        }
        if let Some(want) = example.get("digest") {
            assert_eq!(digest(&object), want.as_str().unwrap(), "{name}");
        }
    }
}
