//! JSON Schemas as OpenRouter's strict mode takes them (spec/providers.md §9.2, "Schemas").
//!
//! - [`inline_refs`]: when the root has a `$defs` object, every object whose `$ref` starts with
//!   `#/$defs/` is replaced by a deep copy of its target with the referencing object's other
//!   members laid over it, recursively; every `$defs` member is then dropped. An unknown target is
//!   refused (`unknown local schema reference: <ref>`), and so is a cycle (`cyclic local schema
//!   reference: <ref>`). `#/definitions/…` is left as is.
//! - [`strict`]: at schema positions only (not inside a `properties` map's names, not inside
//!   `enum`/`const` values), drop `default` and the assertions OpenRouter's strict mode refuses
//!   (`contains`, `format`, `maxContains`, `maxItems`, `maxLength`, `maxProperties`, `maximum`,
//!   `minContains`, `minItems`, `minLength`, `minProperties`, `minimum`, `multipleOf`, `pattern`,
//!   `patternProperties`, `propertyNames`, `unevaluatedItems`, `unevaluatedProperties`,
//!   `uniqueItems`); in every schema with an object `properties`, set `required` to every property
//!   name in order and `additionalProperties` to `false`. Member order is kept; `required` and
//!   `additionalProperties` are replaced in place or appended.
//!
//! Schema positions are the root and, below a schema: each value of `properties`, `$defs`,
//! `definitions` and `dependentSchemas`; `items`, `prefixItems`, `additionalItems`, `allOf`,
//! `anyOf` and `oneOf` (a schema or a list of schemas); `additionalProperties` (when an object),
//! `not`, `if`, `then`, `else` and `contains` (the last is dropped anyway).

use serde_json::{Map, Value};

/// The members strict canonicalization drops from every schema.
pub const DROPPED: [&str; 20] = [
    "default",
    "contains",
    "format",
    "maxContains",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minContains",
    "minItems",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "pattern",
    "patternProperties",
    "propertyNames",
    "unevaluatedItems",
    "unevaluatedProperties",
    "uniqueItems",
];

/// The most values [`inline_refs`] produces. Copies of copies can grow a small schema
/// exponentially (each definition naming the next twice); past this the schema is refused
/// rather than built.
pub const MAX_INLINED_VALUES: usize = 1_000_000;

const LOCAL_PREFIX: &str = "#/$defs/";

/// `$ref` inlining (module doc).
pub fn inline_refs(schema: &Value) -> Result<Value, String> {
    let Some(definitions) = schema.get("$defs").and_then(Value::as_object) else {
        return Ok(schema.clone());
    };
    let mut inliner = Inliner {
        definitions,
        expanding: Vec::new(),
        produced: 0,
    };
    inliner.expand(schema)
}

struct Inliner<'a> {
    definitions: &'a Map<String, Value>,
    /// The references whose targets are being expanded, outermost first.
    expanding: Vec<&'a str>,
    produced: usize,
}

impl<'a> Inliner<'a> {
    fn expand(&mut self, value: &'a Value) -> Result<Value, String> {
        self.produced += 1;
        if self.produced > MAX_INLINED_VALUES {
            return Err(format!(
                "local schema references expand to more than {MAX_INLINED_VALUES} values"
            ));
        }
        match value {
            Value::Array(items) => items
                .iter()
                .map(|item| self.expand(item))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Object(map) => match map.get("$ref") {
                Some(Value::String(reference)) if reference.starts_with(LOCAL_PREFIX) => {
                    self.reference(reference, map)
                }
                _ => {
                    let mut out = Map::new();
                    for (key, item) in map {
                        if key != "$defs" {
                            out.insert(key.clone(), self.expand(item)?);
                        }
                    }
                    Ok(Value::Object(out))
                }
            },
            other => Ok(other.clone()),
        }
    }

    /// The target of `reference`, expanded, with the referencing object's other members
    /// (expanded where they stand) laid over it. Laying the siblings over the expanded target is
    /// the same as expanding the target with the siblings laid over it, since a sibling replaces
    /// a whole member; expanding them apart only keeps a sibling's own references from counting
    /// as a cycle through this target.
    fn reference(
        &mut self,
        reference: &'a str,
        referencing: &'a Map<String, Value>,
    ) -> Result<Value, String> {
        let name = &reference[LOCAL_PREFIX.len()..];
        let Some(target @ Value::Object(_)) = self.definitions.get(name) else {
            return Err(format!("unknown local schema reference: {reference}"));
        };
        if self.expanding.contains(&reference) {
            return Err(format!("cyclic local schema reference: {reference}"));
        }
        self.expanding.push(reference);
        let expanded = self.expand(target);
        self.expanding.pop();
        let Value::Object(mut out) = expanded? else {
            // An object always expands to an object.
            return Err(format!("unknown local schema reference: {reference}"));
        };
        for (key, item) in referencing {
            if key != "$ref" && key != "$defs" {
                out.insert(key.clone(), self.expand(item)?);
            }
        }
        Ok(Value::Object(out))
    }
}

/// Strict canonicalization (module doc).
pub fn strict(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(strict_object(map)),
        other => other.clone(),
    }
}

fn strict_object(map: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in map {
        if DROPPED.contains(&key.as_str()) {
            continue;
        }
        let value = match key.as_str() {
            "properties" | "$defs" | "definitions" | "dependentSchemas" => schema_map(value),
            "items" | "prefixItems" | "additionalItems" | "allOf" | "anyOf" | "oneOf" => {
                schema_or_list(value)
            }
            "additionalProperties" | "not" | "if" | "then" | "else" => strict(value),
            _ => value.clone(),
        };
        out.insert(key.clone(), value);
    }
    let names = match out.get("properties") {
        Some(Value::Object(properties)) => Some(
            properties
                .keys()
                .map(|name| Value::String(name.clone()))
                .collect::<Vec<_>>(),
        ),
        _ => None,
    };
    if let Some(names) = names {
        out.insert("required".into(), Value::Array(names));
        out.insert("additionalProperties".into(), Value::Bool(false));
    }
    out
}

/// A map of schemas by name: each value is a schema position, the names are not.
fn schema_map(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(name, schema)| (name.clone(), strict(schema)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A schema, or a list of schemas.
fn schema_or_list(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(strict).collect()),
        other => strict(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(value: &Value) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[test]
    fn strict_reproduces_the_reference_engines_sent_schema() {
        // The reference engine's sent schema: defaults and assertions dropped, every object's
        // properties required and closed.
        let schema = json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer", "default": 0},
                "detail": {
                    "type": "object",
                    "properties": {
                        "label": {
                            "type": "string",
                            "default": "count",
                            "minLength": 1,
                            "pattern": "^[a-z]+$"
                        }
                    }
                }
            }
        });
        let sent = strict(&schema);
        assert_eq!(
            sent,
            json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer"},
                    "detail": {
                        "type": "object",
                        "properties": {"label": {"type": "string"}},
                        "required": ["label"],
                        "additionalProperties": false
                    }
                },
                "additionalProperties": false,
                "required": ["count", "detail"]
            })
        );
        // Member order: required and additionalProperties appended in that order.
        assert_eq!(
            text(&sent),
            r#"{"type":"object","properties":{"count":{"type":"integer"},"detail":{"type":"object","properties":{"label":{"type":"string"}},"required":["label"],"additionalProperties":false}},"required":["count","detail"],"additionalProperties":false}"#
        );
    }

    #[test]
    fn required_and_additional_properties_are_replaced_in_place() {
        let schema = json!({
            "required": ["a"],
            "type": "object",
            "additionalProperties": true,
            "properties": {"a": {"type": "string"}, "b": {"type": "number", "minimum": 0}},
            "title": "T"
        });
        assert_eq!(
            text(&strict(&schema)),
            r#"{"required":["a","b"],"type":"object","additionalProperties":false,"properties":{"a":{"type":"string"},"b":{"type":"number"}},"title":"T"}"#
        );
    }

    #[test]
    fn strict_walks_schema_positions_only() {
        let schema = json!({
            "type": "object",
            "properties": {
                "format": {"type": "string", "format": "date"},
                "pattern": {"type": "string"},
                "default": {"type": "boolean", "default": true},
                "list": {
                    "type": "array",
                    "items": {"type": "object", "properties": {"x": {"type": "number"}}},
                    "minItems": 1,
                    "uniqueItems": true
                },
                "pair": {"type": "array", "prefixItems": [{"type": "string", "maxLength": 3}]},
                "either": {"anyOf": [{"type": "string", "pattern": "x"}, {"type": "null"}]},
                "choice": {"enum": [{"format": "kept", "default": 1}]},
                "fixed": {"const": {"minimum": 3}}
            }
        });
        let sent = strict(&schema);
        let properties = &sent["properties"];
        // Property names that equal keywords survive; their schemas are canonicalized.
        assert_eq!(properties["format"], json!({"type": "string"}));
        assert_eq!(properties["pattern"], json!({"type": "string"}));
        assert_eq!(properties["default"], json!({"type": "boolean"}));
        assert_eq!(
            sent["required"],
            json!([
                "format", "pattern", "default", "list", "pair", "either", "choice", "fixed"
            ])
        );
        assert_eq!(
            properties["list"],
            json!({"type": "array", "items": {"type": "object", "properties": {"x": {"type": "number"}},
                "required": ["x"], "additionalProperties": false}})
        );
        assert_eq!(
            properties["pair"],
            json!({"type": "array", "prefixItems": [{"type": "string"}]})
        );
        assert_eq!(
            properties["either"],
            json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
        );
        // enum and const values are data, not schemas.
        assert_eq!(
            properties["choice"],
            json!({"enum": [{"format": "kept", "default": 1}]})
        );
        assert_eq!(properties["fixed"], json!({"const": {"minimum": 3}}));
    }

    #[test]
    fn strict_reaches_every_applicator() {
        let inner =
            json!({"type": "object", "properties": {"k": {"type": "string", "format": "uri"}}});
        let closed = json!({"type": "object", "properties": {"k": {"type": "string"}},
            "required": ["k"], "additionalProperties": false});
        let schema = json!({
            "$defs": {"d": inner},
            "definitions": {"d": inner},
            "dependentSchemas": {"d": inner},
            "additionalProperties": inner,
            "not": inner, "if": inner, "then": inner, "else": inner,
            "allOf": [inner], "oneOf": [inner, true],
            "items": [inner], "additionalItems": inner,
            "contains": inner, "patternProperties": {"^x": inner}, "propertyNames": inner
        });
        let sent = strict(&schema);
        for key in ["$defs", "definitions", "dependentSchemas"] {
            assert_eq!(sent[key]["d"], closed, "{key}");
        }
        for key in [
            "additionalProperties",
            "not",
            "if",
            "then",
            "else",
            "additionalItems",
        ] {
            assert_eq!(sent[key], closed, "{key}");
        }
        assert_eq!(sent["allOf"], json!([closed]));
        assert_eq!(sent["oneOf"], json!([closed, true]));
        assert_eq!(sent["items"], json!([closed]));
        for key in ["contains", "patternProperties", "propertyNames"] {
            assert!(sent.get(key).is_none(), "{key}");
        }
        assert_eq!(strict(&json!(true)), json!(true));
        // additionalProperties as a boolean stays; with properties it becomes false.
        assert_eq!(
            strict(&json!({"additionalProperties": true})),
            json!({"additionalProperties": true})
        );
    }

    #[test]
    fn refs_are_inlined_with_their_siblings() {
        let schema = json!({
            "type": "object",
            "properties": {
                "where": {"$ref": "#/$defs/point", "description": "the spot"},
                "list": {"type": "array", "items": {"$ref": "#/$defs/point"}}
            },
            "$defs": {
                "point": {
                    "type": "object",
                    "description": "a point",
                    "properties": {"x": {"$ref": "#/$defs/coordinate"}, "y": {"$ref": "#/$defs/coordinate"}}
                },
                "coordinate": {"type": "number", "minimum": 0}
            }
        });
        let inlined = inline_refs(&schema).unwrap();
        let point = json!({"type": "object", "description": "a point",
            "properties": {"x": {"type": "number", "minimum": 0}, "y": {"type": "number", "minimum": 0}}});
        assert_eq!(
            inlined,
            json!({
                "type": "object",
                "properties": {
                    "where": {"type": "object", "description": "the spot",
                        "properties": {"x": {"type": "number", "minimum": 0}, "y": {"type": "number", "minimum": 0}}},
                    "list": {"type": "array", "items": point}
                }
            })
        );
        // The sibling replaces the target's member in place.
        assert_eq!(
            text(&inlined["properties"]["where"]),
            r#"{"type":"object","description":"the spot","properties":{"x":{"type":"number","minimum":0},"y":{"type":"number","minimum":0}}}"#
        );
    }

    #[test]
    fn a_reference_to_a_reference_carries_every_sibling() {
        let schema = json!({
            "properties": {"a": {"$ref": "#/$defs/a", "title": "outer"}},
            "$defs": {
                "a": {"$ref": "#/$defs/b", "description": "from a", "title": "inner"},
                "b": {"type": "string", "title": "b", "description": "from b", "maxLength": 3}
            }
        });
        assert_eq!(
            text(&inline_refs(&schema).unwrap()),
            r#"{"properties":{"a":{"type":"string","title":"outer","description":"from a","maxLength":3}}}"#
        );
    }

    #[test]
    fn nested_defs_are_dropped_and_other_refs_left() {
        let schema = json!({
            "$defs": {"a": {"type": "string"}},
            "properties": {
                "x": {"$ref": "#/definitions/legacy"},
                "y": {"type": "object", "$defs": {"z": {"type": "number"}}},
                "w": {"$ref": "#"}
            },
            "definitions": {"legacy": {"type": "number"}}
        });
        assert_eq!(
            inline_refs(&schema).unwrap(),
            json!({
                "properties": {
                    "x": {"$ref": "#/definitions/legacy"},
                    "y": {"type": "object"},
                    "w": {"$ref": "#"}
                },
                "definitions": {"legacy": {"type": "number"}}
            })
        );
        // Without a root $defs nothing changes, nested $defs included.
        let plain = json!({"properties": {"y": {"$defs": {}, "$ref": "#/$defs/missing"}}});
        assert_eq!(inline_refs(&plain).unwrap(), plain);
    }

    #[test]
    fn unknown_and_cyclic_references_are_refused() {
        let unknown =
            json!({"$defs": {"a": {"type": "string"}}, "properties": {"x": {"$ref": "#/$defs/b"}}});
        assert_eq!(
            inline_refs(&unknown).unwrap_err(),
            "unknown local schema reference: #/$defs/b"
        );
        let not_a_schema_object = json!({"$defs": {"a": true}, "items": {"$ref": "#/$defs/a"}});
        assert_eq!(
            inline_refs(&not_a_schema_object).unwrap_err(),
            "unknown local schema reference: #/$defs/a"
        );
        let direct = json!({"$defs": {"node": {"type": "object", "properties": {"next": {"$ref": "#/$defs/node"}}}},
            "$ref": "#/$defs/node"});
        assert_eq!(
            inline_refs(&direct).unwrap_err(),
            "cyclic local schema reference: #/$defs/node"
        );
        let indirect = json!({"$defs": {"a": {"items": {"$ref": "#/$defs/b"}}, "b": {"items": {"$ref": "#/$defs/a"}}},
            "properties": {"p": {"$ref": "#/$defs/a"}}});
        assert_eq!(
            inline_refs(&indirect).unwrap_err(),
            "cyclic local schema reference: #/$defs/a"
        );
        // A sibling naming the same target is not a cycle: it is expanded where it stands.
        let sibling = json!({"$defs": {"a": {"type": "object"}},
            "properties": {"p": {"$ref": "#/$defs/a", "properties": {"q": {"$ref": "#/$defs/a"}}}}});
        assert_eq!(
            inline_refs(&sibling).unwrap(),
            json!({"properties": {"p": {"type": "object", "properties": {"q": {"type": "object"}}}}})
        );
    }

    #[test]
    fn exponential_copies_are_refused() {
        let mut definitions = Map::new();
        for i in 0..40 {
            definitions.insert(
                format!("d{i}"),
                json!({"allOf": [{"$ref": format!("#/$defs/d{}", i + 1)}, {"$ref": format!("#/$defs/d{}", i + 1)}]}),
            );
        }
        definitions.insert("d40".into(), json!({"type": "string"}));
        let schema = json!({"$defs": definitions, "$ref": "#/$defs/d0"});
        assert_eq!(
            inline_refs(&schema).unwrap_err(),
            "local schema references expand to more than 1000000 values"
        );
    }
}
