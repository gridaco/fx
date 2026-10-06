//! The `run` messages and the host-to-engine requests (protocol.md §3, §5.3–§5.6, §6). Step 3
//! (the runner) uses them; step 2 defines them so the schema is covered in one place.
//!
//! One serde type per `$defs` entry of fx-node-protocol-v1, field names as in the schema:
//! `file_ref`, `file_value`, `staged_input`, `output_value`, `port_output`, `mark`,
//! `instance`, `run_params`, `run_result`, `tool_invoke_params`/`result`,
//! `agent_check_params`/`result`, `cancel_params`, `capability_params`/`result`, `agent_tool`,
//! `agent_run_params`, `agent_message`, `agent_run_result`, `fact_params`/`result`,
//! `annotate_params`/`result`, `progress_params`, `prompt_render_params`/`result`,
//! `file_put_params`/`result`, and the stand-in's `stand_in_load_params`/`result`,
//! `stand_in_answer_params`/`result` and `stand_in_file` (protocol.md §5.7). The conventions are
//! [`crate::types`]': an optional member is an `Option` left out when `None`, a required member
//! that may be `null` must be present, a `oneOf` is an untagged enum, and unknown members are
//! refused where the schema says `additionalProperties: false`. JSON objects whose members the
//! schema leaves open are `IndexMap<String, Value>`, which keeps their order.

use crate::jsonrpc::Id;
use crate::types::{nullable, present};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A file the engine hands a host (protocol.md §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub digest: String,
    pub kind: String,
    pub size: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub path: String,
    pub facts: IndexMap<String, Value>,
}

/// A file inside a JSON value: `{"file": "<digest>"}` (protocol.md §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileValue {
    pub file: String,
}

/// An input port's value in `run` (protocol.md §3.3): one file, a list, or a keyed collection
/// in collection order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum StagedInput {
    One(FileRef),
    List { list: Vec<FileRef> },
    Collection { collection: Vec<(String, FileRef)> },
}

/// One output file a body returns (protocol.md §3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum OutputValue {
    /// A file the body wrote under `work_dir` (POSIX, relative); `kind` defaults to the suffix
    /// rule.
    Work {
        work_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    /// A file the engine handed this run, passed through by digest.
    File { file: FileRef },
}

/// An output port's value (protocol.md §3.4). `null` (no output) is the `None` of
/// [`RunResult::outputs`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum PortOutput {
    One(OutputValue),
    List {
        list: Vec<OutputValue>,
    },
    Collection {
        collection: Vec<(String, OutputValue)>,
    },
}

/// A mark's shape (protocol.md §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkShape {
    Point,
    Points,
    Box,
}

/// One annotation mark (protocol.md §6.4), in fractions of the image from 0 to 1. Fields other
/// than the named ones are kept as given in `extra`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Mark {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<MarkShape>,
    /// `point`: `[x, y]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// `points`: `[[x, y], …]`, at least two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f64; 2]>>,
    /// `box`: `[x0, y0, x1, y1]`.
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub box_: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Every other field, as given.
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

/// The instance a `run` is for (protocol.md §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunInstance {
    /// The instance id (identity.md §11), e.g. `draw#1`.
    pub id: String,
    /// The step path with repeat keys.
    pub path: String,
    /// The declared step path, without keys.
    pub step: String,
    /// The repeat key, or `null`.
    #[serde(deserialize_with = "nullable")]
    pub key: Option<String>,
    /// One take number per regenerating level, outermost first.
    pub take: Vec<u64>,
}

/// Which body a `run` runs: a project type's, or a built-in this host carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum RunBody {
    Project { path: String, attribute: String },
    Builtin { builtin: String },
}

/// `run` params (protocol.md §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunParams {
    pub run_id: String,
    pub instance: RunInstance,
    /// The type identity (identity.md §6).
    #[serde(rename = "type")]
    pub type_: String,
    pub body: RunBody,
    /// Rendered, defaults filled in; a missing value is `null`.
    pub params: IndexMap<String, Value>,
    /// Files inside `params`, by RFC 6901 pointer into `params`.
    pub param_files: IndexMap<String, FileRef>,
    pub inputs: IndexMap<String, StagedInput>,
    pub work_dir: String,
    /// `{declared path: absolute path}`.
    pub resources: IndexMap<String, String>,
    /// `{name: executable or null}`.
    pub tools: IndexMap<String, Option<String>>,
    /// `{capability: bound}`, resolved for this step.
    pub calls: IndexMap<String, u64>,
    #[serde(deserialize_with = "nullable")]
    pub timeout_s: Option<f64>,
}

/// `run` result (protocol.md §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunResult {
    /// Each output port's value; `None` is `null`, no output.
    pub outputs: IndexMap<String, Option<PortOutput>>,
    /// Node facts by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<IndexMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<Vec<Mark>>,
}

/// `tool.invoke` params (protocol.md §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInvokeParams {
    pub run_id: String,
    pub agent_id: String,
    /// The model's id for the tool call, or `null`.
    #[serde(deserialize_with = "nullable")]
    pub call_id: Option<String>,
    pub name: String,
    pub arguments: IndexMap<String, Value>,
}

/// `tool.invoke` result (protocol.md §5.4): the tool's answer, or the text of what it raised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ToolInvokeResult {
    Content {
        content: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        images: Option<Vec<FileValue>>,
    },
    Error {
        error: String,
    },
}

/// `agent.check` params (protocol.md §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCheckParams {
    pub run_id: String,
    pub agent_id: String,
    pub value: Value,
}

/// `agent.check` result: `{"refusal": null}` accepts the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCheckResult {
    #[serde(deserialize_with = "nullable")]
    pub refusal: Option<String>,
}

/// `$/cancel` params (protocol.md §5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelParams {
    pub id: Id,
}

/// `capability` params (protocol.md §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityParams {
    pub run_id: String,
    pub capability: String,
    /// The canonical request; files as file values.
    pub request: IndexMap<String, Value>,
}

/// `capability` result (protocol.md §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityResult {
    /// The call key.
    pub key: String,
    pub cached: bool,
    /// 0 on a hit; `null` when the provider reported no cost.
    #[serde(deserialize_with = "nullable")]
    pub cost_usd: Option<f64>,
    pub files: IndexMap<String, FileRef>,
    pub data: Value,
}

/// One of the body's agent tools (protocol.md §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTool {
    pub name: String,
    pub description: String,
    /// A JSON Schema.
    pub parameters: IndexMap<String, Value>,
}

/// `agent.run` params (protocol.md §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRunParams {
    pub run_id: String,
    pub agent_id: String,
    pub system: String,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<FileValue>>,
    pub tools: Vec<AgentTool>,
    pub max_steps: u64,
    /// The schema the answer must meet, or `null` for a text answer.
    #[serde(deserialize_with = "nullable")]
    pub submit: Option<IndexMap<String, Value>>,
    pub check: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_images: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// Who wrote a transcript message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    User,
    Assistant,
    Tool,
}

/// A tool call in an assistant message of the transcript (protocol.md §6.2 step 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentToolCall {
    /// The model's id: absent, `null` or a string, kept as given.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<Value>,
    pub name: String,
    /// The canonical JSON text (identity.md §2) of the object the model gave, as provider APIs
    /// carry it, so what the model wrote never reads as a file value or a reserved marker. The
    /// tool itself is dispatched with the object ([`ToolInvokeParams::arguments`]).
    pub arguments: String,
}

/// One transcript message (protocol.md §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessage {
    pub role: AgentRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<FileValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AgentToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// How an agent loop ended: with text (no `submit`), or with a submitted value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum AgentAnswer {
    Text { text: String },
    Submitted { submitted: Value },
}

/// `agent.run` result (protocol.md §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunResult {
    #[serde(flatten)]
    pub answer: AgentAnswer,
    /// Every message, unwindowed.
    pub transcript: Vec<AgentMessage>,
    pub turns: u64,
    /// The cost of the turns that were not cached.
    pub cost_usd: f64,
}

/// `fact` params (protocol.md §6.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactParams {
    pub run_id: String,
    pub name: String,
    pub value: Value,
}

/// The empty object `{}` that `fact` and `annotate` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyResult {}

/// `fact` result: `{}`.
pub type FactResult = EmptyResult;

/// `annotate` params (protocol.md §6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotateParams {
    pub run_id: String,
    pub mark: Mark,
}

/// `annotate` result: `{}`.
pub type AnnotateResult = EmptyResult;

/// `progress` params (protocol.md §6.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressParams {
    pub run_id: String,
    pub text: String,
    /// From 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
}

/// `prompt.render` params (protocol.md §6.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptRenderParams {
    pub run_id: String,
    /// One of the type's resources, exactly as declared.
    pub path: String,
    /// Files as file values.
    pub variables: IndexMap<String, Value>,
}

/// `prompt.render` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptRenderResult {
    pub text: String,
}

/// Where the bytes of a `file.put` come from: exactly one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum FilePutSource {
    /// A file under the run's `work_dir`, relative and POSIX.
    WorkPath { work_path: String },
    /// The bytes, base64 with padding.
    Base64 { base64: String },
    /// A JSON value, written as identity.md §5 "Writing JSON".
    Json { json: Value },
}

/// `file.put` params (protocol.md §6.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilePutParams {
    pub run_id: String,
    #[serde(flatten)]
    pub source: FilePutSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `file.put` result: the stored file's ref, with its file facts.
pub type FilePutResult = FileRef;

/// `stand_in.load` params (protocol.md §5.7): the engine asks a stand-in host to load one
/// stand-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandInLoadParams {
    /// The stand-in file's absolute path. Never recorded.
    pub path: String,
    /// The function's name in that file.
    pub function: String,
}

/// `stand_in.load` result: `{}`.
pub type StandInLoadResult = EmptyResult;

/// The route of a call a stand-in is asked to answer: `{id, fingerprint}` (identity.md §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandInRoute {
    pub id: String,
    pub fingerprint: String,
}

/// The instance making a call a stand-in is asked to answer: `{id, path, step}`, as in `run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandInInstance {
    pub id: String,
    pub path: String,
    pub step: String,
}

/// `stand_in.answer` params (protocol.md §5.7): one paid call for the stand-in to answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandInAnswerParams {
    pub capability: String,
    pub route: StandInRoute,
    /// The canonical request, exactly as keyed; files as file values.
    pub request: IndexMap<String, Value>,
    /// The call's take list, as in the call key.
    pub take: Vec<u64>,
    /// The call key.
    pub key: String,
    pub instance: StandInInstance,
    /// A file ref for every file value in `request`, by digest.
    pub files: IndexMap<String, FileRef>,
}

/// A file of a stand-in's answer, as the stand-in sends it: its bytes, base64 with padding, and
/// its kind unless the capability names one (`stand_in_file`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandInFileWire {
    pub base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// The `true` of a decline: reads only `true`, writes `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct True;

impl Serialize for True {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<True, D::Error> {
        match bool::deserialize(deserializer)? {
            true => Ok(True),
            false => Err(serde::de::Error::custom("a decline is true")),
        }
    }
}

/// `stand_in.answer` result (protocol.md §5.7): an answer, or a decline that leaves the call
/// `not_live`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum StandInAnswerResult {
    Answer {
        /// The answer's files by name.
        files: IndexMap<String, StandInFileWire>,
        /// JSON, or `null`.
        data: Value,
    },
    Decline {
        decline: True,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    fn file_ref() -> Value {
        json!({
            "digest": "ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859",
            "kind": "image/png",
            "size": 136,
            "name": "square.png",
            "path": "/work/acme/.fx/cache/files/ad/ad5e",
            "facts": {"bytes": 136, "kind": "image/png", "width": 64, "height": 64}
        })
    }

    /// Reads a value as `T` and writes it back unchanged.
    fn round_trip<T: DeserializeOwned + Serialize>(value: Value) -> T {
        let typed: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), value);
        typed
    }

    fn refused<T: DeserializeOwned + std::fmt::Debug>(value: Value) {
        let read = serde_json::from_value::<T>(value.clone());
        assert!(read.is_err(), "{value} read as {read:?}");
    }

    #[test]
    fn staged_inputs() {
        assert!(matches!(
            round_trip::<StagedInput>(file_ref()),
            StagedInput::One(_)
        ));
        assert!(matches!(
            round_trip::<StagedInput>(json!({"list": [file_ref(), file_ref()]})),
            StagedInput::List { list } if list.len() == 2
        ));
        let keyed = round_trip::<StagedInput>(json!({"collection": [["a", file_ref()]]}));
        assert!(matches!(keyed, StagedInput::Collection { collection } if collection[0].0 == "a"));
        refused::<StagedInput>(json!({"list": [], "collection": []}));
        refused::<StagedInput>(json!({"collection": [["a"]]}));
        let mut extra = file_ref();
        extra["x"] = json!(1);
        refused::<StagedInput>(extra);
    }

    #[test]
    fn output_values_and_ports() {
        let work =
            round_trip::<OutputValue>(json!({"work_path": "out/a.txt", "kind": "text/plain"}));
        assert!(matches!(work, OutputValue::Work { kind: Some(_), .. }));
        round_trip::<OutputValue>(json!({"work_path": "out/a.txt"}));
        round_trip::<OutputValue>(json!({"file": file_ref()}));
        refused::<OutputValue>(json!({"work_path": "a", "file": file_ref()}));
        round_trip::<PortOutput>(json!({"list": [{"work_path": "a.png"}, {"file": file_ref()}]}));
        round_trip::<PortOutput>(json!({"collection": [["k", {"work_path": "a.png"}]]}));
        let result = round_trip::<RunResult>(json!({
            "outputs": {"caption": {"work_path": "out/caption.txt", "kind": "text/plain"}, "extra": null},
            "facts": {"words": 7},
            "marks": [{"shape": "point", "at": [0.5, 0.25], "label": "eye", "score": 3}]
        }));
        assert_eq!(result.outputs["extra"], None);
        let mark = &result.marks.unwrap()[0];
        assert_eq!(mark.shape, Some(MarkShape::Point));
        assert_eq!(mark.extra["score"], json!(3));
        refused::<RunResult>(json!({"facts": {}}));
    }

    #[test]
    fn marks() {
        round_trip::<Mark>(json!({}));
        round_trip::<Mark>(json!({"shape": "box", "box": [0.0, 0.0, 0.5, 0.5], "color": "red"}));
        round_trip::<Mark>(
            json!({"shape": "points", "points": [[0.0, 0.0], [1.0, 1.0]], "closed": true, "tag": "t"}),
        );
        refused::<Mark>(json!({"shape": "circle"}));
        refused::<Mark>(json!({"shape": "box", "box": [0.0, 0.5]}));
    }

    #[test]
    fn run_params() {
        let params = round_trip::<RunParams>(json!({
            "run_id": "r1",
            "instance": {"id": "caption#1", "path": "caption", "step": "caption", "key": null, "take": [1]},
            "type": "nodes/caption.py#caption@1",
            "body": {"path": "nodes/caption.py", "attribute": "caption"},
            "params": {"question": "Describe it."},
            "param_files": {"/brief": file_ref()},
            "inputs": {"image": file_ref()},
            "work_dir": "/work/acme/.fx/cache/work/r1",
            "resources": {"prompts/a.md": "/work/acme/prompts/a.md"},
            "tools": {"blender": null, "git": "/usr/bin/git"},
            "calls": {"structured.generate": 1},
            "timeout_s": 2.5
        }));
        assert_eq!(params.type_, "nodes/caption.py#caption@1");
        assert_eq!(params.instance.key, None);
        assert_eq!(params.tools["blender"], None);
        assert_eq!(
            round_trip::<RunBody>(json!({"builtin": "fx/image.resize@1"})),
            RunBody::Builtin {
                builtin: "fx/image.resize@1".into()
            }
        );
        refused::<RunInstance>(json!({"id": "a#1", "path": "a", "step": "a", "take": [1]}));
        refused::<RunBody>(json!({"path": "a.py", "attribute": "a", "builtin": "fx/a@1"}));
    }

    #[test]
    fn tool_and_agent_messages() {
        let call = round_trip::<ToolInvokeParams>(json!({
            "run_id": "r1", "agent_id": "a1", "call_id": null, "name": "look", "arguments": {"x": 1}
        }));
        assert_eq!(call.call_id, None);
        refused::<ToolInvokeParams>(
            json!({"run_id": "r1", "agent_id": "a1", "name": "look", "arguments": {}}),
        );
        round_trip::<ToolInvokeResult>(json!({"content": {"seen": true}}));
        round_trip::<ToolInvokeResult>(json!({"content": null, "images": [{"file": "ab"}]}));
        assert!(matches!(
            round_trip::<ToolInvokeResult>(json!({"error": "ValueError: no"})),
            ToolInvokeResult::Error { .. }
        ));
        refused::<ToolInvokeResult>(json!({"content": 1, "error": "x"}));
        round_trip::<AgentCheckParams>(json!({"run_id": "r1", "agent_id": "a1", "value": null}));
        assert_eq!(
            round_trip::<AgentCheckResult>(json!({"refusal": null})).refusal,
            None
        );
        refused::<AgentCheckResult>(json!({}));
        round_trip::<AgentRunParams>(json!({
            "run_id": "r1", "agent_id": "a1", "system": "s", "instructions": "i",
            "tools": [{"name": "look", "description": "d", "parameters": {"type": "object"}}],
            "max_steps": 4, "submit": null, "check": false
        }));
        round_trip::<AgentRunParams>(json!({
            "run_id": "r1", "agent_id": "a1", "system": "s", "instructions": "i",
            "images": [{"file": "ab"}], "tools": [], "max_steps": 1,
            "submit": {"type": "object"}, "check": true, "recent_images": 2, "max_tokens": 100
        }));
        let messages = json!([
            {"role": "user", "content": "go"},
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": null, "name": "look", "arguments": "{}"},
                {"name": "look", "arguments": "{}"},
                {"id": "c1", "name": "submit", "arguments": "{\"a\":1}"}
            ]},
            {"role": "tool", "name": "look", "tool_call_id": "c1", "content": "x", "images": [{"file": "ab"}]}
        ]);
        let text = round_trip::<AgentRunResult>(json!({
            "text": "done", "transcript": messages, "turns": 2, "cost_usd": 0.5
        }));
        assert_eq!(
            text.answer,
            AgentAnswer::Text {
                text: "done".into()
            }
        );
        let calls = text.transcript[1].tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].id, Some(Value::Null));
        assert_eq!(calls[1].id, None);
        assert_eq!(calls[2].arguments, "{\"a\":1}");
        // A transcript's arguments are text: an object there is refused.
        refused::<AgentToolCall>(json!({"name": "look", "arguments": {"a": 1}}));
        let submitted = round_trip::<AgentRunResult>(json!({
            "submitted": null, "transcript": [], "turns": 1, "cost_usd": 0.0
        }));
        assert_eq!(
            submitted.answer,
            AgentAnswer::Submitted {
                submitted: Value::Null
            }
        );
        refused::<AgentRunResult>(
            json!({"text": "a", "submitted": 1, "transcript": [], "turns": 1, "cost_usd": 0}),
        );
        refused::<AgentRunResult>(json!({"transcript": [], "turns": 1, "cost_usd": 0}));
        refused::<AgentMessage>(json!({"role": "system", "content": "x"}));
    }

    #[test]
    fn host_requests() {
        round_trip::<CancelParams>(json!({"id": 3}));
        round_trip::<CancelParams>(json!({"id": "x"}));
        round_trip::<CapabilityParams>(json!({
            "run_id": "r1", "capability": "image.generate", "request": {"prompt": "p", "context": [{"file": "ab"}]}
        }));
        let result = round_trip::<CapabilityResult>(json!({
            "key": "cd", "cached": false, "cost_usd": null, "files": {"image": file_ref()}, "data": {}
        }));
        assert_eq!(result.cost_usd, None);
        refused::<CapabilityResult>(json!({"key": "cd", "cached": false, "files": {}, "data": {}}));
        round_trip::<FactParams>(json!({"run_id": "r1", "name": "words", "value": [1, "a"]}));
        round_trip::<FactResult>(json!({}));
        refused::<FactResult>(json!({"x": 1}));
        round_trip::<AnnotateParams>(json!({"run_id": "r1", "mark": {"label": "whole"}}));
        round_trip::<ProgressParams>(json!({"run_id": "r1", "text": "half", "fraction": 0.5}));
        round_trip::<ProgressParams>(json!({"run_id": "r1", "text": "busy"}));
        round_trip::<PromptRenderParams>(
            json!({"run_id": "r1", "path": "prompts/a.md", "variables": {"n": 1}}),
        );
        round_trip::<PromptRenderResult>(json!({"text": "hello"}));
        let put = round_trip::<FilePutParams>(
            json!({"run_id": "r1", "work_path": "out/a.png", "kind": "image/png"}),
        );
        assert!(matches!(put.source, FilePutSource::WorkPath { .. }));
        round_trip::<FilePutParams>(json!({"run_id": "r1", "base64": "AAE=", "name": "a.bin"}));
        let json_put = round_trip::<FilePutParams>(json!({"run_id": "r1", "json": {"a": [1, 2]}}));
        assert!(matches!(json_put.source, FilePutSource::Json { .. }));
        refused::<FilePutParams>(json!({"run_id": "r1"}));
        refused::<FilePutParams>(json!({"run_id": "r1", "base64": "AA==", "json": 1}));
        refused::<FilePutParams>(json!({"run_id": "r1", "base64": "AA==", "other": 1}));
        round_trip::<FilePutResult>(file_ref());
    }
}
