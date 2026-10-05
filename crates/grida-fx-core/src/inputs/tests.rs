//! Unit tests of the inputs compiler, defaults and the validator.
//!
//! Messages that quote values go through [`crate::text::py_repr`]; the tests in
//! [`with_text`] check those quotes and pass once the text module is in place.

use super::*;
use serde_json::json;

fn inputs(value: Value) -> IndexMap<String, Value> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn compiled(value: Value) -> Value {
    compile_inputs(&inputs(value), None).unwrap()
}

fn compile_error(value: Value) -> String {
    compile_inputs(&inputs(value), None).unwrap_err().message
}

#[test]
fn shorthand_compiles_like_gnode() {
    // The shapes stage-gen's `schema` verb printed (the `linear` and `tiered-price` cases,
    // optional members), with FX's file tag.
    assert_eq!(
        compiled(json!({"brief": {"type": "file", "kind": "text/plain"}})),
        json!({
            "$schema": DRAFT_2020_12,
            "type": "object",
            "properties": {"brief": {"type": "string", "x-fx-file": {"kind": "text/plain"}}},
            "additionalProperties": false,
            "required": ["brief"],
        })
    );
    assert_eq!(
        compiled(json!({})),
        json!({"$schema": DRAFT_2020_12, "type": "object", "properties": {},
               "additionalProperties": false})
    );
    let schema = compiled(json!({
        "ratio": {"type": "number", "optional": true},
        "shots": {"type": "files", "kind": "image", "optional": true},
        "pics": {"type": "files", "glob": true},
        "doc": {"type": "file"},
    }));
    assert_eq!(
        schema["properties"]["ratio"],
        json!({"default": null, "type": ["number", "null"]})
    );
    assert_eq!(
        schema["properties"]["shots"],
        json!({"default": null, "items": {"type": "string"}, "type": ["array", "null"],
               "x-fx-file": {"glob": false, "kind": "image", "many": true}})
    );
    assert_eq!(
        schema["properties"]["pics"]["x-fx-file"],
        json!({"kind": "file", "many": true, "glob": true})
    );
    assert_eq!(
        schema["properties"]["doc"]["x-fx-file"],
        json!({"kind": "file"})
    );
    assert_eq!(schema["required"], json!(["pics", "doc"]));
}

#[test]
fn keywords_are_renamed_in_declared_order() {
    let schema = compiled(json!({"n": {
        "minimum": 1, "type": "integer", "exclusive_maximum": 9, "multiple_of": 2,
        "description": "d", "examples": [2], "default": 4, "format": "x"
    }}));
    let n = &schema["properties"]["n"];
    let keys: Vec<&String> = n.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "minimum",
            "exclusiveMaximum",
            "multipleOf",
            "description",
            "examples",
            "default",
            "format",
            "type"
        ]
    );
    assert!(schema.get("required").is_none());
    let s = compiled(
        json!({"s": {"type": "string", "min_length": 1, "max_length": 3,
        "pattern": "^a", "enum": ["a"]}}),
    );
    assert_eq!(
        s["properties"]["s"],
        json!({"minLength": 1, "maxLength": 3, "pattern": "^a", "enum": ["a"], "type": "string"})
    );
    let l = compiled(json!({"l": {"type": "list", "min_items": 1, "max_items": 2,
        "unique_items": true, "items": {"type": "integer"}}}));
    assert_eq!(
        l["properties"]["l"],
        json!({"minItems": 1, "maxItems": 2, "uniqueItems": true, "type": "array",
               "items": {"type": "integer"}})
    );
}

#[test]
fn lists_maps_and_nested_objects() {
    let schema = compiled(json!({
        "style": {"tone": {"type": "string"}, "size": {"type": "integer", "default": 2}},
        "opts": {"a": {"type": "boolean", "optional": true}},
        "names": {"type": "list", "items": {"type": "string"}},
        "byname": {"type": "map", "values": {"w": {"type": "integer", "default": 1}}},
        "empty": {},
    }));
    assert_eq!(
        schema["properties"]["style"],
        json!({"type": "object", "properties": {"tone": {"type": "string"},
               "size": {"default": 2, "type": "integer"}},
               "additionalProperties": false, "required": ["tone"]})
    );
    assert_eq!(
        schema["properties"]["opts"],
        json!({"type": "object", "properties": {"a": {"default": null, "type": ["boolean", "null"]}},
               "additionalProperties": false})
    );
    assert_eq!(
        schema["properties"]["byname"],
        json!({"type": "object", "additionalProperties": {"type": "object",
               "properties": {"w": {"default": 1, "type": "integer"}},
               "additionalProperties": false}})
    );
    assert_eq!(
        schema["properties"]["empty"],
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    );
    // A group is required when any member is; lists and maps are fields.
    assert_eq!(schema["required"], json!(["style", "names", "byname"]));
    assert!(required(
        &json!({"a": {"type": "string"}, "b": {"type": "string", "optional": true}})
    ));
    assert!(!required(
        &json!({"b": {"type": "string", "optional": true}})
    ));
    assert!(!required(&json!({"$ref": "x#/y", "optional": true})));
    assert!(required(&json!({"$ref": "x#/y"})));
    assert!(required(&json!(5)));
}

#[test]
fn compile_errors() {
    assert_eq!(
        compile_error(json!({"a": 5})),
        "inputs.a: an input is a mapping"
    );
    assert_eq!(
        compile_error(json!({"a": {"type": "string", "zeta": 1, "alpha": 2}})),
        "inputs.a: unknown keyword alpha, zeta"
    );
    assert_eq!(
        compile_error(json!({"a": {"type": "list"}})),
        "inputs.a: a list declares its items"
    );
    assert_eq!(
        compile_error(json!({"a": {"type": "map", "values": "x"}})),
        "inputs.a: a map declares its values"
    );
    assert_eq!(
        compile_error(json!({"g": {"b": {"type": "list", "items": {"type": "map"}}}})),
        "inputs.g.b[]: a map declares its values"
    );
    assert_eq!(
        compile_error(json!({"a": {"$ref": "lib.yaml#/inputs/a"}})),
        "inputs.a: $ref needs the referenced workflow"
    );
    let error = compile_inputs(&inputs(json!({"a": {"type": "string"}})), None);
    assert!(error.is_ok());
    let error = compile_inputs(&inputs(json!({"a": {"type": "x"}})), None).unwrap_err();
    assert_eq!(error.kind, crate::ErrorKind::Input);
}

#[test]
fn refs_are_resolved_and_replace_the_reference() {
    let mut calls = Vec::new();
    let mut resolver = |reference: &str| -> Result<Value> {
        calls.push(reference.to_string());
        match reference {
            "lib.yaml#/inputs/tone" => Ok(json!({"type": "string", "enum": ["calm", "wild"]})),
            "lib.yaml#/inputs/again" => Ok(json!({"$ref": "lib.yaml#/inputs/tone"})),
            "lib.yaml#/inputs/loop" => Ok(json!({"inner": {"$ref": "lib.yaml#/inputs/loop"}})),
            "lib.yaml#/inputs/bad" => Ok(json!({"type": "nope"})),
            _ => Err(Error::input(format!("$ref {reference} names nothing"))),
        }
    };
    let declared = inputs(json!({
        "tone": {"$ref": "lib.yaml#/inputs/tone", "optional": true},
        "again": {"$ref": "lib.yaml#/inputs/again"},
    }));
    let schema = compile_inputs(&declared, Some(&mut resolver)).unwrap();
    assert_eq!(
        schema["properties"]["tone"],
        json!({"enum": ["calm", "wild"], "type": "string"})
    );
    assert_eq!(schema["properties"]["again"], schema["properties"]["tone"]);
    // required() judges the referencing mapping.
    assert_eq!(schema["required"], json!(["again"]));
    let looped = compile_inputs(
        &inputs(json!({"x": {"$ref": "lib.yaml#/inputs/loop"}})),
        Some(&mut resolver),
    )
    .unwrap_err();
    assert_eq!(
        looped.message,
        "inputs.x.inner: $ref lib.yaml#/inputs/loop refers back to itself"
    );
    let bad = compile_input(
        &json!({"$ref": "lib.yaml#/inputs/bad"}),
        "inputs.b",
        Some(&mut resolver),
    )
    .unwrap_err();
    assert!(
        bad.message.starts_with("inputs.b: unknown type "),
        "{}",
        bad.message
    );
}

#[test]
fn an_invalid_pattern_is_refused_when_compiled() {
    let message = compile_error(json!({"a": {"type": "string", "pattern": "(unclosed"}}));
    assert!(
        message.starts_with("inputs.a: pattern ")
            && message.ends_with(" is not a regular expression"),
        "{message}"
    );
}

#[test]
fn defaults_fill_groups_but_not_map_values() {
    let schema = compiled(json!({
        "count": {"type": "integer", "default": 3},
        "mood": {"type": "string", "optional": true},
        "style": {"tone": {"type": "string", "default": "calm"}, "size": {"type": "integer", "optional": true}},
        "deep": {"must": {"type": "string"}},
        "byname": {"type": "map", "values": {"w": {"type": "integer", "default": 1}}},
        "rows": {"type": "list", "items": {"w": {"type": "integer", "default": 1}}},
    }));
    let filled = with_defaults(
        &schema,
        json!({"rows": [{}, {"w": 5}], "byname": {"k": {}}, "count": 7}),
    );
    assert_eq!(
        filled,
        json!({
            "rows": [{"w": 1}, {"w": 5}],
            "byname": {"k": {}},
            "count": 7,
            "mood": null,
            "style": {"tone": "calm", "size": null},
        })
    );
    // Given names first in their order, then defaults in property order.
    let keys: Vec<&String> = filled.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["rows", "byname", "count", "mood", "style"]);
    // The same walk over runtime values.
    let val = with_defaults_val(&schema, bind::json_to_val(&json!({"count": 7})));
    let Val::Object(map) = val else { panic!() };
    assert_eq!(map["count"], Val::Number(7.0));
    assert_eq!(map["mood"], Val::Null);
    let Val::Object(style) = &map["style"] else {
        panic!()
    };
    assert_eq!(style["tone"], Val::Str("calm".into()));
}

#[test]
fn validation_without_quoted_values() {
    let schema = compiled(json!({
        "count": {"type": "integer"},
        "deep": {"must": {"type": "string"}},
        "n": {"type": "number", "minimum": 0},
    }));
    assert!(
        validate(
            &schema,
            &json!({"count": 2.0, "deep": {"must": ""}, "n": 0})
        )
        .is_ok()
    );
    // Errors are sorted by location; the root comes first; required names the containing object.
    let message = validate(&schema, &json!({"deep": {}, "n": 1})).unwrap_err();
    assert!(message.starts_with("inputs: "), "{message}");
    assert!(message.contains("; inputs.deep: "), "{message}");
    // At most eight are kept.
    let many = compiled(json!({"l": {"type": "list", "items": {"type": "string"}}}));
    let value = json!({"l": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]});
    let message = validate(&many, &value).unwrap_err();
    assert_eq!(message.matches("; ").count(), 7, "{message}");
    assert!(message.starts_with("inputs.l[0]: "), "{message}");
    assert!(message.contains("inputs.l[7]: "), "{message}");
    assert!(!message.contains("inputs.l[8]: "), "{message}");
    // Indices sort as numbers, keys by code point.
    let message = validate(
        &many,
        &json!({"l": [1, "a", "b", "c", "d", "e", "f", "g", "h", "i", 2]}),
    )
    .unwrap_err();
    assert!(
        message.starts_with("inputs.l[0]: ") && message.contains("; inputs.l[10]: "),
        "{message}"
    );
}

#[test]
fn booleans_are_not_numbers_and_whole_numbers_are_integers() {
    let schema = compiled(json!({"i": {"type": "integer"}, "x": {"type": "number"}}));
    assert!(validate(&schema, &json!({"i": 2.0, "x": 2})).is_ok());
    assert!(validate(&schema, &json!({"i": 2.5, "x": 1})).is_err());
    assert!(validate(&schema, &json!({"i": true, "x": 1})).is_err());
    assert!(validate(&schema, &json!({"i": 1, "x": false})).is_err());
}

#[test]
fn flag_names() {
    assert_eq!(flag_name("max_entities"), "--max-entities");
    assert_eq!(flag_name("n"), "--n");
}

#[test]
fn default_markers_are_found_at_every_depth() {
    let marker = json!({"missing": true});
    let check =
        |declared: Value| check_default_markers(&declared, "inputs.a").map_err(|r| r.message);
    assert!(
        check(json!({"type": "string", "default": marker.clone()}))
            .unwrap_err()
            .starts_with("inputs.a.default: ")
    );
    assert!(
        check(json!({"b": {"type": "string", "default": marker.clone()}}))
            .unwrap_err()
            .starts_with("inputs.a.b.default: ")
    );
    assert!(
        check(json!({"type": "list", "items": {"type": "string", "default": [marker.clone()]}}))
            .unwrap_err()
            .starts_with("inputs.a[].default[0]: ")
    );
    assert!(
        check(json!({"type": "map", "values": {"type": "string", "default": marker.clone()}}))
            .unwrap_err()
            .starts_with("inputs.a{}.default: ")
    );
    assert!(check(json!({"$ref": "x#/y", "default": marker.clone()})).is_err());
    assert!(check(json!({"type": "string", "default": {"missing": false}})).is_ok());
    assert!(check(json!({"missing": {"type": "string"}})).is_ok());
}

/// Tests of message texts that quote values (they need [`crate::text::py_repr`]).
mod with_text {
    use super::*;

    #[test]
    fn the_example_stage_gen_printed() {
        let schema = compiled(json!({
            "count": {"type": "integer"},
            "style": {"tone": {"type": "string"}},
            "table": {"type": "map", "values": {"type": "integer"}},
            "tags": {"type": "list", "items": {"type": "string"}},
        }));
        let value = json!({
            "count": true,
            "style": {"tone": 5, "extra": 1},
            "table": {"a": "x"},
            "tags": [1, "ok", 2],
        });
        assert_eq!(
            validate(&schema, &value).unwrap_err(),
            "inputs.count: True is not of type 'integer'; \
             inputs.style: Additional properties are not allowed ('extra' was unexpected); \
             inputs.style.tone: 5 is not of type 'string'; \
             inputs.table.a: 'x' is not of type 'integer'; \
             inputs.tags[0]: 1 is not of type 'string'; \
             inputs.tags[2]: 2 is not of type 'string'"
        );
    }

    #[test]
    fn every_keyword_message() {
        let check = |declared: Value, value: Value| {
            let schema = compiled(json!({"v": declared}));
            validate(&schema, &json!({"v": value})).unwrap_err()
        };
        assert_eq!(
            check(json!({"type": "string", "optional": true}), json!(1)),
            "inputs.v: 1 is not of type 'string', 'null'"
        );
        assert_eq!(
            check(json!({"type": "string", "enum": ["a", "b"]}), json!("c")),
            "inputs.v: 'c' is not one of ['a', 'b']"
        );
        assert_eq!(
            check(json!({"type": "integer", "minimum": 2}), json!(1)),
            "inputs.v: 1 is less than the minimum of 2"
        );
        assert_eq!(
            check(json!({"type": "integer", "maximum": 2}), json!(3)),
            "inputs.v: 3 is greater than the maximum of 2"
        );
        assert_eq!(
            check(json!({"type": "integer", "exclusive_minimum": 2}), json!(2)),
            "inputs.v: 2 is less than or equal to the minimum of 2"
        );
        assert_eq!(
            check(json!({"type": "integer", "exclusive_maximum": 2}), json!(2)),
            "inputs.v: 2 is greater than or equal to the maximum of 2"
        );
        assert_eq!(
            check(json!({"type": "integer", "multiple_of": 3}), json!(4)),
            "inputs.v: 4 is not a multiple of 3"
        );
        assert_eq!(
            check(json!({"type": "string", "min_length": 3}), json!("ab")),
            "inputs.v: 'ab' is too short"
        );
        assert_eq!(
            check(json!({"type": "string", "max_length": 1}), json!("ab")),
            "inputs.v: 'ab' is too long"
        );
        assert_eq!(
            check(json!({"type": "string", "min_length": 1}), json!("")),
            "inputs.v: '' should be non-empty"
        );
        assert_eq!(
            check(json!({"type": "string", "max_length": 0}), json!("a")),
            "inputs.v: 'a' is expected to be empty"
        );
        assert_eq!(
            check(json!({"type": "string", "pattern": "^a$"}), json!("b")),
            "inputs.v: 'b' does not match '^a$'"
        );
        assert_eq!(
            check(
                json!({"type": "list", "min_items": 2, "items": {"type": "integer"}}),
                json!([1])
            ),
            "inputs.v: [1] is too short"
        );
        assert_eq!(
            check(
                json!({"type": "list", "max_items": 1, "items": {"type": "integer"}}),
                json!([1, 2])
            ),
            "inputs.v: [1, 2] is too long"
        );
        assert_eq!(
            check(
                json!({"type": "list", "unique_items": true, "items": {"type": "number"}}),
                json!([1, 1.0])
            ),
            "inputs.v: [1, 1] has non-unique elements"
        );
        assert_eq!(
            check(
                json!({"a": {"type": "string"}, "b": {"type": "string"}}),
                json!({"c": 1, "d": 2})
            ),
            "inputs.v: Additional properties are not allowed ('c', 'd' were unexpected); \
             inputs.v: 'a' is a required property; inputs.v: 'b' is a required property"
        );
        // A type failure does not stop the other keywords (enum before type: declared order).
        assert_eq!(
            check(json!({"type": "string", "enum": ["a"]}), json!(5)),
            "inputs.v: 5 is not one of ['a']; inputs.v: 5 is not of type 'string'"
        );
        // Code points, not bytes.
        let schema = compiled(json!({"v": {"type": "string", "max_length": 2}}));
        assert!(validate(&schema, &json!({"v": "éé"})).is_ok());
    }

    #[test]
    fn an_optional_enum_left_out_is_refused_as_gnode_does() {
        let schema = compiled(
            json!({"mood": {"type": "string", "enum": ["calm", "wild"], "optional": true}}),
        );
        let value = with_defaults(&schema, json!({}));
        assert_eq!(
            validate(&schema, &value).unwrap_err(),
            "inputs.mood: None is not one of ['calm', 'wild']"
        );
    }

    #[test]
    fn root_messages() {
        let schema = compiled(json!({"brief": {"type": "string"}}));
        assert_eq!(
            validate(&schema, &json!({})).unwrap_err(),
            "inputs: 'brief' is a required property"
        );
        assert_eq!(
            validate(&schema, &json!({"brief": "x", "zz": 1})).unwrap_err(),
            "inputs: Additional properties are not allowed ('zz' was unexpected)"
        );
    }

    #[test]
    fn a_missing_value_prints_as_gnode_did() {
        let schema = compiled(json!({"name": {"type": "string"}}));
        let mut given = IndexMap::new();
        given.insert("name".to_string(), Val::Missing);
        assert_eq!(
            validate_val(&schema, &Val::Object(given)).unwrap_err(),
            "inputs.name: MISSING is not of type 'string'"
        );
    }

    #[test]
    fn unknown_types_quote_the_type() {
        assert_eq!(
            compile_error(json!({"a": {"type": "x"}})),
            "inputs.a: unknown type 'x'"
        );
        assert_eq!(
            compile_error(json!({"a": {"type": 5}})),
            "inputs.a: unknown type 5"
        );
        assert_eq!(
            compile_error(json!({"a": {"type": null}})),
            "inputs.a: unknown type None"
        );
    }
}
