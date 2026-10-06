//! The agent loop (spec/protocol.md §6.2) in isolation: a scripted `TurnCaller` plays the model's
//! replies and records every turn's request, and a fake `AgentBody` plays the body's tools and
//! check. Nothing here needs the store, the ledger or a host.

use grida_fx_core::money::Usd;
use grida_fx_protocol::{
    AgentAnswer, AgentRunParams, AgentTool, ErrorCode, FileValue, RpcError, ToolInvokeResult,
};
use grida_fx_providers::BoxFuture;
use grida_fx_runtime::agent::{AgentBody, AgentError, TurnCaller, run_agent};
use grida_fx_runtime::calls::{CallAnswer, CallError};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Plays the model: one scripted answer per turn, every request kept.
struct Model {
    replies: Mutex<VecDeque<Result<CallAnswer, CallError>>>,
    requests: Mutex<Vec<Value>>,
}

impl Model {
    fn new(replies: Vec<Result<CallAnswer, CallError>>) -> Model {
        Model {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

impl TurnCaller for Model {
    fn turn(&self, request: Value) -> BoxFuture<'_, Result<CallAnswer, CallError>> {
        self.requests.lock().unwrap().push(request);
        let next = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(CallError::Store(
                "the model's script ran out (a test sent too many turns)".into(),
            ))
        });
        Box::pin(async move { next })
    }
}

/// An uncached reply charged `micros`.
fn reply(data: Value, micros: i64) -> Result<CallAnswer, CallError> {
    Ok(CallAnswer {
        key: format!("{:064x}", micros),
        cached: false,
        cost: Some(Usd(micros)),
        charged: Usd(micros),
        files: IndexMap::new(),
        data,
    })
}

fn cached(data: Value) -> Result<CallAnswer, CallError> {
    Ok(CallAnswer {
        key: "k".into(),
        cached: true,
        cost: Some(Usd::ZERO),
        charged: Usd::ZERO,
        files: IndexMap::new(),
        data,
    })
}

type ToolFn =
    dyn Fn(&str, &IndexMap<String, Value>) -> Result<ToolInvokeResult, RpcError> + Send + Sync;

/// One `tool.invoke`: the agent id, the call id, the tool's name and its arguments.
type Invoked = (String, Option<String>, String, Value);

/// Plays the body: its tools by a function, its check by a script; every call kept.
struct Body {
    tool: Box<ToolFn>,
    checks: Mutex<VecDeque<Result<Option<String>, RpcError>>>,
    invoked: Mutex<Vec<Invoked>>,
    checked: Mutex<Vec<(String, Value)>>,
}

impl Body {
    fn new(
        tool: impl Fn(&str, &IndexMap<String, Value>) -> Result<ToolInvokeResult, RpcError>
        + Send
        + Sync
        + 'static,
        checks: Vec<Result<Option<String>, RpcError>>,
    ) -> Body {
        Body {
            tool: Box::new(tool),
            checks: Mutex::new(checks.into()),
            invoked: Mutex::new(Vec::new()),
            checked: Mutex::new(Vec::new()),
        }
    }

    fn quiet() -> Body {
        Body::new(|_, _| panic!("no tool should run"), Vec::new())
    }

    fn invoked(&self) -> Vec<Invoked> {
        self.invoked.lock().unwrap().clone()
    }
}

impl AgentBody for Body {
    fn invoke<'a>(
        &'a self,
        agent_id: &'a str,
        call_id: Option<&'a str>,
        name: &'a str,
        arguments: &'a IndexMap<String, Value>,
    ) -> BoxFuture<'a, Result<ToolInvokeResult, RpcError>> {
        self.invoked.lock().unwrap().push((
            agent_id.to_string(),
            call_id.map(str::to_string),
            name.to_string(),
            Value::Object(arguments.clone().into_iter().collect()),
        ));
        let result = (self.tool)(name, arguments);
        Box::pin(async move { result })
    }

    fn check<'a>(
        &'a self,
        agent_id: &'a str,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<Option<String>, RpcError>> {
        self.checked
            .lock()
            .unwrap()
            .push((agent_id.to_string(), value.clone()));
        let next = self
            .checks
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted check");
        Box::pin(async move { next })
    }
}

fn look_tool() -> AgentTool {
    AgentTool {
        name: "look".into(),
        description: "Look at something.".into(),
        parameters: object(json!({
            "type": "object",
            "properties": {"what": {"type": "string"}},
            "required": ["what"]
        })),
    }
}

fn object(value: Value) -> IndexMap<String, Value> {
    match value {
        Value::Object(map) => map.into_iter().collect(),
        other => panic!("{other} is not an object"),
    }
}

fn answer_schema() -> IndexMap<String, Value> {
    object(json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"]
    }))
}

fn params(tools: Vec<AgentTool>, submit: Option<IndexMap<String, Value>>) -> AgentRunParams {
    AgentRunParams {
        run_id: "r1".into(),
        agent_id: "a1".into(),
        system: "Be brief.".into(),
        instructions: "do it".into(),
        images: None,
        tools,
        max_steps: 4,
        submit,
        check: false,
        recent_images: None,
        max_tokens: None,
    }
}

fn picture(n: u8) -> FileValue {
    FileValue {
        file: format!("{n:064x}"),
    }
}

fn rpc(error: &AgentError) -> RpcError {
    error.to_rpc()
}

#[tokio::test]
async fn each_turn_sends_the_transcript_the_tools_and_the_choice() {
    let model = Model::new(vec![
        reply(
            json!({"text": "thinking", "tool_calls": [
                {"id": "c1", "name": "look", "arguments": {"what": "x"}},
                {"id": null, "name": "nope", "arguments": {}},
                {"id": "c3", "name": "look", "arguments": {"what": "boom"}}
            ]}),
            1_000,
        ),
        reply(
            json!({"text": "", "tool_calls": [
                {"id": "c4", "name": "submit", "arguments": {"answer": "good"}}
            ]}),
            2_000,
        ),
    ]);
    let body = Body::new(
        |_, arguments| match arguments["what"].as_str() {
            Some("x") => Ok(ToolInvokeResult::Content {
                content: json!({"saw": "x", "n": 1, "f": 1.0}),
                images: None,
            }),
            _ => Ok(ToolInvokeResult::Error {
                error: "RuntimeError: bad look".into(),
            }),
        },
        Vec::new(),
    );
    let mut run = params(vec![look_tool()], Some(answer_schema()));
    run.max_tokens = Some(50);
    let result = run_agent(&run, &model, &body).await.unwrap();

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let tools = json!([
        {
            "name": "look",
            "description": "Look at something.",
            "parameters": {
                "type": "object",
                "properties": {"what": {"type": "string"}},
                "required": ["what"]
            }
        },
        {
            "name": "submit",
            "description": "Finish: submit the answer this task asks for.",
            "parameters": {
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            }
        }
    ]);
    assert_eq!(
        requests[0],
        json!({
            "system": "Be brief.",
            "messages": [{"role": "user", "content": "do it"}],
            "tools": tools,
            "tool_choice": "required",
            "max_tokens": 50
        })
    );
    assert_eq!(
        requests[1]["messages"],
        json!([
            {"role": "user", "content": "do it"},
            // A call's arguments travel as the canonical JSON text of the object.
            {"role": "assistant", "content": "thinking", "tool_calls": [
                {"id": "c1", "name": "look", "arguments": "{\"what\":\"x\"}"},
                {"id": null, "name": "nope", "arguments": "{}"},
                {"id": "c3", "name": "look", "arguments": "{\"what\":\"boom\"}"}
            ]},
            {"role": "tool", "name": "look", "tool_call_id": "c1", "content": "{\"f\":1,\"n\":1,\"saw\":\"x\"}"},
            {"role": "tool", "name": "nope", "content": "no tool named nope"},
            {"role": "tool", "name": "look", "tool_call_id": "c3", "content": "RuntimeError: bad look"}
        ])
    );
    assert_eq!(requests[1]["tools"], tools);
    assert_eq!(requests[1]["tool_choice"], json!("required"));

    assert_eq!(
        body.invoked(),
        vec![
            (
                "a1".to_string(),
                Some("c1".to_string()),
                "look".to_string(),
                json!({"what": "x"})
            ),
            (
                "a1".to_string(),
                Some("c3".to_string()),
                "look".to_string(),
                json!({"what": "boom"})
            ),
        ]
    );
    assert_eq!(
        result.answer,
        AgentAnswer::Submitted {
            submitted: json!({"answer": "good"})
        }
    );
    assert_eq!(result.turns, 2);
    assert_eq!(result.cost_usd, 0.003);
    // The transcript ends with the reply that submitted, unanswered.
    assert_eq!(result.transcript.len(), 6);
    assert_eq!(
        serde_json::to_value(&result.transcript[5]).unwrap(),
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c4", "name": "submit", "arguments": "{\"answer\":\"good\"}"}
        ]})
    );
}

#[tokio::test]
async fn a_reply_without_calls_ends_the_loop_with_its_text() {
    let model = Model::new(vec![reply(json!({"text": "a red square"}), 10)]);
    let result = run_agent(&params(vec![look_tool()], None), &model, &Body::quiet())
        .await
        .unwrap();
    assert_eq!(
        result.answer,
        AgentAnswer::Text {
            text: "a red square".into()
        }
    );
    assert_eq!(result.turns, 1);
    assert_eq!(model.requests()[0]["tool_choice"], json!("auto"));
    assert_eq!(model.requests()[0].get("max_tokens"), None);
    assert_eq!(
        serde_json::to_value(&result.transcript).unwrap(),
        json!([
            {"role": "user", "content": "do it"},
            {"role": "assistant", "content": "a red square", "tool_calls": []}
        ])
    );
}

#[tokio::test]
async fn with_submit_a_reply_without_calls_is_told_to_finish() {
    let model = Model::new(vec![
        reply(json!({"tool_calls": []}), 10),
        reply(
            json!({"text": "done", "tool_calls": [
                {"id": "s", "name": "submit", "arguments": {"answer": "yes"}}
            ]}),
            10,
        ),
    ]);
    let result = run_agent(
        &params(vec![], Some(answer_schema())),
        &model,
        &Body::quiet(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.answer,
        AgentAnswer::Submitted {
            submitted: json!({"answer": "yes"})
        }
    );
    assert_eq!(
        model.requests()[1]["messages"],
        json!([
            {"role": "user", "content": "do it"},
            {"role": "assistant", "content": "", "tool_calls": []},
            {"role": "user", "content": "Finish by calling submit."}
        ])
    );
}

#[tokio::test]
async fn submissions_are_refused_by_the_schema_then_by_the_check() {
    let long = "x".repeat(700);
    let model = Model::new(vec![
        reply(
            json!({"text": "", "tool_calls": [
                {"id": "s1", "name": "submit", "arguments": {"answer": 3}}
            ]}),
            10,
        ),
        reply(
            json!({"text": "", "tool_calls": [
                {"id": "s2", "name": "submit", "arguments": {"answer": "ok"}}
            ]}),
            10,
        ),
        reply(
            json!({"text": "", "tool_calls": [
                {"name": "submit", "arguments": {"answer": "long"}}
            ]}),
            10,
        ),
        reply(
            json!({"text": "", "tool_calls": [
                {"id": "s4", "name": "submit", "arguments": {"answer": "good"}},
                {"id": "l1", "name": "look", "arguments": {"what": "late"}}
            ]}),
            10,
        ),
    ]);
    let body = Body::new(
        |_, _| panic!("a call after an accepted submit is never run"),
        vec![Ok(Some("not good enough".into())), Ok(Some(long)), Ok(None)],
    );
    let mut run = params(vec![look_tool()], Some(answer_schema()));
    run.check = true;
    let result = run_agent(&run, &model, &body).await.unwrap();
    assert_eq!(
        result.answer,
        AgentAnswer::Submitted {
            submitted: json!({"answer": "good"})
        }
    );
    let transcript = serde_json::to_value(&result.transcript).unwrap();
    let schema_refusal = transcript[2]["content"].as_str().unwrap();
    assert!(
        schema_refusal.starts_with("refused: answer: "),
        "{schema_refusal}"
    );
    assert!(schema_refusal.contains("string"), "{schema_refusal}");
    assert_eq!(transcript[2]["tool_call_id"], json!("s1"));
    assert_eq!(
        transcript[4],
        json!({"role": "tool", "name": "submit", "tool_call_id": "s2", "content": "refused: not good enough"})
    );
    // A check's refusal is cut to 500 characters; a call without an id gets no tool_call_id.
    assert_eq!(
        transcript[6],
        json!({"role": "tool", "name": "submit", "content": format!("refused: {}", "x".repeat(500))})
    );
    // The schema refused the first value, so only the later three reached the check.
    let checked = body.checked.lock().unwrap().clone();
    assert_eq!(
        checked,
        vec![
            ("a1".to_string(), json!({"answer": "ok"})),
            ("a1".to_string(), json!({"answer": "long"})),
            ("a1".to_string(), json!({"answer": "good"})),
        ]
    );
    // The accepted submit ends the loop at once: the later look call stays unanswered.
    assert_eq!(result.transcript.len(), 8);
    assert!(body.invoked().is_empty());
    assert_eq!(result.turns, 4);
}

#[tokio::test]
async fn a_nested_schema_refusal_names_its_place() {
    let schema = object(json!({
        "type": "object",
        "properties": {"items": {"type": "array", "items": {"type": "integer"}}},
        "required": ["items"]
    }));
    let model = Model::new(vec![
        reply(
            json!({"text": "", "tool_calls": [
                {"id": "s1", "name": "submit", "arguments": {"items": [1, "two"]}}
            ]}),
            10,
        ),
        reply(
            json!({"text": "", "tool_calls": [{"id": "s2", "name": "submit", "arguments": {}}]}),
            10,
        ),
        reply(
            json!({"text": "", "tool_calls": [
                {"id": "s3", "name": "submit", "arguments": {"items": []}}
            ]}),
            10,
        ),
    ]);
    let result = run_agent(&params(vec![], Some(schema)), &model, &Body::quiet())
        .await
        .unwrap();
    let transcript = serde_json::to_value(&result.transcript).unwrap();
    let nested = transcript[2]["content"].as_str().unwrap();
    assert!(nested.starts_with("refused: items/1: "), "{nested}");
    let top = transcript[4]["content"].as_str().unwrap();
    assert!(top.starts_with("refused: the answer: "), "{top}");
}

#[tokio::test]
async fn without_a_submit_schema_submit_is_an_unknown_tool() {
    let model = Model::new(vec![
        reply(
            json!({"text": "", "tool_calls": [{"id": "s", "name": "submit", "arguments": {"a": 1}}]}),
            10,
        ),
        reply(json!({"text": "fine"}), 10),
    ]);
    let result = run_agent(&params(vec![], None), &model, &Body::quiet())
        .await
        .unwrap();
    assert_eq!(
        result.transcript[2].content, "no tool named submit",
        "{:?}",
        result.transcript
    );
    assert_eq!(
        result.answer,
        AgentAnswer::Text {
            text: "fine".into()
        }
    );
}

#[tokio::test]
async fn recent_images_keep_the_newest_pictures_in_each_request() {
    let model = Model::new(vec![
        reply(
            json!({"text": "", "tool_calls": [{"id": "c1", "name": "look", "arguments": {"what": "pic"}}]}),
            10,
        ),
        reply(json!({"text": "seen"}), 10),
    ]);
    let body = Body::new(
        |_, _| {
            Ok(ToolInvokeResult::Content {
                content: json!("here"),
                images: Some(vec![picture(3)]),
            })
        },
        Vec::new(),
    );
    let mut run = params(vec![look_tool()], None);
    run.images = Some(vec![picture(1), picture(2)]);
    run.recent_images = Some(2);
    let result = run_agent(&run, &model, &body).await.unwrap();
    let requests = model.requests();
    assert_eq!(
        requests[0]["messages"],
        json!([{"role": "user", "content": "do it", "images": [
            {"file": format!("{:064x}", 1)}, {"file": format!("{:064x}", 2)}
        ]}])
    );
    assert_eq!(
        requests[1]["messages"],
        json!([
            {"role": "user", "content": "do it\n[1 older picture(s) not shown]", "images": [
                {"file": format!("{:064x}", 2)}
            ]},
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "c1", "name": "look", "arguments": "{\"what\":\"pic\"}"}
            ]},
            {"role": "tool", "name": "look", "tool_call_id": "c1", "content": "here", "images": [
                {"file": format!("{:064x}", 3)}
            ]}
        ])
    );
    // The transcript keeps every picture.
    assert_eq!(
        result.transcript[0].images,
        Some(vec![picture(1), picture(2)])
    );
}

#[tokio::test]
async fn running_out_of_steps_is_agent_unfinished() {
    let call = json!({"text": "", "tool_calls": [{"id": "c", "name": "nope", "arguments": {}}]});
    let model = Model::new(vec![reply(call.clone(), 10), reply(call, 10)]);
    let mut run = params(vec![], None);
    run.max_steps = 2;
    let error = run_agent(&run, &model, &Body::quiet()).await.unwrap_err();
    let error = rpc(&error);
    assert_eq!(error.code, ErrorCode::AgentUnfinished.code());
    assert_eq!(error.message, "the agent did not finish within 2 turns");
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn a_tool_that_fails_the_node_ends_the_loop_with_its_error() {
    let model = Model::new(vec![reply(
        json!({"text": "", "tool_calls": [
            {"id": "c1", "name": "look", "arguments": {"what": "x"}},
            {"id": "c2", "name": "look", "arguments": {"what": "y"}}
        ]}),
        10,
    )]);
    let failure = RpcError::new(ErrorCode::NodeFailure, "the picture is unusable")
        .with_data(json!({"facts": {"kept": 1}}));
    let returned = failure.clone();
    let body = Body::new(move |_, _| Err(returned.clone()), Vec::new());
    let error = run_agent(&params(vec![look_tool()], None), &model, &body)
        .await
        .unwrap_err();
    assert_eq!(error, AgentError::Rpc(failure));
    assert_eq!(body.invoked().len(), 1);
}

#[tokio::test]
async fn a_check_that_raised_ends_the_loop_with_its_error() {
    let model = Model::new(vec![reply(
        json!({"text": "", "tool_calls": [{"id": "s", "name": "submit", "arguments": {"answer": "a"}}]}),
        10,
    )]);
    let raised = RpcError::new(ErrorCode::NodeError, "KeyError: 'x'");
    let body = Body::new(|_, _| unreachable!(), vec![Err(raised.clone())]);
    let mut run = params(vec![], Some(answer_schema()));
    run.check = true;
    let error = run_agent(&run, &model, &body).await.unwrap_err();
    assert_eq!(error, AgentError::Rpc(raised));
}

#[tokio::test]
async fn a_turn_that_fails_ends_the_loop_with_the_call_error() {
    let refused = CallError::Rpc(RpcError::new(
        ErrorCode::OverBound,
        "scout declared at most 1 agent.turn calls",
    ));
    let model = Model::new(vec![
        reply(
            json!({"text": "", "tool_calls": [{"id": "c", "name": "nope", "arguments": {}}]}),
            10,
        ),
        Err(refused.clone()),
    ]);
    let error = run_agent(&params(vec![], None), &model, &Body::quiet())
        .await
        .unwrap_err();
    assert_eq!(error, AgentError::Call(refused));
}

#[tokio::test]
async fn cached_turns_cost_nothing() {
    let model = Model::new(vec![
        cached(json!({"text": "", "tool_calls": [{"id": "c", "name": "nope", "arguments": {}}]})),
        reply(json!({"text": "done"}), 2_500),
    ]);
    let result = run_agent(&params(vec![], None), &model, &Body::quiet())
        .await
        .unwrap();
    assert_eq!(result.turns, 2);
    assert_eq!(result.cost_usd, 0.0025);
}

#[tokio::test]
async fn a_reply_of_another_shape_fails_the_call() {
    for (data, why) in [
        (json!("text"), "the reply is not an object"),
        (json!({"text": 3}), "text is not a string"),
        (json!({"tool_calls": {}}), "tool_calls is not a list"),
        (json!({"tool_calls": [3]}), "tool_calls[0] is not an object"),
        (
            json!({"tool_calls": [{"id": 3, "name": "x", "arguments": {}}]}),
            "tool_calls[0].id is not a string",
        ),
        (
            json!({"tool_calls": [{"arguments": {}}]}),
            "tool_calls[0] has no name",
        ),
        (
            json!({"tool_calls": [{"name": "x", "arguments": [1]}]}),
            "tool_calls[0].arguments is not an object",
        ),
    ] {
        let model = Model::new(vec![reply(data.clone(), 10)]);
        let error = run_agent(&params(vec![], None), &model, &Body::quiet())
            .await
            .unwrap_err();
        let AgentError::Call(CallError::Rpc(error)) = error else {
            panic!("{data}: {error:?}")
        };
        assert_eq!(error.code, ErrorCode::CallFailed.code(), "{data}");
        assert!(error.message.ends_with(why), "{data}: {}", error.message);
        assert_eq!(
            error.data.as_ref().unwrap()["capability"],
            json!("agent.turn")
        );
    }
}

#[tokio::test]
async fn a_lenient_reply_reads_missing_members_as_empty() {
    let model = Model::new(vec![
        reply(
            json!({"text": null, "tool_calls": [{"name": "nope"}], "usage": {"tokens": 3}}),
            10,
        ),
        reply(json!({"tool_calls": null}), 10),
    ]);
    let result = run_agent(&params(vec![], None), &model, &Body::quiet())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&result.transcript).unwrap(),
        json!([
            {"role": "user", "content": "do it"},
            {"role": "assistant", "content": "", "tool_calls": [{"name": "nope", "arguments": "{}"}]},
            {"role": "tool", "name": "nope", "content": "no tool named nope"},
            {"role": "assistant", "content": "", "tool_calls": []}
        ])
    );
    assert_eq!(
        result.answer,
        AgentAnswer::Text {
            text: String::new()
        }
    );
}

#[tokio::test]
async fn agent_runs_that_cannot_start_are_refused_before_any_turn() {
    let model = Model::new(Vec::new());
    let mut named_submit = look_tool();
    named_submit.name = "submit".into();
    let mut marked = look_tool();
    marked.parameters = object(json!({"type": "object", "default": {"missing": true}}));
    let mut no_steps = params(vec![], None);
    no_steps.max_steps = 0;
    for (run, message) in [
        (
            params(vec![named_submit], None),
            "submit is the agent's own tool; name yours otherwise".to_string(),
        ),
        (
            params(vec![marked], None),
            "tools[0].parameters.default: an object holding only `missing` in this shape is \
             reserved for FX's own values"
                .to_string(),
        ),
        (
            params(
                vec![],
                Some(object(json!({"type": "object", "const": {"pending": 1}}))),
            ),
            "submit.const: an object holding only `pending` in this shape is reserved for FX's \
             own values"
                .to_string(),
        ),
        (
            no_steps,
            "an agent takes at least 1 step (max_steps)".to_string(),
        ),
    ] {
        let error = rpc(&run_agent(&run, &model, &Body::quiet()).await.unwrap_err());
        assert_eq!(error.code, ErrorCode::InvalidParams.code(), "{message}");
        assert_eq!(error.message, message);
    }
    let bad_schema = params(vec![], Some(object(json!({"type": 5}))));
    let error = rpc(&run_agent(&bad_schema, &model, &Body::quiet())
        .await
        .unwrap_err());
    assert_eq!(error.code, ErrorCode::InvalidParams.code());
    assert!(
        error
            .message
            .starts_with("the submit schema is not a JSON Schema: "),
        "{}",
        error.message
    );
    assert!(model.requests().is_empty());
}
