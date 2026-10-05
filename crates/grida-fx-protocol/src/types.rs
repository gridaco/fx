//! The session, `describe` and `build` messages (protocol.md §2, §4, §5.1, §5.2): what step 2
//! uses. `run` and the host-to-engine requests are in [`crate::run_types`].
//!
//! Field names are the schema's. An optional member is an `Option` left out when `None`; a
//! required member that may be `null` is an `Option` that must be present (read with `nullable`).
//! Where the schema says `additionalProperties: false`, unknown members are refused, except in
//! [`TypeSpec`]: the engine validates a spec's content itself, and a catalog entry that carries
//! more than a spec (`uses`, `major`, `body`, …) deserializes as one.
//!
//! Tests: serialize each type and validate it against its `$defs` entry of
//! spec/schemas/fx-node-protocol-v1.schema.json (load the schema from the repository in the test);
//! deserialize protocol.md §9's describe result.

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Reads a member that the schema requires but allows to be `null`: with
/// `#[serde(deserialize_with = "nullable")]` and no `default`, a missing member is refused
/// instead of read as `None`.
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Reads a member that may be absent or `null` and keeps the difference: with
/// `#[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]`, a
/// missing member is `None` and a `null` one `Some(Value::Null)`.
pub(crate) fn present<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// `{name, version}` of the engine (initialize).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
}

/// `initialize` params (protocol.md §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeParams {
    pub protocol: String,
    pub engine: EngineInfo,
    /// The absolute path of the folder holding fx.yaml.
    pub project_root: String,
    /// The project's declared source packages.
    pub sources: Vec<String>,
}

/// `{language, version, sdk_version}` of a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInfo {
    pub language: String,
    pub version: String,
    pub sdk_version: String,
}

/// `initialize` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeResult {
    pub protocol: String,
    pub host: HostInfo,
}

/// A node type as data (protocol.md §4), exactly as it crosses the wire. The engine validates it
/// into `grida_fx_core::spec::NodeSpec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Port notation by input name: `image`, `image[]?`, `text{}`.
    pub inputs: IndexMap<String, String>,
    /// A JSON Schema by param name.
    pub params: IndexMap<String, Value>,
    pub outputs: IndexMap<String, String>,
    pub judge: bool,
    /// `{capability: number or param name}`.
    pub calls: IndexMap<String, Value>,
    pub resources: Vec<String>,
    pub tools: Vec<String>,
    /// Optional in the schema; written as `null` when there is none.
    #[serde(default)]
    pub view: Option<String>,
    /// Null for an unversioned type. Required, even when null.
    #[serde(deserialize_with = "nullable")]
    pub version: Option<u64>,
    pub retry: RetryMode,
}

/// A type's retry mode (protocol.md §4 "Retry").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RetryMode {
    #[default]
    Service,
    Engine,
}

/// One describe target: a project module, and optionally one attribute of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribeTarget {
    /// POSIX, relative to the project root.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
}

/// `describe` params (protocol.md §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribeParams {
    pub targets: Vec<DescribeTarget>,
    pub builtins: bool,
}

/// A described node type of a module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribedType {
    pub attribute: String,
    pub spec: TypeSpec,
}

/// One file of a source closure: its label and the absolute path the engine reads (never
/// recorded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureEntry {
    pub label: String,
    pub path: String,
}

/// One entry of `modules`: the module's types and closure, or why it could not be described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ModuleDescription {
    Described {
        path: String,
        types: Vec<DescribedType>,
        closure: Vec<ClosureEntry>,
    },
    Failed {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attribute: Option<String>,
        error: String,
    },
}

/// A built-in type a host carries (`{uses: "fx/<name>@<major>", spec}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribedBuiltin {
    pub uses: String,
    pub spec: TypeSpec,
}

/// `describe` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribeResult {
    pub modules: Vec<ModuleDescription>,
    pub builtins: Vec<DescribedBuiltin>,
}

/// `build` params (protocol.md §5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildParams {
    pub path: String,
    pub function: String,
    pub arguments: IndexMap<String, String>,
    /// The engine's working directory, absolute.
    pub cwd: String,
}

/// `build` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildResult {
    /// The workflow document as authored (`fx: workflow/v1`), no defaults filled in.
    pub document: Value,
    /// Project-relative path of the module that constructed the `Workflow`.
    pub takes_anchor: String,
}

/// An error's `data` (fx-node-protocol-v1 `error_data`, protocol.md §2, §5.3, §6.1, §7). Every
/// member is optional; which ones appear depends on the code.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorData {
    /// `node_failure`, `node_error`: node facts the body kept locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<IndexMap<String, Value>>,
    /// `node_failure`, `node_error`: marks the body kept locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<Vec<crate::run_types::Mark>>,
    /// `node_error`: the exception's type in the host language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception: Option<String>,
    /// `node_error`: where it was raised, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// A route id, `model@provider`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// A call key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// `ceiling_exceeded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needed_usd: Option<f64>,
    /// `ceiling_exceeded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_usd: Option<f64>,
    /// `protocol_mismatch`: the protocol the engine speaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_protocol: Option<String>,
    /// `protocol_mismatch`: the protocol the host speaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_protocol: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec_value() -> Value {
        json!({
            "name": "caption",
            "inputs": {"image": "image"},
            "params": {"question": {"type": "string", "default": "Describe it."}},
            "outputs": {"caption": "text"},
            "judge": false,
            "calls": {"structured.generate": 1},
            "resources": [],
            "tools": [],
            "view": null,
            "version": 1,
            "retry": "service"
        })
    }

    #[test]
    fn type_spec_round_trip() {
        let spec: TypeSpec = serde_json::from_value(spec_value()).unwrap();
        assert_eq!(spec.version, Some(1));
        assert_eq!(spec.retry, RetryMode::Service);
        assert_eq!(spec.description, None);
        assert_eq!(serde_json::to_value(&spec).unwrap(), spec_value());
    }

    #[test]
    fn type_spec_version_is_required_and_nullable() {
        let mut value = spec_value();
        value["version"] = Value::Null;
        let spec: TypeSpec = serde_json::from_value(value).unwrap();
        assert_eq!(spec.version, None);
        let mut value = spec_value();
        value.as_object_mut().unwrap().shift_remove("version");
        assert!(serde_json::from_value::<TypeSpec>(value).is_err());
        let mut value = spec_value();
        value["version"] = json!(-1);
        assert!(serde_json::from_value::<TypeSpec>(value).is_err());
    }

    #[test]
    fn type_spec_view_may_be_left_out() {
        let mut value = spec_value();
        value.as_object_mut().unwrap().shift_remove("view");
        value["retry"] = json!("engine");
        value["description"] = json!("Captions a picture.");
        let spec: TypeSpec = serde_json::from_value(value).unwrap();
        assert_eq!(spec.view, None);
        assert_eq!(spec.retry, RetryMode::Engine);
        assert_eq!(spec.description.as_deref(), Some("Captions a picture."));
        let mut value = spec_value();
        value["retry"] = json!("never");
        assert!(serde_json::from_value::<TypeSpec>(value).is_err());
    }

    #[test]
    fn module_descriptions_are_one_shape_or_the_other() {
        let described: ModuleDescription = serde_json::from_value(json!({
            "path": "nodes/a.py",
            "types": [{"attribute": "a", "spec": spec_value()}],
            "closure": [{"label": "nodes/a.py", "path": "/p/nodes/a.py"}]
        }))
        .unwrap();
        assert!(matches!(described, ModuleDescription::Described { .. }));
        let failed: ModuleDescription = serde_json::from_value(json!({
            "path": "nodes/x.py",
            "attribute": "x",
            "error": "nodes/x.py failed to import: ModuleNotFoundError: No module named 'foo'"
        }))
        .unwrap();
        assert_eq!(
            failed,
            ModuleDescription::Failed {
                path: "nodes/x.py".into(),
                attribute: Some("x".into()),
                error: "nodes/x.py failed to import: ModuleNotFoundError: No module named 'foo'"
                    .into(),
            }
        );
        assert_eq!(
            serde_json::to_value(ModuleDescription::Failed {
                path: "nodes/x.py".into(),
                attribute: None,
                error: "e".into(),
            })
            .unwrap(),
            json!({"path": "nodes/x.py", "error": "e"})
        );
        // Both shapes at once is neither.
        assert!(
            serde_json::from_value::<ModuleDescription>(json!({
                "path": "nodes/a.py", "types": [], "closure": [], "error": "e"
            }))
            .is_err()
        );
    }

    #[test]
    fn unknown_members_are_refused() {
        assert!(
            serde_json::from_value::<InitializeResult>(json!({
                "protocol": "fx-node-protocol-v1",
                "host": {"language": "python", "version": "3.12", "sdk_version": "0.1", "x": 1}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<BuildResult>(json!({
                "document": {"fx": "workflow/v1"}, "takes_anchor": "a.py", "extra": true
            }))
            .is_err()
        );
        // A spec keeps what it knows of a richer object.
        let mut value = spec_value();
        value["uses"] = json!("fx/caption@1");
        assert!(serde_json::from_value::<TypeSpec>(value).is_ok());
    }

    #[test]
    fn error_data_reads_a_protocol_mismatch() {
        let data: ErrorData = serde_json::from_value(json!({
            "engine_protocol": "fx-node-protocol-v1",
            "host_protocol": "fx-node-protocol-v2"
        }))
        .unwrap();
        assert_eq!(data.host_protocol.as_deref(), Some("fx-node-protocol-v2"));
        assert_eq!(
            serde_json::to_value(ErrorData::default()).unwrap(),
            json!({})
        );
    }
}
