//! The protocol types against spec/schemas/fx-node-protocol-v1.schema.json and the example of
//! spec/protocol.md §9.
//!
//! Each `$defs` entry is validated on its own by wrapping it as
//! `{"$ref": "#/$defs/<name>", "$defs": …}`. Every type of the crate is serialized from a sample
//! value, validated against its entry and read back; every entry is either checked that way or
//! listed as a building block that the typed entries cover. The §9 exchange and its first frame
//! are read from protocol.md itself, so the test follows the document.

use grida_fx_protocol::framing::{read_message, write_message};
use grida_fx_protocol::run_types::*;
use grida_fx_protocol::*;
use indexmap::IndexMap;
use jsonschema::Validator;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Debug;
use std::path::PathBuf;

const DIGEST: &str = "ad5e9999a3966063951fa4baef73a311c378ae034778afe41d450d98876c5859";

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema() -> Value {
    let path = repository().join("spec/schemas/fx-node-protocol-v1.schema.json");
    let text = std::fs::read_to_string(&path).expect("the protocol schema");
    serde_json::from_str(&text).expect("the protocol schema is JSON")
}

fn protocol_md() -> String {
    std::fs::read_to_string(repository().join("spec/protocol.md")).expect("spec/protocol.md")
}

/// Validators for the schema's `$defs` entries, and which entries a test has checked.
struct Schema {
    defs: Value,
    validators: HashMap<String, Validator>,
    checked: BTreeSet<String>,
}

impl Schema {
    fn new() -> Schema {
        let schema = schema();
        Schema {
            defs: schema["$defs"].clone(),
            validators: HashMap::new(),
            checked: BTreeSet::new(),
        }
    }

    fn names(&self) -> Vec<String> {
        self.defs.as_object().unwrap().keys().cloned().collect()
    }

    fn validator(&mut self, def: &str) -> &Validator {
        assert!(
            self.defs.get(def).is_some(),
            "the schema has no $defs/{def}"
        );
        if !self.validators.contains_key(def) {
            let wrapped = json!({"$ref": format!("#/$defs/{def}"), "$defs": self.defs.clone()});
            let validator = jsonschema::draft202012::new(&wrapped)
                .unwrap_or_else(|e| panic!("$defs/{def} does not compile: {e}"));
            self.validators.insert(def.to_string(), validator);
        }
        &self.validators[def]
    }

    /// Asserts that `value` is valid against `$defs/<def>`.
    fn valid(&mut self, def: &str, value: &Value) {
        let validator = self.validator(def);
        let errors: Vec<String> = validator
            .iter_errors(value)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{value} is not a valid {def}: {errors:#?}"
        );
        self.checked.insert(def.to_string());
    }

    /// Asserts that `value` is invalid against `$defs/<def>`.
    fn invalid(&mut self, def: &str, value: &Value) {
        assert!(
            !self.validator(def).is_valid(value),
            "{value} should not be a valid {def}"
        );
    }

    /// Serializes a sample, validates it, and reads it back unchanged.
    fn typed<T>(&mut self, def: &str, sample: &T) -> Value
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let value = serde_json::to_value(sample).unwrap();
        self.valid(def, &value);
        let back: T = serde_json::from_value(value.clone())
            .unwrap_or_else(|e| panic!("{value} does not read back as {def}: {e}"));
        assert_eq!(&back, sample);
        value
    }

    /// A value both the schema and serde refuse.
    fn refused<T: DeserializeOwned + Debug>(&mut self, def: &str, value: Value) {
        self.invalid(def, &value);
        let read = serde_json::from_value::<T>(value.clone());
        assert!(read.is_err(), "serde read {value} as {read:?}");
    }
}

fn file_ref(key: Option<&str>) -> FileRef {
    FileRef {
        digest: DIGEST.into(),
        kind: "image/png".into(),
        size: 136,
        name: "square.png".into(),
        key: key.map(str::to_string),
        path: "/work/acme/.fx/cache/files/ad/ad5e".into(),
        facts: [
            ("bytes".to_string(), json!(136)),
            ("kind".to_string(), json!("image/png")),
            ("width".to_string(), json!(64)),
            ("height".to_string(), json!(64)),
            ("has_alpha".to_string(), json!(false)),
            ("opaque".to_string(), json!(true)),
        ]
        .into_iter()
        .collect(),
    }
}

fn map<V>(pairs: Vec<(&str, V)>) -> IndexMap<String, V> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn type_spec(version: Option<u64>) -> TypeSpec {
    TypeSpec {
        name: "caption.short".into(),
        description: Some("Captions a picture.".into()),
        inputs: map(vec![
            ("image", "image".into()),
            ("extra", "image/png[]?".into()),
        ]),
        params: map(vec![
            (
                "question",
                json!({"type": "string", "default": "Describe it.", "x-fx-template": true}),
            ),
            (
                "count",
                json!({"type": "integer", "minimum": 1, "maximum": 4}),
            ),
            ("note", json!({"type": "string", "x-fx-optional": true})),
        ]),
        outputs: map(vec![("caption", "text".into()), ("parts", "json{}".into())]),
        judge: false,
        calls: map(vec![
            ("structured.generate", json!(1)),
            ("image.generate", json!("count")),
        ]),
        resources: vec!["prompts/caption.md".into()],
        tools: vec!["blender>=4.2".into(), "ffprobe".into()],
        view: None,
        version,
        retry: RetryMode::Engine,
    }
}

fn mark() -> Mark {
    Mark {
        shape: Some(MarkShape::Box),
        box_: Some([0.0, 0.25, 0.5, 0.75]),
        label: Some("face".into()),
        color: Some("#ff0000".into()),
        extra: map(vec![("score", json!(0.9))]),
        ..Mark::default()
    }
}

/// Every `$defs` entry that has no Rust type of its own: building blocks of the typed entries.
const BUILDING_BLOCKS: [&str; 9] = [
    "digest",
    "kind",
    "facts",
    "data_marker",
    "plain_value",
    "file_json_value",
    "node_facts",
    "port",
    "param_schema",
];

#[test]
fn every_definition_compiles() {
    let mut schema = Schema::new();
    for name in schema.names() {
        schema.validator(&name);
    }
    jsonschema::draft202012::new(&self::schema()).expect("the whole schema compiles");
}

#[test]
fn error_codes_match_the_schema() {
    let defs = schema()["$defs"].clone();
    let listed: Vec<(i64, String)> = defs["error_code"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["const"].as_i64().unwrap(),
                c["title"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let ours: Vec<(i64, String)> = ErrorCode::ALL
        .iter()
        .map(|c| (c.code(), c.name().to_string()))
        .collect();
    assert_eq!(ours, listed);
}

#[test]
fn every_type_validates_against_its_definition() {
    let mut s = Schema::new();

    // Ids, errors, error data.
    s.typed("id", &Id::Number(7));
    s.typed("id", &Id::Text("r-7".into()));
    let data = ErrorData {
        facts: Some(map(vec![("score", json!(3))])),
        marks: Some(vec![mark()]),
        exception: Some("ValueError".into()),
        traceback: Some("Traceback (most recent call last): …".into()),
        capability: Some("image.generate".into()),
        route: Some("img-a@acme".into()),
        key: Some(DIGEST.into()),
        needed_usd: Some(0.5),
        remaining_usd: Some(0.25),
        engine_protocol: Some(PROTOCOL.into()),
        host_protocol: Some("fx-node-protocol-v2".into()),
    };
    let data_value = s.typed("error_data", &data);
    s.typed("error_data", &ErrorData::default());
    for code in ErrorCode::ALL {
        s.typed(
            "error",
            &RpcError::new(code, format!("{} happened", code.name())),
        );
    }
    s.typed(
        "error",
        &RpcError::new(ErrorCode::NodeError, "boom").with_data(data_value),
    );
    for code in ErrorCode::ALL {
        s.valid("error_code", &json!(code.code()));
    }
    s.invalid("error_code", &json!(-32603));

    // The session, describe and build.
    s.typed(
        "initialize_params",
        &InitializeParams {
            protocol: PROTOCOL.into(),
            engine: EngineInfo {
                name: "grida-fx".into(),
                version: "0.1.0-alpha.1".into(),
            },
            project_root: "/work/acme".into(),
            sources: vec!["acme_lib".into()],
        },
    );
    s.typed(
        "initialize_result",
        &InitializeResult {
            protocol: PROTOCOL.into(),
            host: HostInfo {
                language: "python".into(),
                version: "3.12.7".into(),
                sdk_version: "0.1.0a1".into(),
            },
        },
    );
    s.valid("shutdown_result", &Value::Null);
    s.typed("type_spec", &type_spec(Some(2)));
    s.typed("type_spec", &type_spec(None));
    s.typed(
        "describe_params",
        &DescribeParams {
            targets: vec![
                DescribeTarget {
                    path: "nodes/caption.py".into(),
                    attribute: Some("caption".into()),
                },
                DescribeTarget {
                    path: "nodes/all.py".into(),
                    attribute: None,
                },
            ],
            builtins: false,
        },
    );
    let closure = ClosureEntry {
        label: "nodes/caption.py".into(),
        path: "/work/acme/nodes/caption.py".into(),
    };
    s.typed("closure_entry", &closure);
    s.typed(
        "describe_result",
        &DescribeResult {
            modules: vec![
                ModuleDescription::Described {
                    path: "nodes/caption.py".into(),
                    types: vec![DescribedType {
                        attribute: "caption".into(),
                        spec: type_spec(Some(1)),
                    }],
                    closure: vec![closure.clone()],
                },
                ModuleDescription::Failed {
                    path: "nodes/x.py".into(),
                    attribute: Some("x".into()),
                    error:
                        "nodes/x.py failed to import: ModuleNotFoundError: No module named 'foo'"
                            .into(),
                },
                ModuleDescription::Failed {
                    path: "nodes/y.py".into(),
                    attribute: None,
                    error: "nodes/y.py failed to import: SyntaxError: invalid syntax".into(),
                },
            ],
            builtins: vec![DescribedBuiltin {
                uses: "fx/image.resize@1".into(),
                spec: TypeSpec {
                    name: "image.resize".into(),
                    ..type_spec(Some(1))
                },
            }],
        },
    );
    s.typed(
        "build_params",
        &BuildParams {
            path: "workflows/levels.py".into(),
            function: "build".into(),
            arguments: map(vec![("count", "3".to_string())]),
            cwd: "/work/acme".into(),
        },
    );
    s.typed(
        "build_result",
        &BuildResult {
            document: json!({"fx": "workflow/v1", "id": "levels", "title": "Levels", "steps": {}}),
            takes_anchor: "workflows/levels.py".into(),
        },
    );

    // Files and values.
    s.typed("file_ref", &file_ref(None));
    s.typed("file_ref", &file_ref(Some("a")));
    s.valid("facts", &json!(file_ref(None).facts));
    s.valid("digest", &json!(DIGEST));
    s.valid("kind", &json!("image/png"));
    s.valid("port", &json!("image/png[]?"));
    s.valid(
        "param_schema",
        &json!({"type": "string", "x-fx-template": true}),
    );
    s.valid("node_facts", &json!({"words": 7, "verdict": "accept"}));
    s.valid(
        "plain_value",
        &json!({"a": [1, {"file": "a.png"}, {"missing": false}]}),
    );
    s.valid("file_json_value", &json!({"context": [{"file": DIGEST}]}));
    s.valid("data_marker", &json!({"missing": true}));
    s.typed(
        "file_value",
        &FileValue {
            file: DIGEST.into(),
        },
    );
    s.typed("staged_input", &StagedInput::One(file_ref(None)));
    s.typed(
        "staged_input",
        &StagedInput::List {
            list: vec![file_ref(None), file_ref(None)],
        },
    );
    s.typed(
        "staged_input",
        &StagedInput::Collection {
            collection: vec![
                ("a".into(), file_ref(Some("a"))),
                ("b".into(), file_ref(Some("b"))),
            ],
        },
    );
    let work = OutputValue::Work {
        work_path: "out/caption.txt".into(),
        kind: Some("text/plain".into()),
    };
    s.typed("output_value", &work);
    s.typed(
        "output_value",
        &OutputValue::Work {
            work_path: "out/a.png".into(),
            kind: None,
        },
    );
    s.typed(
        "output_value",
        &OutputValue::File {
            file: file_ref(None),
        },
    );
    s.typed("port_output", &PortOutput::One(work.clone()));
    s.typed(
        "port_output",
        &PortOutput::List {
            list: vec![work.clone()],
        },
    );
    s.typed(
        "port_output",
        &PortOutput::Collection {
            collection: vec![("k".into(), work.clone())],
        },
    );
    s.typed("port_output", &None::<PortOutput>);
    s.typed("mark", &mark());
    s.typed("mark", &Mark::default());
    s.typed(
        "mark",
        &Mark {
            shape: Some(MarkShape::Point),
            at: Some([0.5, 0.5]),
            ..Mark::default()
        },
    );
    s.typed(
        "mark",
        &Mark {
            shape: Some(MarkShape::Points),
            points: Some(vec![[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
            closed: Some(true),
            tag: Some("outline".into()),
            ..Mark::default()
        },
    );

    // run.
    let instance = RunInstance {
        id: "draw#1".into(),
        path: "levels[a]/draw".into(),
        step: "levels/draw".into(),
        key: Some("a".into()),
        take: vec![1, 2],
    };
    s.typed("instance", &instance);
    s.typed(
        "instance",
        &RunInstance {
            key: None,
            take: vec![1],
            ..instance.clone()
        },
    );
    let run = RunParams {
        run_id: "r1".into(),
        instance,
        type_: "nodes/caption.py#caption@1".into(),
        body: RunBody::Project {
            path: "nodes/caption.py".into(),
            attribute: "caption".into(),
        },
        params: map(vec![
            ("question", json!("Describe it.")),
            ("brief", Value::Null),
        ]),
        param_files: map(vec![("/brief", file_ref(None))]),
        inputs: map(vec![
            ("image", StagedInput::One(file_ref(None))),
            (
                "refs",
                StagedInput::List {
                    list: vec![file_ref(None)],
                },
            ),
        ]),
        work_dir: "/work/acme/.fx/cache/work/r1".into(),
        resources: map(vec![(
            "prompts/caption.md",
            "/work/acme/prompts/caption.md".to_string(),
        )]),
        tools: map(vec![
            ("blender", None),
            ("git", Some("/usr/bin/git".to_string())),
        ]),
        calls: map(vec![("structured.generate", 1)]),
        timeout_s: Some(30.5),
    };
    s.typed("run_params", &run);
    s.typed(
        "run_params",
        &RunParams {
            body: RunBody::Builtin {
                builtin: "fx/image.resize@1".into(),
            },
            timeout_s: None,
            ..run.clone()
        },
    );
    s.typed(
        "run_result",
        &RunResult {
            outputs: map(vec![
                ("caption", Some(PortOutput::One(work.clone()))),
                ("debug", None),
            ]),
            facts: Some(map(vec![("words", json!(7))])),
            marks: Some(vec![mark()]),
        },
    );
    s.typed(
        "run_result",
        &RunResult {
            outputs: IndexMap::new(),
            facts: None,
            marks: None,
        },
    );

    // The engine's requests inside a run.
    s.typed(
        "tool_invoke_params",
        &ToolInvokeParams {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            call_id: Some("call_1".into()),
            name: "look".into(),
            arguments: map(vec![("x", json!(1))]),
        },
    );
    s.typed(
        "tool_invoke_params",
        &ToolInvokeParams {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            call_id: None,
            name: "look".into(),
            arguments: IndexMap::new(),
        },
    );
    s.typed(
        "tool_invoke_result",
        &ToolInvokeResult::Content {
            content: json!({"seen": "a cat"}),
            images: Some(vec![FileValue {
                file: DIGEST.into(),
            }]),
        },
    );
    s.typed(
        "tool_invoke_result",
        &ToolInvokeResult::Content {
            content: json!("plain"),
            images: None,
        },
    );
    s.typed(
        "tool_invoke_result",
        &ToolInvokeResult::Error {
            error: "ValueError: no such region".into(),
        },
    );
    s.typed(
        "agent_check_params",
        &AgentCheckParams {
            run_id: "r1".into(),
            agent_id: "a1".into(),
            value: json!({"answer": 4}),
        },
    );
    s.typed("agent_check_result", &AgentCheckResult { refusal: None });
    s.typed(
        "agent_check_result",
        &AgentCheckResult {
            refusal: Some("too small".into()),
        },
    );
    s.typed("cancel_params", &CancelParams { id: Id::Number(3) });

    // The host's requests.
    s.typed(
        "capability_params",
        &CapabilityParams {
            run_id: "r1".into(),
            capability: "structured.generate".into(),
            request: map(vec![
                ("prompt", json!("Describe it.")),
                ("context", json!([{"file": DIGEST}])),
            ]),
        },
    );
    let capability = CapabilityResult {
        key: DIGEST.into(),
        cached: false,
        cost_usd: Some(0.0012),
        files: map(vec![("image", file_ref(None))]),
        data: json!({"caption": "A red square."}),
    };
    s.typed("capability_result", &capability);
    s.typed(
        "capability_result",
        &CapabilityResult {
            cost_usd: None,
            cached: true,
            files: IndexMap::new(),
            data: Value::Null,
            ..capability
        },
    );
    let tool = AgentTool {
        name: "look".into(),
        description: "Look at a region.".into(),
        parameters: map(vec![("type", json!("object"))]),
    };
    s.typed("agent_tool", &tool);
    let agent = AgentRunParams {
        run_id: "r1".into(),
        agent_id: "a1".into(),
        system: "You place portraits.".into(),
        instructions: "Place it.".into(),
        images: Some(vec![FileValue {
            file: DIGEST.into(),
        }]),
        tools: vec![tool],
        max_steps: 8,
        submit: Some(map(vec![("type", json!("object"))])),
        check: true,
        recent_images: Some(2),
        max_tokens: Some(1024),
    };
    s.typed("agent_run_params", &agent);
    s.typed(
        "agent_run_params",
        &AgentRunParams {
            images: None,
            tools: Vec::new(),
            submit: None,
            check: false,
            recent_images: None,
            max_tokens: None,
            ..agent
        },
    );
    let transcript = vec![
        AgentMessage {
            role: AgentRole::User,
            content: "Place it.".into(),
            images: Some(vec![FileValue {
                file: DIGEST.into(),
            }]),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        },
        AgentMessage {
            role: AgentRole::Assistant,
            content: String::new(),
            images: None,
            tool_calls: Some(vec![
                AgentToolCall {
                    id: Some(json!("c1")),
                    name: "look".into(),
                    arguments: "{}".into(),
                },
                AgentToolCall {
                    id: Some(Value::Null),
                    name: "look".into(),
                    arguments: "{\"x\":0.5}".into(),
                },
                AgentToolCall {
                    id: None,
                    name: "submit".into(),
                    arguments: "{\"x\":1}".into(),
                },
            ]),
            name: None,
            tool_call_id: None,
        },
        AgentMessage {
            role: AgentRole::Tool,
            content: "a cat".into(),
            images: None,
            tool_calls: None,
            name: Some("look".into()),
            tool_call_id: Some("c1".into()),
        },
    ];
    for message in &transcript {
        s.typed("agent_message", message);
    }
    s.typed(
        "agent_run_result",
        &AgentRunResult {
            answer: AgentAnswer::Text {
                text: "done".into(),
            },
            transcript: transcript.clone(),
            turns: 2,
            cost_usd: 0.25,
        },
    );
    s.typed(
        "agent_run_result",
        &AgentRunResult {
            answer: AgentAnswer::Submitted {
                submitted: json!({"x": 1}),
            },
            transcript,
            turns: 3,
            cost_usd: 0.0,
        },
    );
    s.typed(
        "fact_params",
        &FactParams {
            run_id: "r1".into(),
            name: "words".into(),
            value: json!(7),
        },
    );
    s.typed("fact_result", &FactResult::default());
    s.typed(
        "annotate_params",
        &AnnotateParams {
            run_id: "r1".into(),
            mark: mark(),
        },
    );
    s.typed("annotate_result", &AnnotateResult::default());
    s.typed(
        "progress_params",
        &ProgressParams {
            run_id: "r1".into(),
            text: "half way".into(),
            fraction: Some(0.5),
        },
    );
    s.typed(
        "progress_params",
        &ProgressParams {
            run_id: "r1".into(),
            text: "busy".into(),
            fraction: None,
        },
    );
    s.typed(
        "prompt_render_params",
        &PromptRenderParams {
            run_id: "r1".into(),
            path: "prompts/caption.md".into(),
            variables: map(vec![
                ("subject", json!("a cat")),
                ("ref", json!({"file": DIGEST})),
            ]),
        },
    );
    s.typed(
        "prompt_render_result",
        &PromptRenderResult {
            text: "Describe a cat.".into(),
        },
    );
    for source in [
        FilePutSource::WorkPath {
            work_path: "out/a.png".into(),
        },
        FilePutSource::Base64 {
            base64: "iVBORw0KGgo=".into(),
        },
        FilePutSource::Json {
            json: json!({"a": [1, 2]}),
        },
    ] {
        s.typed(
            "file_put_params",
            &FilePutParams {
                run_id: "r1".into(),
                source: source.clone(),
                kind: None,
                name: None,
            },
        );
        s.typed(
            "file_put_params",
            &FilePutParams {
                run_id: "r1".into(),
                source,
                kind: Some("image/png".into()),
                name: Some("a.png".into()),
            },
        );
    }
    s.typed("file_put_result", &file_ref(None));

    // A stand-in's session (protocol.md §5.7).
    s.typed(
        "stand_in_load_params",
        &StandInLoadParams {
            path: "/work/acme/tests/stand_in.py".into(),
            function: "answer".into(),
        },
    );
    s.typed("stand_in_load_result", &StandInLoadResult::default());
    s.typed(
        "stand_in_answer_params",
        &StandInAnswerParams {
            capability: "image.edit".into(),
            route: StandInRoute {
                id: "img-a@acme".into(),
                fingerprint: DIGEST.into(),
            },
            request: map(vec![
                ("prompt", json!("a kite at dusk")),
                ("image", json!({"file": DIGEST})),
                ("size", json!("64x64")),
            ]),
            take: vec![2, 1],
            key: DIGEST.into(),
            instance: StandInInstance {
                id: "draw['a']#2.1".into(),
                path: "draw['a']".into(),
                step: "draw".into(),
            },
            files: map(vec![(DIGEST, file_ref(None))]),
        },
    );
    s.typed(
        "stand_in_file",
        &StandInFileWire {
            base64: "iVBORw0KGgo=".into(),
            kind: None,
        },
    );
    s.typed(
        "stand_in_file",
        &StandInFileWire {
            base64: "AAAA".into(),
            kind: Some("audio/mpeg".into()),
        },
    );
    s.typed(
        "stand_in_answer_result",
        &StandInAnswerResult::Answer {
            files: map(vec![(
                "image",
                StandInFileWire {
                    base64: "iVBORw0KGgo=".into(),
                    kind: Some("image/png".into()),
                },
            )]),
            data: Value::Null,
        },
    );
    s.typed(
        "stand_in_answer_result",
        &StandInAnswerResult::Answer {
            files: IndexMap::new(),
            data: json!({"json": {"title": "Kite"}}),
        },
    );
    s.typed(
        "stand_in_answer_result",
        &StandInAnswerResult::Decline { decline: True },
    );

    // A project adapter (protocol.md §5.8, §6.8).
    adapter_samples(&mut s);

    // Whole messages.
    messages(&mut s);

    let unchecked: Vec<String> = s
        .names()
        .into_iter()
        .filter(|name| !s.checked.contains(name))
        .collect();
    assert!(
        unchecked.is_empty(),
        "$defs entries no sample checked: {unchecked:?}"
    );
    for block in BUILDING_BLOCKS {
        assert!(s.defs.get(block).is_some(), "{block} is not in the schema");
    }
}

fn adapter_target() -> AdapterTarget {
    AdapterTarget {
        path: "adapters/acme.py".into(),
        attribute: "acme".into(),
    }
}

/// An `adapter.send` of an `image.edit` call with one input picture.
fn adapter_call() -> AdapterCallParams {
    AdapterCallParams {
        call_id: "c1".into(),
        adapter: adapter_target(),
        capability: "image.edit".into(),
        route: AdapterRoute {
            id: "img-a@acme".into(),
            model: "img-a".into(),
            provider: "acme".into(),
            contract: map(vec![
                ("adapter", json!("adapters/acme.py#acme")),
                ("adapter_behavior", json!(1)),
            ]),
        },
        base_url: "https://api.acme.test/v1".into(),
        request: map(vec![
            ("prompt", json!("a kite at dusk")),
            ("image", json!({"file": DIGEST})),
        ]),
        files: map(vec![(
            DIGEST,
            AdapterFile {
                digest: DIGEST.into(),
                kind: "image/png".into(),
                size: 136,
                path: "/work/acme/.fx/cache/files/ad/ad5e".into(),
            },
        )]),
        take: vec![2, 1],
        key: DIGEST.into(),
        attempt: 1,
    }
}

fn adapter_answer() -> AdapterAnswer {
    AdapterAnswer {
        files: map(vec![(
            "image",
            StandInFileWire {
                base64: "iVBORw0KGgo=".into(),
                kind: Some("image/png".into()),
            },
        )]),
        data: Value::Null,
        cost_usd: Some(0.04),
        usage: Some(AdapterUsage {
            input_tokens: Some(1200),
            cached_input_tokens: None,
            output_tokens: Some(4160),
        }),
    }
}

/// Every adapter definition: a typed sample of each, and each outcome against its own entry.
fn adapter_samples(s: &mut Schema) {
    s.typed("adapter_target", &adapter_target());
    s.typed("adapter_load_params", &adapter_target());
    let capabilities = map(vec![
        ("image.edit", CallKind::Request),
        ("video.generate", CallKind::Job),
    ]);
    for version in [Some(3), None] {
        s.typed(
            "adapter_load_result",
            &AdapterLoadResult {
                adapter: AdapterDeclaration {
                    version,
                    capabilities: capabilities.clone(),
                },
                closure: vec![ClosureEntry {
                    label: "adapters/acme.py".into(),
                    path: "/work/acme/adapters/acme.py".into(),
                }],
            },
        );
    }
    let call = adapter_call();
    s.typed("adapter_route", &call.route);
    s.typed("adapter_file", &call.files[DIGEST]);
    s.typed("adapter_send_params", &call);
    s.typed("adapter_submit_params", &call);
    s.typed(
        "adapter_collect_params",
        &AdapterCollectParams::new(call, map(vec![("job", json!("j-1"))])),
    );
    s.typed("adapter_usage", &AdapterUsage::default());
    s.typed("adapter_usage", &adapter_answer().usage.unwrap());

    let sends = [
        (
            "outcome_answered",
            AdapterSendResult::Answered(adapter_answer()),
        ),
        (
            "outcome_answered",
            AdapterSendResult::Answered(AdapterAnswer {
                files: IndexMap::new(),
                data: json!({"json": {"title": "Kite"}}),
                cost_usd: None,
                usage: None,
            }),
        ),
        (
            "outcome_not_received",
            AdapterSendResult::NotReceived {
                reason: "acme returned HTTP 429".into(),
                retry_after_s: Some(1.5),
            },
        ),
        (
            "outcome_refused",
            AdapterSendResult::Refused {
                reason: "prompt is blank".into(),
            },
        ),
        (
            "outcome_failed",
            AdapterSendResult::Failed {
                reason: "acme returned HTTP 500".into(),
                retryable: true,
                cost_usd: None,
            },
        ),
        (
            "outcome_failed",
            AdapterSendResult::Failed {
                reason: "acme returned HTTP 400: the size is not served".into(),
                retryable: false,
                cost_usd: Some(0.0),
            },
        ),
    ];
    for (outcome, result) in sends {
        let value = s.typed("adapter_send_result", &result);
        s.valid(outcome, &value);
    }
    let submits = [
        (
            "outcome_accepted",
            AdapterSubmitResult::Accepted {
                handle: map(vec![("job", json!("j-1")), ("frames", json!(48))]),
            },
        ),
        (
            "outcome_uncertain",
            AdapterSubmitResult::Uncertain {
                reason: "acme video submission timed out after the request left".into(),
            },
        ),
        (
            "outcome_not_received",
            AdapterSubmitResult::NotReceived {
                reason: "no connection".into(),
                retry_after_s: None,
            },
        ),
        (
            "outcome_refused",
            AdapterSubmitResult::Refused {
                reason: "a first frame is needed".into(),
            },
        ),
        (
            "outcome_failed",
            AdapterSubmitResult::Failed {
                reason: "acme returned HTTP 422".into(),
                retryable: false,
                cost_usd: Some(0.25),
            },
        ),
    ];
    for (outcome, result) in submits {
        let value = s.typed("adapter_submit_result", &result);
        s.valid(outcome, &value);
    }
    let collects = [
        (
            "outcome_answered",
            AdapterCollectResult::Answered(AdapterAnswer {
                files: map(vec![(
                    "video",
                    StandInFileWire {
                        base64: "AAAAIGZ0eXBpc29t".into(),
                        kind: None,
                    },
                )]),
                data: Value::Null,
                cost_usd: None,
                usage: None,
            }),
        ),
        (
            "outcome_ended",
            AdapterCollectResult::Ended {
                reason: "acme cancelled the job".into(),
                cost_usd: Some(0.0),
            },
        ),
        (
            "outcome_unreachable",
            AdapterCollectResult::Unreachable {
                reason: "still running at the collect deadline".into(),
            },
        ),
    ];
    for (outcome, result) in collects {
        let value = s.typed("adapter_collect_result", &result);
        s.valid(outcome, &value);
    }

    s.typed(
        "http_header",
        &("content-type".to_string(), "image/png".to_string()),
    );
    s.typed(
        "fetch_body",
        &FetchBody::Json {
            json: json!({"prompt": "a kite"}),
        },
    );
    s.typed(
        "fetch_body",
        &FetchBody::Bytes {
            base64: "iVBORw0KGgo=".into(),
            content_type: "image/png".into(),
        },
    );
    s.typed(
        "fetch_params",
        &FetchParams {
            call_id: "c1".into(),
            method: FetchMethod::Post,
            url: "https://api.acme.test/v1/edits".into(),
            headers: Some(vec![("accept".into(), "application/json".into())]),
            body: Some(FetchBody::Json {
                json: json!({"prompt": "a kite at dusk"}),
            }),
            lane: FetchLane::Provider,
            credential: Some(true),
            timeout_s: Some(120.0),
            max_response_bytes: Some(16 * 1024 * 1024),
        },
    );
    s.typed(
        "fetch_params",
        &FetchParams {
            call_id: "c1".into(),
            method: FetchMethod::Get,
            url: "https://files.acme.test/out/j-1.png".into(),
            headers: None,
            body: None,
            lane: FetchLane::Download,
            credential: None,
            timeout_s: None,
            max_response_bytes: None,
        },
    );
    s.typed(
        "fetch_result",
        &FetchResult::Response {
            status: 200,
            headers: vec![("content-type".into(), "image/png".into())],
            base64: "iVBORw0KGgo=".into(),
        },
    );
    for phase in [
        FetchPhase::Refused,
        FetchPhase::NotSent,
        FetchPhase::AfterSend,
    ] {
        s.typed(
            "fetch_result",
            &FetchResult::Failed {
                error: FetchFailure {
                    phase,
                    reason: "the network is off (GRIDA_FX_NETWORK=off)".into(),
                },
            },
        );
    }
}

/// Requests, notifications and responses as [`Message::to_value`] writes them, validated as
/// messages of their kind and against the root schema.
fn messages(s: &mut Schema) {
    let root = jsonschema::draft202012::new(&schema()).unwrap();
    let check = |s: &mut Schema, def: &str, message: Message| {
        let value = message.to_value();
        s.valid(def, &value);
        assert!(root.is_valid(&value), "{value} is not a protocol message");
        assert_eq!(Message::from_value(value).unwrap(), message);
    };
    let request = |id: i64, method: &str, params: Option<Value>| Message::Request {
        id: Id::Number(id),
        method: method.into(),
        params,
    };
    let initialize = serde_json::to_value(InitializeParams {
        protocol: PROTOCOL.into(),
        engine: EngineInfo {
            name: "grida-fx".into(),
            version: "0.1.0".into(),
        },
        project_root: "/work/acme".into(),
        sources: Vec::new(),
    })
    .unwrap();
    check(
        s,
        "request",
        request(1, method::INITIALIZE, Some(initialize)),
    );
    check(s, "request", request(2, method::SHUTDOWN, None));
    let describe = serde_json::to_value(DescribeParams {
        targets: vec![DescribeTarget {
            path: "nodes/caption.py".into(),
            attribute: None,
        }],
        builtins: false,
    })
    .unwrap();
    check(s, "request", request(3, method::DESCRIBE, Some(describe)));
    let fact = serde_json::to_value(FactParams {
        run_id: "r1".into(),
        name: "words".into(),
        value: json!(7),
    })
    .unwrap();
    check(s, "request", request(4, method::FACT, Some(fact)));
    let load = serde_json::to_value(StandInLoadParams {
        path: "/work/acme/stand_in.py".into(),
        function: "answer".into(),
    })
    .unwrap();
    check(s, "request", request(5, method::STAND_IN_LOAD, Some(load)));
    let answer = serde_json::to_value(StandInAnswerParams {
        capability: "image.generate".into(),
        route: StandInRoute {
            id: "img-a@acme".into(),
            fingerprint: DIGEST.into(),
        },
        request: map(vec![("prompt", json!("a kite"))]),
        take: vec![1],
        key: DIGEST.into(),
        instance: StandInInstance {
            id: "draw#1".into(),
            path: "draw".into(),
            step: "draw".into(),
        },
        files: IndexMap::new(),
    })
    .unwrap();
    check(
        s,
        "request",
        request(6, method::STAND_IN_ANSWER, Some(answer)),
    );
    let load = serde_json::to_value(adapter_target()).unwrap();
    check(s, "request", request(7, method::ADAPTER_LOAD, Some(load)));
    let call = adapter_call();
    let send = serde_json::to_value(&call).unwrap();
    check(
        s,
        "request",
        request(8, method::ADAPTER_SEND, Some(send.clone())),
    );
    check(s, "request", request(9, method::ADAPTER_SUBMIT, Some(send)));
    let collect = serde_json::to_value(AdapterCollectParams::new(
        call,
        map(vec![("job", json!("j-1"))]),
    ))
    .unwrap();
    check(
        s,
        "request",
        request(10, method::ADAPTER_COLLECT, Some(collect)),
    );
    let fetch = serde_json::to_value(FetchParams {
        call_id: "c1".into(),
        method: FetchMethod::Get,
        url: "https://api.acme.test/v1/jobs/j-1".into(),
        headers: None,
        body: None,
        lane: FetchLane::Provider,
        credential: Some(true),
        timeout_s: None,
        max_response_bytes: None,
    })
    .unwrap();
    check(s, "request", request(11, method::FETCH, Some(fetch)));
    check(
        s,
        "notification",
        Message::Notification {
            method: method::EXIT.into(),
            params: None,
        },
    );
    check(
        s,
        "notification",
        Message::Notification {
            method: method::CANCEL.into(),
            params: Some(serde_json::to_value(CancelParams { id: Id::Number(3) }).unwrap()),
        },
    );
    check(
        s,
        "response",
        Message::Response {
            id: Id::Number(4),
            result: Value::Null,
        },
    );
    check(
        s,
        "error_response",
        Message::Error {
            id: Some(Id::Number(1)),
            error: RpcError::new(ErrorCode::ProtocolMismatch, "the protocols differ").with_data(
                json!({"engine_protocol": PROTOCOL, "host_protocol": "fx-node-protocol-v2"}),
            ),
        },
    );
    check(
        s,
        "error_response",
        Message::Error {
            id: None,
            error: RpcError::new(ErrorCode::ParseError, "not JSON"),
        },
    );
    // A request whose params do not fit its method is not a protocol message.
    s.invalid(
        "request",
        &json!({"jsonrpc": "2.0", "id": 1, "method": "describe", "params": {"targets": []}}),
    );
    s.invalid(
        "notification",
        &json!({"jsonrpc": "2.0", "method": "progress"}),
    );
    // A fetch names its call, never a run; a collect carries its handle.
    s.invalid(
        "request",
        &json!({"jsonrpc": "2.0", "id": 1, "method": "fetch",
                "params": {"run_id": "r1", "method": "GET", "url": "u", "lane": "provider"}}),
    );
    let mut send = serde_json::to_value(adapter_call()).unwrap();
    s.invalid(
        "request",
        &json!({"jsonrpc": "2.0", "id": 1, "method": "adapter.collect", "params": send.clone()}),
    );
    send["handle"] = json!({"job": "j-1"});
    s.invalid(
        "request",
        &json!({"jsonrpc": "2.0", "id": 1, "method": "adapter.send", "params": send}),
    );
}

#[test]
fn schema_and_serde_refuse_the_same_shapes() {
    let mut s = Schema::new();
    let mut spec = serde_json::to_value(type_spec(Some(1))).unwrap();
    spec.as_object_mut().unwrap().shift_remove("version");
    s.refused::<TypeSpec>("type_spec", spec);
    let mut spec = serde_json::to_value(type_spec(Some(1))).unwrap();
    spec["retry"] = json!("always");
    s.refused::<TypeSpec>("type_spec", spec);
    let mut spec = serde_json::to_value(type_spec(Some(1))).unwrap();
    spec["version"] = json!(-1);
    s.refused::<TypeSpec>("type_spec", spec);
    s.refused::<DescribeResult>(
        "describe_result",
        json!({"modules": [{"path": "a.py", "types": [], "closure": [], "error": "e"}], "builtins": []}),
    );
    s.refused::<DescribeResult>("describe_result", json!({"modules": []}));
    s.refused::<InitializeResult>(
        "initialize_result",
        json!({"protocol": PROTOCOL, "host": {"language": "python", "version": "3"}}),
    );
    s.refused::<BuildParams>(
        "build_params",
        json!({"path": "a.py", "function": "build", "arguments": {"n": 3}, "cwd": "/w"}),
    );
    s.refused::<RunInstance>(
        "instance",
        json!({"id": "a#1", "path": "a", "step": "a", "take": [1]}),
    );
    let mut extra = serde_json::to_value(file_ref(None)).unwrap();
    extra["mtime"] = json!(1);
    s.refused::<StagedInput>("staged_input", extra);
    s.refused::<OutputValue>(
        "output_value",
        json!({"work_path": "a.txt", "file": serde_json::to_value(file_ref(None)).unwrap()}),
    );
    s.refused::<FilePutParams>("file_put_params", json!({"run_id": "r1"}));
    s.refused::<FilePutParams>(
        "file_put_params",
        json!({"run_id": "r1", "base64": "AA==", "json": 1}),
    );
    s.refused::<AgentRunResult>(
        "agent_run_result",
        json!({"text": "a", "submitted": 1, "transcript": [], "turns": 1, "cost_usd": 0}),
    );
    s.refused::<AgentRunResult>(
        "agent_run_result",
        json!({"transcript": [], "turns": 1, "cost_usd": 0}),
    );
    // A transcript's tool call carries its arguments as text, never as the object.
    s.refused::<AgentMessage>(
        "agent_message",
        json!({"role": "assistant", "content": "",
               "tool_calls": [{"name": "look", "arguments": {"x": 1}}]}),
    );
    s.refused::<AgentCheckResult>("agent_check_result", json!({}));
    s.refused::<ToolInvokeResult>("tool_invoke_result", json!({"content": 1, "error": "e"}));
    s.refused::<CapabilityResult>(
        "capability_result",
        json!({"key": DIGEST, "cached": true, "files": {}, "data": null}),
    );
    s.refused::<Mark>("mark", json!({"shape": "box", "box": [0, 0]}));
    s.refused::<FactResult>("fact_result", json!({"x": 1}));
    s.refused::<StandInAnswerResult>("stand_in_answer_result", json!({"decline": false}));
    s.refused::<StandInAnswerResult>("stand_in_answer_result", json!({"files": {}}));
    s.refused::<StandInAnswerResult>(
        "stand_in_answer_result",
        json!({"files": {}, "data": null, "decline": true}),
    );
    s.refused::<StandInAnswerResult>(
        "stand_in_answer_result",
        json!({"files": {"image": {"kind": "image/png"}}, "data": null}),
    );
    s.refused::<StandInLoadParams>("stand_in_load_params", json!({"path": "/w/s.py"}));

    // A project adapter's messages.
    s.refused::<AdapterLoadParams>("adapter_load_params", json!({"path": "adapters/acme.py"}));
    s.refused::<AdapterLoadResult>(
        "adapter_load_result",
        json!({"adapter": {"capabilities": {"image.generate": "request"}}, "closure": []}),
    );
    s.refused::<AdapterLoadResult>(
        "adapter_load_result",
        json!({"adapter": {"version": 1, "capabilities": {"image.generate": "stream"}}, "closure": []}),
    );
    s.refused::<AdapterLoadResult>(
        "adapter_load_result",
        json!({"adapter": {"version": -1, "capabilities": {}}, "closure": []}),
    );
    let mut send = serde_json::to_value(adapter_call()).unwrap();
    send.as_object_mut().unwrap().shift_remove("base_url");
    s.refused::<AdapterCallParams>("adapter_send_params", send);
    let mut collect = serde_json::to_value(adapter_call()).unwrap();
    collect["handle"] = json!("j-1");
    s.refused::<AdapterCollectParams>("adapter_collect_params", collect);
    s.refused::<AdapterSendResult>("adapter_send_result", json!({"files": {}, "data": null}));
    s.refused::<AdapterSendResult>(
        "adapter_send_result",
        json!({"outcome": "answered", "files": {}}),
    );
    s.refused::<AdapterSendResult>(
        "adapter_send_result",
        json!({"outcome": "accepted", "handle": {"job": "j-1"}}),
    );
    s.refused::<AdapterSendResult>(
        "adapter_send_result",
        json!({"outcome": "refused", "reason": "no", "cost_usd": 0.5}),
    );
    s.refused::<AdapterSendResult>(
        "adapter_send_result",
        json!({"outcome": "failed", "reason": "no"}),
    );
    s.refused::<AdapterSendResult>(
        "adapter_send_result",
        json!({"outcome": "answered", "files": {"image": {"kind": "image/png"}}, "data": null}),
    );
    s.refused::<AdapterSubmitResult>(
        "adapter_submit_result",
        json!({"outcome": "accepted", "handle": "j-1"}),
    );
    s.refused::<AdapterSubmitResult>(
        "adapter_submit_result",
        json!({"outcome": "answered", "files": {}, "data": null}),
    );
    s.refused::<AdapterCollectResult>(
        "adapter_collect_result",
        json!({"outcome": "uncertain", "reason": "no"}),
    );
    s.refused::<AdapterCollectResult>("adapter_collect_result", json!({"outcome": "ended"}));
    s.refused::<FetchParams>(
        "fetch_params",
        json!({"call_id": "c1", "method": "PUT", "url": "u", "lane": "provider"}),
    );
    s.refused::<FetchParams>(
        "fetch_params",
        json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "cdn"}),
    );
    s.refused::<FetchParams>(
        "fetch_params",
        json!({"run_id": "r1", "method": "GET", "url": "u", "lane": "provider"}),
    );
    s.refused::<FetchParams>(
        "fetch_params",
        json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "provider", "headers": [["accept"]]}),
    );
    s.refused::<FetchParams>(
        "fetch_params",
        json!({"call_id": "c1", "method": "POST", "url": "u", "lane": "provider",
               "body": {"json": {}, "base64": "AA=="}}),
    );
    s.refused::<FetchResult>(
        "fetch_result",
        json!({"error": {"phase": "lost", "reason": "no"}}),
    );
    s.refused::<FetchResult>(
        "fetch_result",
        json!({"status": 200, "headers": [], "base64": "", "error": {"phase": "refused", "reason": "no"}}),
    );
    // What only the schema says, and the engine checks when it reads the message: a body on a GET,
    // the caps of a fetch, a blank reason, a cost below 0, an attempt past 6, a header named in
    // capitals.
    s.invalid(
        "fetch_params",
        &json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "provider", "body": {"json": {}}}),
    );
    s.invalid(
        "fetch_params",
        &json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "provider", "timeout_s": 301}),
    );
    s.invalid(
        "fetch_params",
        &json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "provider", "timeout_s": 0}),
    );
    s.invalid(
        "fetch_params",
        &json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "provider",
                "max_response_bytes": 67_108_865}),
    );
    s.invalid(
        "fetch_params",
        &json!({"call_id": "c1", "method": "GET", "url": "u", "lane": "provider",
                "headers": [["Accept", "image/png"]]}),
    );
    s.invalid(
        "adapter_send_result",
        &json!({"outcome": "refused", "reason": ""}),
    );
    s.invalid(
        "adapter_send_result",
        &json!({"outcome": "failed", "reason": "no", "retryable": false, "cost_usd": -1}),
    );
    let mut seventh = serde_json::to_value(adapter_call()).unwrap();
    seventh["attempt"] = json!(7);
    s.invalid("adapter_send_params", &seventh);
}

/// The fenced block of protocol.md §9 that starts with ```` ```jsonc ````.
fn example_exchange() -> Vec<(String, Value)> {
    let text = protocol_md();
    let section = &text[text.find("## 9. Example").expect("protocol.md §9")..];
    let start = section.find("```jsonc\n").expect("the §9 exchange") + "```jsonc\n".len();
    let block = &section[start..start + section[start..].find("```").unwrap()];
    let mut messages = Vec::new();
    let mut direction = String::new();
    let mut current = String::new();
    let flush = |direction: &str, current: &mut String, messages: &mut Vec<(String, Value)>| {
        if !current.trim().is_empty() {
            let value: Value = serde_json::from_str(current).expect("a §9 message is JSON");
            messages.push((direction.to_string(), value));
        }
        current.clear();
    };
    for line in block.lines() {
        if let Some(comment) = line.trim().strip_prefix("//") {
            flush(&direction, &mut current, &mut messages);
            direction = comment.trim().to_string();
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    flush(&direction, &mut current, &mut messages);
    messages
}

/// Reads `value` as `T`, checks it against `$defs/<def>`, and checks it writes back unchanged.
fn exact<T>(s: &mut Schema, def: &str, value: &Value) -> T
where
    T: Serialize + DeserializeOwned,
{
    s.valid(def, value);
    let typed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{value} does not read as {def}: {e}"));
    assert_eq!(&serde_json::to_value(&typed).unwrap(), value, "{def}");
    typed
}

#[test]
fn the_example_exchange_reads() {
    let mut s = Schema::new();
    let root = jsonschema::draft202012::new(&schema()).unwrap();
    let exchange = example_exchange();
    assert_eq!(exchange.len(), 13);
    // The method of each pending request, by the sender's direction and id.
    let mut pending: HashMap<(String, Id), String> = HashMap::new();
    let mut methods = Vec::new();
    for (direction, value) in &exchange {
        assert!(
            direction == "engine → host" || direction == "host → engine",
            "{direction}"
        );
        assert!(root.is_valid(value), "{value} is not a protocol message");
        let message = Message::from_value(value.clone()).unwrap();
        assert_eq!(&message.to_value(), value);
        let other = if direction == "engine → host" {
            "host → engine"
        } else {
            "engine → host"
        };
        match &message {
            Message::Request { id, method, params } => {
                pending.insert((direction.clone(), id.clone()), method.clone());
                methods.push(format!("{direction} {method} {id}"));
                let params = params.clone().unwrap_or(Value::Null);
                match method.as_str() {
                    method::INITIALIZE => {
                        let p: InitializeParams = exact(&mut s, "initialize_params", &params);
                        assert_eq!(p.protocol, PROTOCOL);
                        assert_eq!(p.engine.name, "grida-fx");
                    }
                    method::DESCRIBE => {
                        let p: DescribeParams = exact(&mut s, "describe_params", &params);
                        assert_eq!(p.targets[0].attribute.as_deref(), Some("caption"));
                    }
                    method::RUN => {
                        let p: RunParams = exact(&mut s, "run_params", &params);
                        assert_eq!(p.instance.take, vec![1]);
                        assert!(matches!(p.inputs["image"], StagedInput::One(_)));
                    }
                    method::CAPABILITY => {
                        exact::<CapabilityParams>(&mut s, "capability_params", &params);
                    }
                    method::FACT => {
                        let p: FactParams = exact(&mut s, "fact_params", &params);
                        assert_eq!(p.value, json!(7));
                    }
                    method::SHUTDOWN => assert_eq!(params, Value::Null),
                    other => panic!("unexpected request {other}"),
                }
            }
            Message::Notification { method, .. } => {
                methods.push(format!("{direction} {method}"));
                assert_eq!(method, method::EXIT);
            }
            Message::Response { id, result } => {
                let method = pending
                    .remove(&(other.to_string(), id.clone()))
                    .unwrap_or_else(|| panic!("{direction} answers {id}, which is not pending"));
                methods.push(format!("{direction} answers {method} {id}"));
                match method.as_str() {
                    method::INITIALIZE => {
                        let r: InitializeResult = exact(&mut s, "initialize_result", result);
                        assert_eq!(r.host.language, "python");
                    }
                    method::DESCRIBE => {
                        let r: DescribeResult = exact(&mut s, "describe_result", result);
                        let ModuleDescription::Described { types, closure, .. } = &r.modules[0]
                        else {
                            panic!("the module is described");
                        };
                        assert_eq!(types[0].spec.version, Some(1));
                        assert_eq!(types[0].spec.retry, RetryMode::Service);
                        assert_eq!(closure[0].label, "nodes/caption.py");
                    }
                    method::RUN => {
                        let r: RunResult = exact(&mut s, "run_result", result);
                        assert!(r.outputs["caption"].is_some());
                    }
                    method::CAPABILITY => {
                        let r: CapabilityResult = exact(&mut s, "capability_result", result);
                        assert_eq!(r.cost_usd, Some(0.0012));
                    }
                    method::FACT => {
                        exact::<FactResult>(&mut s, "fact_result", result);
                    }
                    method::SHUTDOWN => {
                        s.valid("shutdown_result", result);
                    }
                    other => panic!("unexpected response to {other}"),
                }
            }
            Message::Error { .. } => panic!("the example has no error"),
        }
    }
    assert!(pending.is_empty(), "unanswered: {pending:?}");
    assert_eq!(
        methods,
        [
            "engine → host initialize 1",
            "host → engine answers initialize 1",
            "engine → host describe 2",
            "host → engine answers describe 2",
            "engine → host run 3",
            "host → engine capability 1",
            "engine → host answers capability 1",
            "host → engine fact 2",
            "engine → host answers fact 2",
            "host → engine answers run 3",
            "engine → host shutdown 4",
            "host → engine answers shutdown 4",
            "engine → host exit",
        ]
    );
}

#[test]
fn the_first_frame_is_178_bytes() {
    let text = protocol_md();
    let at = text
        .find("Content-Length: 178\\r\\n")
        .expect("the first frame in §9");
    let block = &text[at..at + text[at..].find("\n```").unwrap()];
    let mut lines = block.lines();
    let mut expected = Vec::new();
    for _ in 0..2 {
        let line = lines.next().unwrap();
        expected.extend_from_slice(line.strip_suffix("\\r\\n").unwrap().as_bytes());
        expected.extend_from_slice(b"\r\n");
    }
    let body_line = lines.next().unwrap();
    expected.extend_from_slice(body_line.as_bytes());
    assert!(lines.next().is_none());

    let params = InitializeParams {
        protocol: PROTOCOL.into(),
        engine: EngineInfo {
            name: "grida-fx".into(),
            version: "0.1.0".into(),
        },
        project_root: "/work/acme".into(),
        sources: Vec::new(),
    };
    let message = Message::Request {
        id: Id::Number(1),
        method: method::INITIALIZE.into(),
        params: Some(serde_json::to_value(&params).unwrap()),
    };
    let body = serde_json::to_vec(&message.to_value()).unwrap();
    assert_eq!(body.len(), 178);
    let mut frame = Vec::new();
    write_message(&mut frame, &body).unwrap();
    assert_eq!(
        String::from_utf8(frame.clone()).unwrap(),
        String::from_utf8(expected).unwrap()
    );
    let read = read_message(&mut std::io::Cursor::new(frame))
        .unwrap()
        .unwrap();
    assert_eq!(read, body);
}
