//! The built-in node types (`fx/<name>@<major>`), embedded as data: `catalog.json`, the 37
//! standard types ported from FX's predecessor's standard library.
//!
//! `catalog.json` is `{"kind": "fx-std-catalog-v1", "types": [...]}`; each type has `uses`,
//! `name`, `major`, `version`, `description` (optional), `inputs`, `params`, `outputs`, `judge`,
//! `capability`, `calls`, `resources`, `tools`, `retry`, and `body` (`capability`, `python`,
//! `engine` or `none`), in declaration order. A built-in's identity is
//! `fx/<name>@<major>.<version>` (identity.md §6). The engine owns the capability types and
//! `fx/select@1` (protocol.md §4 "Engine types"); the Python host runs the `python` bodies.
//!
//! The catalog is read once, with FX's JSON reader (one number type), and every spec is
//! validated through [`crate::spec::NodeSpec::from_type_spec`] with its capability. `nodes`
//! kinds are 11 paid, 5 judge, 21 free.

use crate::spec::{BodyKind, NodeSpec};
use grida_fx_protocol::TypeSpec;
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

/// The embedded catalog.
pub const CATALOG_JSON: &str = include_str!("catalog.json");

/// One built-in type.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltinType {
    /// `fx/<name>@<major>`.
    pub uses: String,
    pub name: String,
    pub major: u32,
    pub spec: NodeSpec,
    pub body: BodyKind,
}

impl BuiltinType {
    /// `fx/<name>@<major>.<version>`.
    pub fn identity(&self) -> String {
        format!("{}.{}", self.uses, self.spec.version.unwrap_or(0))
    }
}

/// Every built-in, in catalog order (parsed once).
pub fn builtins() -> &'static [BuiltinType] {
    static BUILTINS: OnceLock<Vec<BuiltinType>> = OnceLock::new();
    BUILTINS.get_or_init(|| {
        // The catalog is part of the engine and its tests read it; a defect is a build defect.
        parse_catalog(CATALOG_JSON)
            .unwrap_or_else(|reason| panic!("the embedded built-in catalog is invalid: {reason}"))
    })
}

/// The built-in `name` at `major`.
pub fn builtin(name: &str, major: u32) -> Option<&'static BuiltinType> {
    builtins()
        .iter()
        .find(|b| b.name == name && b.major == major)
}

/// The catalog document.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    kind: String,
    types: Vec<Entry>,
}

/// One catalog entry: the wire spec's fields plus `uses`, `major`, `capability` and `body`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    uses: String,
    name: String,
    major: u32,
    version: u64,
    #[serde(default)]
    description: Option<String>,
    inputs: indexmap::IndexMap<String, String>,
    params: indexmap::IndexMap<String, Value>,
    outputs: indexmap::IndexMap<String, String>,
    judge: bool,
    capability: Option<String>,
    calls: indexmap::IndexMap<String, Value>,
    resources: Vec<String>,
    tools: Vec<String>,
    #[serde(default)]
    view: Option<String>,
    retry: crate::spec::Retry,
    body: String,
}

/// Reads a catalog document into built-in types, checking each entry.
fn parse_catalog(text: &str) -> Result<Vec<BuiltinType>, String> {
    let value = crate::value::parse_json(text).map_err(|refused| refused.message)?;
    let catalog: Catalog = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if catalog.kind != "fx-std-catalog-v1" {
        return Err(format!(
            "its kind is {}, not fx-std-catalog-v1",
            catalog.kind
        ));
    }
    let mut types: Vec<BuiltinType> = Vec::with_capacity(catalog.types.len());
    for entry in catalog.types {
        let uses = format!("fx/{}@{}", entry.name, entry.major);
        if entry.uses != uses {
            return Err(format!("{} should be {uses}", entry.uses));
        }
        if types.iter().any(|t| t.uses == uses) {
            return Err(format!("{uses} is declared twice"));
        }
        let body = match entry.body.as_str() {
            "capability" => BodyKind::Capability,
            "python" => BodyKind::Python,
            "engine" => BodyKind::Engine,
            "none" => BodyKind::None,
            other => return Err(format!("{uses}: {other} is not a body kind")),
        };
        if (body == BodyKind::Capability) != entry.capability.is_some() {
            return Err(format!(
                "{uses}: a capability body and a capability come together"
            ));
        }
        let wire = TypeSpec {
            name: entry.name.clone(),
            description: entry.description,
            inputs: entry.inputs,
            params: entry.params,
            outputs: entry.outputs,
            judge: entry.judge,
            calls: entry.calls,
            resources: entry.resources,
            tools: entry.tools,
            view: entry.view,
            version: Some(entry.version),
            retry: entry.retry,
        };
        let spec = NodeSpec::from_type_spec(&wire, entry.capability)
            .map_err(|reason| format!("{uses}: {reason}"))?;
        types.push(BuiltinType {
            uses,
            name: entry.name,
            major: entry.major,
            spec,
            body,
        });
    }
    Ok(types)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_parses() {
        assert_eq!(builtins().len(), 37);
        let generate = builtin("image.generate", 1).unwrap();
        assert_eq!(generate.uses, "fx/image.generate@1");
        assert_eq!(generate.identity(), "fx/image.generate@1.1");
        assert_eq!(generate.body, BodyKind::Capability);
        assert_eq!(generate.spec.capability.as_deref(), Some("image.generate"));
        assert!(builtin("image.generate", 2).is_none());
        assert!(builtin("nope", 1).is_none());
        assert_eq!(builtin("select", 1).unwrap().body, BodyKind::Engine);
        assert_eq!(builtin("image.resize", 1).unwrap().body, BodyKind::Python);
        assert_eq!(builtin("image.key", 1).unwrap().body, BodyKind::None);
    }

    #[test]
    fn bad_catalogs_are_refused() {
        let entry = |extra: &str| {
            format!(
                r#"{{"kind": "fx-std-catalog-v1", "types": [{{"uses": "fx/a@1", "name": "a",
                "major": 1, "version": 1, "inputs": {{}}, "params": {{}}, "outputs": {{}},
                "judge": false, "capability": null, "calls": {{}}, "resources": [], "tools": [],
                "retry": "service", "body": "python"{extra}}}]}}"#
            )
        };
        assert_eq!(parse_catalog(&entry("")).unwrap().len(), 1);
        assert!(parse_catalog(&entry(r#", "extra": 1"#)).is_err());
        assert!(parse_catalog(r#"{"kind": "x", "types": []}"#).is_err());
        let twice = entry("").replace(
            "\"types\": [",
            "\"types\": [{\"uses\": \"fx/a@1\", \"name\": \"a\", \"major\": 1, \"version\": 1, \
             \"inputs\": {}, \"params\": {}, \"outputs\": {}, \"judge\": false, \
             \"capability\": null, \"calls\": {}, \"resources\": [], \"tools\": [], \
             \"retry\": \"service\", \"body\": \"python\"}, ",
        );
        assert_eq!(
            parse_catalog(&twice).unwrap_err(),
            "fx/a@1 is declared twice"
        );
        let wrong_uses = entry("").replace("\"uses\": \"fx/a@1\"", "\"uses\": \"fx/b@1\"");
        assert_eq!(
            parse_catalog(&wrong_uses).unwrap_err(),
            "fx/b@1 should be fx/a@1"
        );
        let bad_body = entry("").replace("\"body\": \"python\"", "\"body\": \"capability\"");
        assert!(parse_catalog(&bad_body).is_err());
        let bad_spec = entry("").replace("\"inputs\": {}", "\"inputs\": {\"x\": \"img\"}");
        assert!(
            parse_catalog(&bad_spec)
                .unwrap_err()
                .starts_with("fx/a@1: port kind 'img'")
        );
    }
}
