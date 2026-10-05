//! Node specs: a [`TypeSpec`] from a host or the built-in catalog, validated (protocol.md §4).
//!
//! Validation, in this order (each failure a sentence; the registry reports it as
//! `<uses>: <message>` on the step):
//! - the name matches `^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$`
//!   (`node type name 'X' must be lower_snake words joined by .`);
//! - each input, then each param, then each output: its name matches `^[a-z][a-z0-9_]*$`
//!   (`{name}: input name 'X' must be a lower_snake word`); a port parses
//!   (`port 'image?[]' is not kind, kind[], kind{} with an optional ?`,
//!   `port kind 'x' is not one of [...]`); a param is a JSON Schema object whose `x-fx-template`
//!   and `x-fx-optional` are booleans;
//! - no name is both an input and a param (`{name}: ['x'] declared as both input and param`);
//! - every `calls` bound is a whole number ≥ 1 (`{name}: calls['c'] must be at least 1`) or the
//!   name of an integer param whose schema has a numeric `minimum` ≥ 1 (`… names 'n', which is not
//!   an integer setting with a minimum of at least 1`) and a numeric `maximum`
//!   (`… needs 'n' to set a maximum`); booleans are not numbers;
//! - a catalog type with a capability makes no other calls
//!   (`{name}: a capability type is its own one call`);
//! - resources are POSIX paths relative to the project root, with no empty, `.` or `..` segment;
//! - tools match `^[a-z0-9][a-z0-9_-]*` plus an optional version bound starting with one of
//!   `<>=!~` (`blender>=4.2`).
//!
//! Quoted names are Python `repr`s, as the messages of FX's predecessor wrote them.

use crate::text::py_repr_str;
use crate::val::Val;
use crate::value::as_f64;
use grida_fx_protocol::TypeSpec;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// The port families (protocol.md §4), sorted.
const FAMILIES: [&str; 8] = [
    "annotations",
    "audio",
    "file",
    "image",
    "json",
    "model",
    "text",
    "video",
];

static PORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<kind>[a-z0-9]+(?:/[a-z0-9.+-]+)?)(?P<shape>\[\]|\{\})?(?P<optional>\?)?$")
        .expect("a valid pattern")
});
static TYPE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*$").expect("a valid pattern")
});
static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]*$").expect("a valid pattern"));
static TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9][a-z0-9_-]*(?:\s*[<>=!~].*)?$").expect("a valid pattern")
});

/// How many files a port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    One,
    /// `kind[]`
    List,
    /// `kind{}`
    Keyed,
}

/// An input or output port (protocol.md §4 "Ports").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    /// A family (`image`) or a media type in it (`image/png`).
    pub kind: String,
    pub shape: Shape,
    pub optional: bool,
}

impl Port {
    /// Parses port notation.
    pub fn parse(notation: &str) -> Result<Port, String> {
        let captures = PORT.captures(notation).ok_or_else(|| {
            format!(
                "port {} is not kind, kind[], kind{{}} with an optional ?",
                py_repr_str(notation)
            )
        })?;
        let kind = &captures["kind"];
        if !FAMILIES.contains(&crate::kinds::family(kind)) {
            return Err(format!(
                "port kind {} is not one of {}",
                py_repr_str(kind),
                py_list(FAMILIES.iter().copied())
            ));
        }
        let shape = match captures.name("shape").map(|m| m.as_str()) {
            Some("[]") => Shape::List,
            Some(_) => Shape::Keyed,
            None => Shape::One,
        };
        Ok(Port {
            kind: kind.to_string(),
            shape,
            optional: captures.name("optional").is_some(),
        })
    }

    /// The notation: kind, then `[]`/`{}`, then `?`.
    pub fn notation(&self) -> String {
        let shape = match self.shape {
            Shape::One => "",
            Shape::List => "[]",
            Shape::Keyed => "{}",
        };
        let optional = if self.optional { "?" } else { "" };
        format!("{}{shape}{optional}", self.kind)
    }

    /// The family of the port's kind.
    pub fn family(&self) -> &str {
        crate::kinds::family(&self.kind)
    }
}

/// A call bound (protocol.md §4 "Calls").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallBound {
    Count(u32),
    /// An integer param with a minimum ≥ 1 and a maximum.
    Param(String),
}

pub use grida_fx_protocol::RetryMode as Retry;

/// Who runs a type's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// A paid built-in: the engine makes its one capability call (step 4).
    Capability,
    /// A built-in whose body is in `grida.fx.std`, run by the Python host (step 3).
    Python,
    /// `fx/select@1`: the engine itself.
    Engine,
    /// A built-in that plans, identifies and prices but has no body yet.
    None,
    /// A project type, run by its language's host.
    Project,
}

/// A validated node type.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSpec {
    pub name: String,
    pub description: Option<String>,
    pub inputs: IndexMap<String, Port>,
    /// JSON Schemas, with `x-fx-template`, `x-fx-optional` and `default`.
    pub params: IndexMap<String, Value>,
    pub outputs: IndexMap<String, Port>,
    pub judge: bool,
    /// A paid built-in's own capability: it is one call of it (`calls` is then empty).
    pub capability: Option<String>,
    pub calls: IndexMap<String, CallBound>,
    pub resources: Vec<String>,
    pub tools: Vec<String>,
    pub view: Option<String>,
    pub version: Option<u64>,
    pub retry: Retry,
}

impl NodeSpec {
    /// Validates a wire spec (module doc). `capability` is set only for catalog types.
    pub fn from_type_spec(spec: &TypeSpec, capability: Option<String>) -> Result<NodeSpec, String> {
        let name = &spec.name;
        if !TYPE_NAME.is_match(name) {
            return Err(format!(
                "node type name {} must be lower_snake words joined by .",
                py_repr_str(name)
            ));
        }
        let inputs = ports(name, "input", &spec.inputs)?;
        for (param, schema) in &spec.params {
            check_name(name, "param", param)?;
            check_param(name, param, schema)?;
        }
        let outputs = ports(name, "output", &spec.outputs)?;
        let overlap: BTreeSet<&str> = spec
            .inputs
            .keys()
            .filter(|input| spec.params.contains_key(*input))
            .map(String::as_str)
            .collect();
        if !overlap.is_empty() {
            return Err(format!(
                "{name}: {} declared as both input and param",
                py_list(overlap.into_iter())
            ));
        }
        let mut calls = IndexMap::new();
        for (capability, bound) in &spec.calls {
            calls.insert(
                capability.clone(),
                call_bound(name, &spec.params, capability, bound)?,
            );
        }
        if capability.is_some() && !calls.is_empty() {
            return Err(format!("{name}: a capability type is its own one call"));
        }
        for resource in &spec.resources {
            if !is_resource_path(resource) {
                return Err(format!(
                    "{name}: resource {} is not a POSIX path relative to the project root \
                     with no empty, . or .. segment",
                    py_repr_str(resource)
                ));
            }
        }
        for tool in &spec.tools {
            if !TOOL.is_match(tool) {
                return Err(format!(
                    "{name}: tool {} is not a name of a-z, 0-9, _ and - with an optional \
                     version bound such as >=4.2",
                    py_repr_str(tool)
                ));
            }
        }
        Ok(NodeSpec {
            name: name.clone(),
            description: spec.description.clone(),
            inputs,
            params: spec.params.clone(),
            outputs,
            judge: spec.judge,
            capability,
            calls,
            resources: spec.resources.clone(),
            tools: spec.tools.clone(),
            view: spec.view.clone(),
            version: spec.version,
            retry: spec.retry,
        })
    }

    /// The spec back as wire data (for `grida-fx nodes` and tests). A catalog type's capability
    /// is not part of the wire form.
    pub fn to_type_spec(&self) -> TypeSpec {
        let notation = |ports: &IndexMap<String, Port>| {
            ports
                .iter()
                .map(|(name, port)| (name.clone(), port.notation()))
                .collect()
        };
        TypeSpec {
            name: self.name.clone(),
            description: self.description.clone(),
            inputs: notation(&self.inputs),
            params: self.params.clone(),
            outputs: notation(&self.outputs),
            judge: self.judge,
            calls: self
                .calls
                .iter()
                .map(|(capability, bound)| {
                    let bound = match bound {
                        CallBound::Count(n) => Value::from(*n),
                        CallBound::Param(param) => Value::from(param.as_str()),
                    };
                    (capability.clone(), bound)
                })
                .collect(),
            resources: self.resources.clone(),
            tools: self.tools.clone(),
            view: self.view.clone(),
            version: self.version,
            retry: self.retry,
        }
    }

    /// Whether the type makes paid calls: a capability, or any `calls`.
    pub fn paid(&self) -> bool {
        self.capability.is_some() || !self.calls.is_empty()
    }

    /// The most calls of each capability one run makes, for an instance's with-values: a
    /// capability type `{capability: 1}`; a count as is; a param bound the step's value when it
    /// is a whole number (not capped at `maximum`, saturating at `u32::MAX`), else the param's
    /// `maximum` (a missing, pending, fractional, negative or non-number value). In the spec's
    /// `calls` order.
    pub fn capability_calls(&self, with: &IndexMap<String, Val>) -> IndexMap<String, u32> {
        if let Some(capability) = &self.capability {
            return IndexMap::from([(capability.clone(), 1)]);
        }
        self.calls
            .iter()
            .map(|(capability, bound)| {
                let count = match bound {
                    CallBound::Count(n) => *n,
                    CallBound::Param(param) => match with.get(param) {
                        Some(Val::Number(x)) if x.fract() == 0.0 && *x >= 0.0 => *x as u32,
                        _ => self.maximum_of(param),
                    },
                };
                (capability.clone(), count)
            })
            .collect()
    }

    /// A param's `maximum` as a whole count (truncated; 0 when absent, which validation refuses).
    fn maximum_of(&self, param: &str) -> u32 {
        match self.params.get(param).and_then(|s| s.get("maximum")) {
            Some(Value::Number(n)) => as_f64(n).trunc().max(0.0) as u32,
            _ => 0,
        }
    }

    /// Whether a param is a template (`x-fx-template: true`).
    pub fn is_template(&self, param: &str) -> bool {
        self.flag(param, "x-fx-template")
    }

    /// Whether a param may be left out with no default (`x-fx-optional: true`).
    pub fn is_optional_param(&self, param: &str) -> bool {
        self.flag(param, "x-fx-optional")
    }

    fn flag(&self, param: &str, keyword: &str) -> bool {
        self.params
            .get(param)
            .and_then(|s| s.get(keyword))
            .is_some_and(|v| v == &Value::Bool(true))
    }

    /// A param's `default`, when it has one.
    pub fn default_of(&self, param: &str) -> Option<&Value> {
        self.params.get(param).and_then(|s| s.get("default"))
    }

    /// `judge`, `paid` or `free`, as `grida-fx nodes` prints it.
    pub fn kind_word(&self) -> &'static str {
        if self.judge {
            "judge"
        } else if self.paid() {
            "paid"
        } else {
            "free"
        }
    }
}

/// A Python list repr of strings: `['a', 'b']`.
fn py_list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let items: Vec<String> = items.map(py_repr_str).collect();
    format!("[{}]", items.join(", "))
}

fn check_name(type_name: &str, what: &str, name: &str) -> Result<(), String> {
    if NAME.is_match(name) {
        Ok(())
    } else {
        Err(format!(
            "{type_name}: {what} name {} must be a lower_snake word",
            py_repr_str(name)
        ))
    }
}

fn ports(
    type_name: &str,
    what: &str,
    declared: &IndexMap<String, String>,
) -> Result<IndexMap<String, Port>, String> {
    let mut ports = IndexMap::new();
    for (name, notation) in declared {
        check_name(type_name, what, name)?;
        ports.insert(name.clone(), Port::parse(notation)?);
    }
    Ok(ports)
}

fn check_param(type_name: &str, param: &str, schema: &Value) -> Result<(), String> {
    let Value::Object(schema) = schema else {
        return Err(format!(
            "{type_name}: param {} is not a JSON Schema object",
            py_repr_str(param)
        ));
    };
    for keyword in ["x-fx-template", "x-fx-optional"] {
        if schema.get(keyword).is_some_and(|v| !v.is_boolean()) {
            return Err(format!(
                "{type_name}: param {} has a {keyword} that is not true or false",
                py_repr_str(param)
            ));
        }
    }
    Ok(())
}

fn call_bound(
    type_name: &str,
    params: &IndexMap<String, Value>,
    capability: &str,
    bound: &Value,
) -> Result<CallBound, String> {
    let calls = format!("{type_name}: calls[{}]", py_repr_str(capability));
    match bound {
        Value::String(param) => {
            let setting = params.get(param);
            let integer = setting.and_then(|s| s.get("type")) == Some(&Value::from("integer"));
            let minimum = match setting.and_then(|s| s.get("minimum")) {
                Some(Value::Number(n)) => as_f64(n),
                // An absent minimum is 0; anything else is not a number.
                _ => 0.0,
            };
            if !integer || minimum < 1.0 {
                return Err(format!(
                    "{calls} names {}, which is not an integer setting with a minimum of at \
                     least 1",
                    py_repr_str(param)
                ));
            }
            if !matches!(
                setting.and_then(|s| s.get("maximum")),
                Some(Value::Number(_))
            ) {
                return Err(format!(
                    "{calls} needs {} to set a maximum",
                    py_repr_str(param)
                ));
            }
            Ok(CallBound::Param(param.clone()))
        }
        Value::Number(n) => {
            let x = as_f64(n);
            if x < 1.0 {
                Err(format!("{calls} must be at least 1"))
            } else if x.fract() != 0.0 {
                Err(format!("{calls} must be a whole number"))
            } else {
                Ok(CallBound::Count(x as u32))
            }
        }
        _ => Err(format!(
            "{calls} is neither a number of at least 1 nor the name of an integer setting"
        )),
    }
}

/// A resource path: POSIX, relative to the project root, every segment non-empty and neither `.`
/// nor `..`, no backslash (the `resources` pattern of the node protocol schema).
fn is_resource_path(path: &str) -> bool {
    !path.contains('\\')
        && path
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(value: Value) -> TypeSpec {
        let mut spec = json!({
            "name": "t",
            "inputs": {},
            "params": {},
            "outputs": {},
            "judge": false,
            "calls": {},
            "resources": [],
            "tools": [],
            "view": null,
            "version": null,
            "retry": "service",
        });
        for (key, v) in value.as_object().unwrap() {
            spec[key] = v.clone();
        }
        serde_json::from_value(spec).unwrap()
    }

    fn refused(value: Value) -> String {
        NodeSpec::from_type_spec(&wire(value), None).unwrap_err()
    }

    #[test]
    fn ports_parse_and_print() {
        for (notation, kind, shape, optional) in [
            ("image", "image", Shape::One, false),
            ("image/png", "image/png", Shape::One, false),
            ("image[]?", "image", Shape::List, true),
            ("text{}", "text", Shape::Keyed, false),
            ("model/gltf+json?", "model/gltf+json", Shape::One, true),
            ("annotations", "annotations", Shape::One, false),
        ] {
            let port = Port::parse(notation).unwrap();
            assert_eq!(
                port,
                Port {
                    kind: kind.into(),
                    shape,
                    optional
                }
            );
            assert_eq!(port.notation(), notation);
        }
        assert_eq!(Port::parse("image/png").unwrap().family(), "image");
        assert_eq!(
            Port::parse("image?[]").unwrap_err(),
            "port 'image?[]' is not kind, kind[], kind{} with an optional ?"
        );
        assert_eq!(
            Port::parse("Image").unwrap_err(),
            "port 'Image' is not kind, kind[], kind{} with an optional ?"
        );
        assert!(Port::parse("image\n").is_err());
        assert!(Port::parse("").is_err());
        assert_eq!(
            Port::parse("picture").unwrap_err(),
            "port kind 'picture' is not one of ['annotations', 'audio', 'file', 'image', \
             'json', 'model', 'text', 'video']"
        );
        assert!(
            Port::parse("x/png[]")
                .unwrap_err()
                .starts_with("port kind 'x/png'")
        );
    }

    #[test]
    fn a_full_spec_round_trips() {
        let source = wire(json!({
            "name": "agent.loop",
            "description": "Loops.",
            "inputs": {"image": "image/png", "refs": "image[]?"},
            "params": {
                "prompt": {"type": "string", "x-fx-template": true},
                "steps": {"type": "integer", "minimum": 1, "maximum": 8, "default": 3},
                "size": {"type": "string", "x-fx-optional": true},
            },
            "outputs": {"report": "json", "frames": "image{}?"},
            "judge": true,
            "calls": {"image.generate": 2, "structured.generate": "steps"},
            "resources": ["prompts/a.md", ".hidden/b..c"],
            "tools": ["blender>=4.2", "ffmpeg", "magick ~= 7"],
            "view": "image",
            "version": 3,
            "retry": "engine",
        }));
        let spec = NodeSpec::from_type_spec(&source, None).unwrap();
        assert_eq!(spec.calls["image.generate"], CallBound::Count(2));
        assert_eq!(
            spec.calls["structured.generate"],
            CallBound::Param("steps".into())
        );
        assert_eq!(spec.inputs["refs"].shape, Shape::List);
        assert_eq!(spec.to_type_spec(), source);
        assert!(spec.is_template("prompt"));
        assert!(!spec.is_template("steps"));
        assert!(spec.is_optional_param("size"));
        assert!(!spec.is_optional_param("prompt"));
        assert!(!spec.is_optional_param("nope"));
        assert_eq!(spec.default_of("steps"), Some(&json!(3)));
        assert_eq!(spec.default_of("prompt"), None);
        assert!(spec.paid());
        assert_eq!(spec.kind_word(), "judge");
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(
            refused(json!({"name": "Bad"})),
            "node type name 'Bad' must be lower_snake words joined by ."
        );
        assert!(refused(json!({"name": "a..b"})).starts_with("node type name 'a..b'"));
        assert!(refused(json!({"name": "a.1b"})).starts_with("node type name"));
        assert!(NodeSpec::from_type_spec(&wire(json!({"name": "a.b_c.d2"})), None).is_ok());
        assert_eq!(
            refused(json!({"inputs": {"Image": "image"}})),
            "t: input name 'Image' must be a lower_snake word"
        );
        assert_eq!(
            refused(json!({"params": {"a-b": {}}})),
            "t: param name 'a-b' must be a lower_snake word"
        );
        assert_eq!(
            refused(json!({"outputs": {"_x": "image"}})),
            "t: output name '_x' must be a lower_snake word"
        );
        assert_eq!(
            refused(json!({"outputs": {"x": "img"}})),
            "port kind 'img' is not one of ['annotations', 'audio', 'file', 'image', 'json', \
             'model', 'text', 'video']"
        );
        assert_eq!(
            refused(json!({
                "inputs": {"b": "image", "a": "image", "c": "image"},
                "params": {"c": {}, "b": {}},
            })),
            "t: ['b', 'c'] declared as both input and param"
        );
    }

    #[test]
    fn params_are_schemas() {
        assert_eq!(
            refused(json!({"params": {"x": "string"}})),
            "t: param 'x' is not a JSON Schema object"
        );
        assert_eq!(
            refused(json!({"params": {"x": {"x-fx-template": "yes"}}})),
            "t: param 'x' has a x-fx-template that is not true or false"
        );
        assert_eq!(
            refused(json!({"params": {"x": {"x-fx-optional": 1}}})),
            "t: param 'x' has a x-fx-optional that is not true or false"
        );
    }

    #[test]
    fn call_bounds() {
        let with_param = |schema: Value| {
            refused(json!({"params": {"n": schema}, "calls": {"image.generate": "n"}}))
        };
        let not_integer = "t: calls['image.generate'] names 'n', which is not an integer setting \
                           with a minimum of at least 1";
        assert_eq!(
            with_param(json!({"type": "integer", "maximum": 4})),
            not_integer
        );
        assert_eq!(
            with_param(json!({"type": "number", "minimum": 1, "maximum": 4})),
            not_integer
        );
        assert_eq!(
            with_param(json!({"type": "integer", "minimum": 0, "maximum": 4})),
            not_integer
        );
        assert_eq!(
            with_param(json!({"type": "integer", "minimum": true, "maximum": 4})),
            not_integer
        );
        assert_eq!(
            with_param(json!({"type": "integer", "minimum": 1})),
            "t: calls['image.generate'] needs 'n' to set a maximum"
        );
        assert_eq!(
            with_param(json!({"type": "integer", "minimum": 1, "maximum": "4"})),
            "t: calls['image.generate'] needs 'n' to set a maximum"
        );
        assert_eq!(
            refused(json!({"calls": {"image.generate": "missing"}})),
            "t: calls['image.generate'] names 'missing', which is not an integer setting with a \
             minimum of at least 1"
        );
        assert_eq!(
            refused(json!({"calls": {"image.generate": 0}})),
            "t: calls['image.generate'] must be at least 1"
        );
        assert_eq!(
            refused(json!({"calls": {"image.generate": -2}})),
            "t: calls['image.generate'] must be at least 1"
        );
        assert_eq!(
            refused(json!({"calls": {"image.generate": 1.5}})),
            "t: calls['image.generate'] must be a whole number"
        );
        assert_eq!(
            refused(json!({"calls": {"image.generate": true}})),
            "t: calls['image.generate'] is neither a number of at least 1 nor the name of an \
             integer setting"
        );
        let spec = NodeSpec::from_type_spec(&wire(json!({"calls": {"image.generate": 2.0}})), None)
            .unwrap();
        assert_eq!(spec.calls["image.generate"], CallBound::Count(2));
        assert_eq!(
            NodeSpec::from_type_spec(
                &wire(json!({"calls": {"image.generate": 1}})),
                Some("image.generate".into())
            )
            .unwrap_err(),
            "t: a capability type is its own one call"
        );
    }

    #[test]
    fn resources_and_tools() {
        for bad in [
            "/abs.md",
            "a/../b.md",
            "../b.md",
            "./a.md",
            "a//b",
            "a/",
            "",
            "a\\b",
            ".",
        ] {
            assert_eq!(
                refused(json!({"resources": [bad]})),
                format!(
                    "t: resource {} is not a POSIX path relative to the project root with no \
                     empty, . or .. segment",
                    py_repr_str(bad)
                ),
                "{bad:?}"
            );
        }
        for good in ["a.md", "prompts/r.md", ".a/..b/c..", "x y/z"] {
            assert!(
                NodeSpec::from_type_spec(&wire(json!({"resources": [good]})), None).is_ok(),
                "{good:?}"
            );
        }
        for bad in ["Blender", "-x", "_x", "x y", "", "x/y"] {
            assert_eq!(
                refused(json!({"tools": [bad]})),
                format!(
                    "t: tool {} is not a name of a-z, 0-9, _ and - with an optional version \
                     bound such as >=4.2",
                    py_repr_str(bad)
                ),
                "{bad:?}"
            );
        }
        for good in [
            "blender>=4.2",
            "x>",
            "a_b-c",
            "0x",
            "ffmpeg !=6",
            "magick~7",
        ] {
            assert!(
                NodeSpec::from_type_spec(&wire(json!({"tools": [good]})), None).is_ok(),
                "{good:?}"
            );
        }
    }

    #[test]
    fn capability_calls_follow_the_step() {
        let spec = NodeSpec::from_type_spec(
            &wire(json!({
                "params": {"n": {"type": "integer", "minimum": 1, "maximum": 4}},
                "calls": {"image.generate": 2, "structured.generate": "n"},
            })),
            None,
        )
        .unwrap();
        let calls = |with: IndexMap<String, Val>| {
            spec.capability_calls(&with).into_iter().collect::<Vec<_>>()
        };
        let expected = |n: u32| {
            vec![
                ("image.generate".to_string(), 2),
                ("structured.generate".to_string(), n),
            ]
        };
        assert_eq!(calls(IndexMap::new()), expected(4));
        // The step's value is not capped at the maximum.
        assert_eq!(
            calls(IndexMap::from([("n".into(), Val::Number(7.0))])),
            expected(7)
        );
        assert_eq!(
            calls(IndexMap::from([("n".into(), Val::Number(0.0))])),
            expected(0)
        );
        for other in [
            Val::Number(2.5),
            Val::Number(-1.0),
            Val::Bool(true),
            Val::Str("3".into()),
            Val::Missing,
        ] {
            assert_eq!(calls(IndexMap::from([("n".into(), other)])), expected(4));
        }
        assert_eq!(
            calls(IndexMap::from([("n".into(), Val::Number(1e12))])),
            expected(u32::MAX)
        );
        let capability =
            NodeSpec::from_type_spec(&wire(json!({})), Some("image.edit".into())).unwrap();
        assert_eq!(
            capability.capability_calls(&IndexMap::new()),
            IndexMap::from([("image.edit".to_string(), 1)])
        );
        assert!(capability.paid());
        assert_eq!(capability.kind_word(), "paid");
        let free = NodeSpec::from_type_spec(&wire(json!({})), None).unwrap();
        assert!(free.capability_calls(&IndexMap::new()).is_empty());
        assert_eq!(free.kind_word(), "free");
    }
}
