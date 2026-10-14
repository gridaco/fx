//! The Grok Build adapter and runtime (src/grok/README.md) against fake Grok Build programs:
//! shell scripts that speak the agent's side of ACP. No real Grok Build is ever started.
//!
//! The runtime's reader and writer run on the tokio runtime that started them, so each test runs
//! its calls, and the shutdown, on one runtime ([`on_one_runtime`]).
#![cfg(unix)]

use grida_fx_providers::adapter::{Answer, Billing, RequestAdapter, Runtime, Sent};
use grida_fx_providers::grok::{self, GrokImages, GrokRuntime};
use grida_fx_providers::testing::{self, CallBuilder, TestCall, media};
use serde_json::{Value, json};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Refuses to start unless asked for `agent --no-leader stdio` with API-key sign-in off, no xAI
/// key, nothing imported and FX's overlay; counts each start.
const STARTING: &str = r#"#!/bin/sh
here="$(dirname "$0")"
case "$*" in "agent --no-leader stdio") ;; *) exit 9 ;; esac
[ "$GROK_DISABLE_API_KEY_AUTH" = 1 ] || exit 9
[ -z "$XAI_API_KEY" ] || exit 9
[ "$GROK_CURSOR_MCPS_ENABLED" = false ] || exit 9
case "$GROK_CONFIG" in *'"image_gen":true'*) ;; *) exit 9 ;; esac
echo start >> "$here/starts"
"#;

/// The variables of the built-in declarations, which the program never inherits.
fn withheld() -> Arc<[String]> {
    Arc::from(grida_fx_core::providers::Declarations::built_in().variables())
}

/// `initialize` (Grok Build `version`, sign-in methods `methods`), then, when `cached_token` is
/// offered, `authenticate` with it (an account billed through the API when `billed`).
fn handshake(version: &str, methods: &[&str], billed: bool) -> String {
    let methods: Vec<Value> = methods
        .iter()
        .map(|m| json!({"id": m, "name": m}))
        .collect();
    let initialized = json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1,
        "authMethods": methods, "_meta": {"agentVersion": version}}});
    let mut script = format!("read -r line\necho '{initialized}'\n");
    if methods.iter().any(|m| m["id"] == "cached_token") {
        let account = json!({"jsonrpc": "2.0", "id": 2,
            "result": {"_meta": {"backend_billed": billed, "email": "someone@example.com"}}});
        script.push_str(&format!(
            "read -r line\ncase \"$line\" in *'\"methodId\":\"cached_token\"'*) ;; *) exit 8 ;; esac\necho '{account}'\n"
        ));
    }
    script
}

fn signed_in() -> String {
    handshake("1.0.52", &["cached_token", "grok.com"], false)
}

/// A call's `session/new` (request `id`, answered with session `session`), checked for FX's agent
/// and an empty MCP list; its effort (`id + 1`); then its `session/prompt` (`id + 2`), kept as
/// `prompt-<session>`.
fn session(id: u64, session: &str) -> String {
    format!(
        r#"read -r line
case "$line" in *'"tools":["image_gen","image_edit"]'*) ;; *) exit 7 ;; esac
case "$line" in *'"disallowedTools":["search_tool","use_tool"]'*) ;; *) exit 7 ;; esac
case "$line" in *'"mcpServers":[]'*) ;; *) exit 7 ;; esac
case "$line" in *'"yoloMode":false'*) ;; *) exit 7 ;; esac
echo '{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"{session}"}}}}'
read -r line
case "$line" in *'"configId":"reasoning_effort"'*'"value":"low"'*) ;; *) exit 6 ;; esac
echo '{{"jsonrpc":"2.0","id":{},"result":{{"configOptions":[]}}}}'
read -r line
printf '%s' "$line" > "$here/prompt-{session}"
"#,
        id + 1
    )
}

fn echo(message: Value) -> String {
    format!("echo '{message}'\n")
}

fn update(session: &str, update: Value) -> String {
    echo(json!({"jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": session, "update": update}}))
}

/// An image tool call (`tool`) of `session` that completed with the image at `path`.
fn drew(session: &str, tool: &str, path: &Path) -> String {
    let kind = if tool == "image_edit" {
        "ImageEdit"
    } else {
        "ImageGen"
    };
    update(
        session,
        json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": tool,
            "_meta": {"x.ai/tool": {"name": tool, "kind": tool}}}),
    ) + &update(
        session,
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
            "rawOutput": {"type": kind, "path": path.to_string_lossy(), "filename": "1.jpg",
                "session_folder": "images"}}),
    )
}

/// After an image: FX's `session/cancel`, then the prompt's answer (request `prompt`) with its
/// usage, then FX's `_x.ai/session/delete` (request `prompt + 1`); each is noted in `events`.
fn settled(session: &str, prompt: u64) -> String {
    let answered = json!({"jsonrpc": "2.0", "id": prompt, "result": {"stopReason": "cancelled",
        "_meta": {"modelId": "grok-4.7", "usage": {"inputTokens": 11081, "outputTokens": 182,
            "cachedReadTokens": 6784, "costUsdTicks": 44465200}}}});
    cancelled(session) + &echo(answered) + &deleted(session, prompt + 1)
}

/// FX's `session/cancel` of `session`, noted in `events`.
fn cancelled(session: &str) -> String {
    format!(
        r#"read -r line
case "$line" in *'"method":"session/cancel"'*'"{session}"'*) echo cancelled >> "$here/events" ;; *) exit 5 ;; esac
"#
    )
}

/// FX's `_x.ai/session/delete` of `session` (request `id`), noted in `events` and answered.
fn deleted(session: &str, id: u64) -> String {
    format!(
        r#"read -r line
case "$line" in *'"method":"_x.ai/session/delete"'*'"{session}"'*) echo deleted >> "$here/events" ;; *) exit 4 ;; esac
echo '{{"jsonrpc":"2.0","id":{id},"result":{{"success":true}}}}'
"#
    )
}

/// The prompt (request `id`) ends without an image.
fn ended(id: u64, reason: &str) -> String {
    echo(json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": reason}}))
}

struct Fake {
    dir: tempfile::TempDir,
    program: PathBuf,
}

impl Fake {
    fn marker(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.marker(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Where Grok Build saved `session`'s first image: `<dir>/sessions/<cwd>/<session>/images/1.jpg`.
    fn saved(&self, session: &str, bytes: &[u8]) -> PathBuf {
        let images = self
            .dir
            .path()
            .join("sessions")
            .join("%2Fwork")
            .join(session)
            .join("images");
        std::fs::create_dir_all(&images).unwrap();
        let path = images.join("1.jpg");
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

/// A fake Grok Build whose script is `body`, after [`STARTING`]; `body` may use the fake's
/// folder through `saved` before it is written.
fn fake(body: impl FnOnce(&Fake) -> String) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("grok");
    let fake = Fake { dir, program };
    let script = format!("{STARTING}{}", body(&fake));
    testing::write_program(&fake.program, &script);
    fake
}

fn contract() -> Value {
    json!({"adapter": grok::ADAPTER, "adapter_behavior": "1"})
}

fn kite() -> TestCall {
    CallBuilder::new("image.generate", "imagine@grok")
        .contract(contract())
        .request(json!({"prompt": "a red kite"}))
        .build()
}

/// Runs `test` on one tokio runtime over `program`, then shuts the runtime down, as the engine
/// does after a run; returns what `test` returned and whether the shutdown verified.
fn on_one_runtime<T, F: Future<Output = T>>(
    program: Result<PathBuf, String>,
    test: impl FnOnce(GrokImages) -> F,
) -> (T, bool) {
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio.block_on(async {
        let runtime = Arc::new(GrokRuntime::new(program, withheld()));
        let adapter = GrokImages::new(Arc::clone(&runtime))
            .with_deadlines(Duration::from_secs(20), Duration::from_secs(30));
        let out = test(adapter).await;
        let verified = runtime.shutdown().await;
        (out, verified)
    })
}

/// Waits up to 10 s for `path` to exist.
async fn appears(path: &Path) -> bool {
    let started = Instant::now();
    while !path.exists() {
        if started.elapsed() > Duration::from_secs(10) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

fn send(fake: &Fake, call: &TestCall) -> Sent {
    on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
        adapter.send(&call.call).await
    })
    .0
}

/// Two calls on one runtime: what each was sent, and whether the shutdown verified.
fn send_twice(fake: &Fake) -> ((Sent, Sent), bool) {
    on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
        let first = adapter.send(&kite().call).await;
        let second = adapter.send(&kite().call).await;
        (first, second)
    })
}

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("{other:?}"),
    }
}

fn failed(reason: &str) -> Sent {
    Sent::Failed {
        reason: reason.into(),
        cost: None,
        retryable: false,
    }
}

fn refused(reason: &str) -> Sent {
    Sent::Refused {
        reason: reason.into(),
    }
}

#[test]
fn a_drawn_jpeg_is_answered_as_a_png_with_grok_s_usage_and_no_cost() {
    let fake = fake(|f| {
        let saved = f.saved("s-a", &media::jpeg(6, 4));
        format!(
            "{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            drew("s-a", "image_gen", &saved),
            settled("s-a", 5)
        )
    });
    let call = kite();
    let ((sent, checked), verified) =
        on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
            let sent = adapter.send(&call.call).await;
            let checked = match &sent {
                Sent::Answered(answer) => adapter.check(&call.call, answer),
                _ => Ok(()),
            };
            assert_eq!(adapter.billing(), Billing::Plan);
            (sent, checked)
        });
    let answer = answered(sent);
    assert!(verified);
    assert_eq!(checked, Ok(()));
    assert_eq!(
        answer.cost, None,
        "Grok Build reports no cost: the route's hold is charged"
    );
    assert_eq!(answer.data, Value::Null);
    let image = &answer.files["image"];
    assert_eq!(image.kind, "image/png");
    let decoded = image::load_from_memory(&image.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (6, 4));
    assert_eq!(
        answer.usage.expect("Grok's usage").to_value(Billing::Plan),
        json!({"billing": "plan", "runtime": "grok 1.0.52", "agent_model": "grok-4.7",
            "input_tokens": 11081, "cached_input_tokens": 6784, "output_tokens": 182})
    );
    assert_eq!(fake.lines("events"), ["cancelled", "deleted"]);
    let prompt = std::fs::read_to_string(fake.marker("prompt-s-a")).unwrap();
    assert!(
        prompt.contains("Call the image_gen tool exactly once."),
        "{prompt}"
    );
    assert!(
        prompt.contains("<prompt>\\na red kite\\n</prompt>"),
        "{prompt}"
    );
}

#[test]
fn one_grok_serves_every_call_of_an_invocation() {
    let fake = fake(|f| {
        let (a, b) = (
            f.saved("s-a", &media::jpeg(2, 2)),
            f.saved("s-b", &media::jpeg(3, 3)),
        );
        format!(
            "{}{}{}{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            drew("s-a", "image_gen", &a),
            settled("s-a", 5),
            session(7, "s-b"),
            drew("s-b", "image_gen", &b),
            settled("s-b", 9)
        )
    });
    let (sizes, verified) = on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
        let mut sizes = Vec::new();
        for _ in 0..2 {
            let answer = answered(adapter.send(&kite().call).await);
            let decoded = image::load_from_memory(&answer.files["image"].bytes).unwrap();
            sizes.push(decoded.width());
        }
        sizes
    });
    assert!(verified);
    assert_eq!(sizes, [2, 3]);
    assert_eq!(fake.lines("starts"), ["start"]);
}

#[test]
fn pictures_are_copied_into_the_call_s_folder_and_named_to_the_edit_tool_in_order() {
    let png = media::png(2, 2, None);
    let photo = media::jpeg(3, 2);
    let mut builder = CallBuilder::new("image.edit", "imagine@grok").contract(contract());
    let image = builder.file("image/png", &png);
    let reference = builder.file("image/jpeg", &photo);
    let call = builder
        .request(json!({"prompt": "the boat on the lake", "image": image,
            "references": [reference]}))
        .build();
    let fake = fake(|f| {
        let saved = f.saved("s-a", &media::jpeg(4, 4));
        let seen = r#"case "$line" in *"Call the image_edit tool exactly once. Set image to these input images, in this order: \`"*"/picture-01.png\`, \`"*"/picture-02.jpg\`. Pass the text"*) ;; *) exit 3 ;; esac
first=$(printf '%s' "$line" | sed 's/.*`\([^`]*picture-01.png\)`.*/\1/')
second=$(printf '%s' "$line" | sed 's/.*`\([^`]*picture-02.jpg\)`.*/\1/')
cp "$first" "$here/seen-01"
cp "$second" "$here/seen-02"
printf '%s' "$first" > "$here/first"
"#;
        format!(
            "{}{}{seen}{}{}",
            signed_in(),
            session(3, "s-a"),
            drew("s-a", "image_edit", &saved),
            settled("s-a", 5)
        )
    });
    let (sent, verified) = on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
        adapter.send(&call.call).await
    });
    assert!(verified);
    answered(sent);
    assert_eq!(std::fs::read(fake.marker("seen-01")).unwrap(), png);
    assert_eq!(std::fs::read(fake.marker("seen-02")).unwrap(), photo);
    let first = std::fs::read_to_string(fake.marker("first")).unwrap();
    assert!(Path::new(&first).is_absolute());
    assert!(
        !Path::new(&first).exists(),
        "the call's pictures outlived it"
    );
}

#[test]
fn permission_is_granted_for_the_image_tools_only() {
    let fake = fake(|f| {
        let saved = f.saved("s-a", &media::jpeg(2, 2));
        let ask = |id: &str, title: &str| {
            echo(
                json!({"jsonrpc": "2.0", "id": id, "method": "session/request_permission",
                "params": {"sessionId": "s-a", "toolCall": {"toolCallId": "c9", "title": title},
                    "options": [
                        {"optionId": "a1", "name": "Allow", "kind": "allow_once"},
                        {"optionId": "a2", "name": "Always", "kind": "allow_always"},
                        {"optionId": "r1", "name": "Reject", "kind": "reject_once"}]}}),
            )
        };
        format!(
            "{}{}{}read -r line\ncase \"$line\" in *'\"id\":\"p1\"'*'\"optionId\":\"r1\"'*) ;; *) exit 2 ;; esac\n{}read -r line\ncase \"$line\" in *'\"id\":\"p2\"'*'\"optionId\":\"a1\"'*) ;; *) exit 2 ;; esac\n{}{}",
            signed_in(),
            session(3, "s-a"),
            ask("p1", "run_terminal_command"),
            ask("p2", "image_gen"),
            drew("s-a", "image_gen", &saved),
            settled("s-a", 5)
        )
    });
    answered(send(&fake, &kite()));
}

#[test]
fn a_call_of_any_other_tool_fails_the_call_and_ends_its_session() {
    let fake = fake(|_| {
        format!(
            "{}{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            update(
                "s-a",
                json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Run ls",
                    "_meta": {"x.ai/tool": {"name": "run_terminal_command"}}})
            ),
            cancelled("s-a"),
            deleted("s-a", 6)
        )
    });
    assert_eq!(
        send(&fake, &kite()),
        failed(
            "Grok Build image generation failed: Grok Build called run_terminal_command, a tool \
             FX does not allow"
        )
    );
    assert_eq!(fake.lines("events"), ["cancelled", "deleted"]);
}

#[test]
fn an_image_outside_the_call_s_session_is_not_read() {
    let fake = fake(|f| {
        let elsewhere = f.saved("s-other", &media::jpeg(2, 2));
        format!(
            "{}{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            drew("s-a", "image_gen", &elsewhere),
            ended(5, "end_turn"),
            deleted("s-a", 6)
        )
    });
    assert_eq!(
        send(&fake, &kite()),
        failed("Grok Build image generation failed: Grok Build reported an image FX does not read")
    );
}

#[test]
fn a_turn_without_an_image_fails_with_its_stop_reason_only() {
    let fake = fake(|_| {
        format!(
            "{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            ended(5, "max_turn_requests"),
            deleted("s-a", 6)
        )
    });
    assert_eq!(
        send(&fake, &kite()),
        failed(
            "Grok Build image generation failed: Grok Build's turn ended max_turn_requests \
             without an image"
        )
    );
}

#[test]
fn an_account_without_image_generation_refuses_the_rest_of_the_invocation() {
    let fake = fake(|_| {
        let failure = update(
            "s-a",
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "failed",
                "_meta": {"x.ai/tool": {"name": "image_gen"}},
                "content": [{"type": "content", "content": {"type": "text",
                    "text": "Image generation is a SuperGrok feature, see /Users/someone/x"}}]}),
        );
        format!(
            "{}{}{failure}{}{}",
            signed_in(),
            session(3, "s-a"),
            ended(5, "end_turn"),
            deleted("s-a", 6)
        )
    });
    let ((first, second), verified) = send_twice(&fake);
    assert!(verified);
    assert_eq!(
        first,
        failed(&format!(
            "Grok Build image generation failed: {}",
            grok::NO_SUBSCRIPTION
        ))
    );
    assert_eq!(second, refused(grok::NO_SUBSCRIPTION));
}

#[test]
fn only_a_recent_grok_signed_in_with_a_subscription_may_draw_and_a_refusal_holds() {
    for (script, reason) in [
        (
            handshake("1.0.52", &["grok.com"], false),
            grok::NOT_SIGNED_IN.to_string(),
        ),
        (
            handshake("1.0.30", &["cached_token"], false),
            "Grok Build 1.0.30 is older than 1.0.52: run grok update".to_string(),
        ),
        (
            handshake("1.0.52", &["cached_token"], true),
            "Grok Build bills this account through the xAI API, not a Grok subscription: sign \
             in with grok login"
                .to_string(),
        ),
    ] {
        let fake = fake(|_| script.clone());
        let ((first, second), verified) = send_twice(&fake);
        assert!(verified);
        assert_eq!(first, refused(&reason));
        assert_eq!(second, refused(&reason));
        assert_eq!(fake.lines("starts"), ["start"], "{reason}");
    }
}

#[test]
fn a_missing_grok_and_unhonoured_values_are_refused_before_anything_starts() {
    let (sent, verified) = on_one_runtime(Err(grok::NOT_INSTALLED.into()), |adapter| async move {
        adapter.send(&kite().call).await
    });
    assert!(verified);
    assert_eq!(sent, refused(grok::NOT_INSTALLED));
    let fake = fake(|_| "exit 1\n".to_string());
    for (request, reason) in [
        (
            json!({"prompt": "p", "size": "1024x1024"}),
            grok::NO_EXACT_SIZE,
        ),
        (
            json!({"prompt": "p", "background": "transparent"}),
            grok::NO_TRANSPARENCY,
        ),
        (
            json!({"prompt": " ", "provider_options": {"grok": {"seed": 1}}}),
            "Grok Build takes no provider_options",
        ),
    ] {
        let call = CallBuilder::new("image.generate", "imagine@grok")
            .contract(contract())
            .request(request)
            .build();
        assert_eq!(send(&fake, &call), refused(reason));
    }
    // A route's own are refused the same way.
    let mut route = contract();
    route["provider_options"] = json!({"grok": {"seed": 1}});
    let call = CallBuilder::new("image.generate", "imagine@grok")
        .contract(route)
        .request(json!({"prompt": "p"}))
        .build();
    assert_eq!(
        send(&fake, &call),
        refused("Grok Build takes no provider_options")
    );
    assert!(fake.lines("starts").is_empty());
}

#[test]
fn provider_options_for_other_providers_are_not_read() {
    let (sent, verified) = on_one_runtime(Err(grok::NOT_INSTALLED.into()), |adapter| async move {
        let call = CallBuilder::new("image.generate", "imagine@grok")
            .contract(contract())
            .request(json!({"prompt": "p",
                            "provider_options": {"grok": {}, "openai": {"seed": 1}}}))
            .build();
        adapter.send(&call).await
    });
    assert!(verified);
    // Past the options, to the program.
    assert_eq!(sent, refused(grok::NOT_INSTALLED));
}

#[test]
fn a_call_that_runs_out_of_time_cancels_its_turn_and_deletes_its_session() {
    let fake = fake(|_| {
        format!(
            "{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            cancelled("s-a"),
            deleted("s-a", 6)
        )
    });
    let (sent, verified) = on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
        let adapter = adapter.with_deadlines(Duration::from_secs(20), Duration::from_secs(2));
        adapter.send(&kite().call).await
    });
    assert!(verified);
    assert_eq!(
        sent,
        failed("Grok Build image generation failed: Grok Build did not finish within 2 s")
    );
    assert_eq!(fake.lines("events"), ["cancelled", "deleted"]);
}

#[test]
fn an_image_tool_that_failed_may_be_tried_again_by_the_engine() {
    let fake = fake(|_| {
        let failure = update(
            "s-a",
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "failed",
                "_meta": {"x.ai/tool": {"name": "image_gen"}},
                "content": [{"type": "content", "content": {"type": "text",
                    "text": "Tool `image_gen` failed: error sending request for url"}}]}),
        );
        format!(
            "{}{}{failure}{}{}",
            signed_in(),
            session(3, "s-a"),
            ended(5, "end_turn"),
            deleted("s-a", 6)
        )
    });
    assert_eq!(
        send(&fake, &kite()),
        Sent::Failed {
            reason: "Grok Build image generation failed: Grok Build's image tool failed".into(),
            cost: None,
            retryable: true,
        }
    );
}

#[test]
fn a_dropped_call_s_session_is_cancelled_then_deleted_before_grok_is_shut_down() {
    let fake = fake(|_| {
        let answered = json!({"jsonrpc": "2.0", "id": 5, "result": {"stopReason": "cancelled"}});
        format!(
            "{}{}{}{}{}",
            signed_in(),
            session(3, "s-a"),
            cancelled("s-a"),
            echo(answered),
            deleted("s-a", 6)
        )
    });
    let prompted = fake.marker("prompt-s-a");
    let ((), verified) = on_one_runtime(Ok(fake.program.clone()), |adapter| async move {
        let call = kite();
        // The run is cancelled while Grok Build draws: the send is dropped once it has the prompt.
        tokio::select! {
            sent = adapter.send(&call.call) => panic!("the call ended: {sent:?}"),
            seen = appears(&prompted) => assert!(seen, "Grok Build was never sent the prompt"),
        }
    });
    assert!(verified);
    assert_eq!(fake.lines("events"), ["cancelled", "deleted"]);
}
