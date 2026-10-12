//! The Codex adapter and runtime (spec/providers.md §9.6) against fake Codex programs: shell
//! scripts that speak the app-server's side of the conversation. No real Codex is ever started.
//!
//! The runtime's reader and writer run on the tokio runtime that started them, so each test runs
//! its calls, and the shutdown, on one runtime ([`on_one_runtime`]).
#![cfg(unix)]

use base64::Engine;
use grida_fx_providers::adapter::{Answer, Billing, RequestAdapter, Runtime, Sent};
use grida_fx_providers::codex::{self, CodexImages, CodexRuntime};
use grida_fx_providers::testing::{self, CallBuilder, TestCall, media};
use serde_json::{Value, json};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Counts each start in `starts` (`mcp` or `app-server`), answers `codex mcp list --json` with one
/// enabled and one disabled server, and refuses anything but an app-server whose settings turn
/// off apps, plugins and the enabled server.
const LISTING: &str = r#"#!/bin/sh
here="$(dirname "$0")"
case "$*" in
  *"features.apps=false"*"features.plugins=false"*"mcp list --json"*)
    echo mcp >> "$here/starts"
    echo '[{"name":"acme-docs","enabled":true},{"name":"acme-off","enabled":false}]'; exit 0 ;;
esac
case "$1" in app-server) ;; *) exit 9 ;; esac
case "$*" in *"features.apps=false"*"features.plugins=false"*"mcp_servers.acme-docs.enabled=false"*) ;; *) exit 9 ;; esac
case "$*" in *acme-off*) exit 9 ;; esac
echo app-server >> "$here/starts"
"#;

/// `initialize`, then `initialized`, then `account/read` (a ChatGPT sign-in).
const SIGNED_IN: &str = r#"read -r line
echo '{"id":1,"result":{"userAgent":"grida-fx/0.160.1 (Fake OS)"}}'
read -r line
read -r line
echo '{"id":2,"result":{"account":{"type":"chatgpt","planType":"plus"},"requiresOpenaiAuth":true}}'
"#;

/// A call's `thread/start` (answered with id `thread`), checked for its sandbox and approvals;
/// then its `turn/start`, checked for the prompt and the pinned effort and tier, and accepted.
fn call_script(thread_id: u64, thread: &str, turn: &str) -> String {
    format!(
        r#"read -r line
case "$line" in *'"sandbox":"read-only"'*) ;; *) exit 8 ;; esac
case "$line" in *'"approvalPolicy":"never"'*) ;; *) exit 8 ;; esac
case "$line" in *'"ephemeral":true'*) ;; *) exit 8 ;; esac
echo '{{"id":{thread_id},"result":{{"thread":{{"id":"{thread}"}},"model":"agent-5.5","modelProvider":"openai"}}}}'
read -r line
case "$line" in *'<prompt>\na red kite\n</prompt>'*'"effort":"low","serviceTierForTurn":"default"'*) ;; *) exit 7 ;; esac
echo '{{"id":{},"result":{{"turn":{{"id":"{turn}","status":"inProgress"}}}}}}'
"#,
        thread_id + 1
    )
}

/// The first call's turn completes (after the client's `turn/interrupt`, read here).
fn completed(thread: &str, turn: &str) -> String {
    echo(
        json!({"method": "turn/completed", "params": {"threadId": thread,
        "turn": {"id": turn, "status": "completed", "error": null}}}),
    ) + "read -r line\n"
}

/// Everything up to and including Codex accepting the first call's turn.
fn opening() -> String {
    format!(
        "{LISTING}{SIGNED_IN}{}",
        call_script(3, "thread-a", "turn-a")
    )
}

fn completed_a() -> String {
    completed("thread-a", "turn-a")
}

/// Everything up to a second call: the first call's turn draws `png` and completes. The second
/// call's requests have ids 5 and 6 ([`call_script`]).
fn first_image(png: &[u8]) -> String {
    format!(
        "{}{}{}",
        opening(),
        image_item("thread-a", "turn-a", png),
        completed_a()
    )
}

struct Fake {
    dir: tempfile::TempDir,
    program: PathBuf,
}

impl Fake {
    fn marker(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The starts the fake counted: `mcp` and `app-server` lines.
    fn starts(&self) -> Vec<String> {
        std::fs::read_to_string(self.marker("starts"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// A fake Codex whose script is `body`.
fn fake(body: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("codex");
    testing::write_program(&program, body);
    Fake { dir, program }
}

fn echo(message: Value) -> String {
    format!("echo '{message}'\n")
}

fn item(thread: &str, turn: &str, item: Value) -> String {
    echo(json!({"method": "item/completed",
        "params": {"threadId": thread, "turnId": turn, "item": item}}))
}

fn image_item(thread: &str, turn: &str, bytes: &[u8]) -> String {
    item(
        thread,
        turn,
        json!({
            "type": "imageGeneration", "id": "ig-a", "status": "completed",
            "revisedPrompt": "a red kite", "savedPath": null,
            "result": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    )
}

fn generated(bytes: &[u8]) -> String {
    image_item("thread-a", "turn-a", bytes)
}

fn call(request: Value) -> TestCall {
    CallBuilder::new("image.generate", "imagegen@codex")
        .contract(json!({"adapter": codex::ADAPTER, "adapter_behavior": "1"}))
        .request(request)
        .build()
}

fn kite() -> TestCall {
    call(json!({"prompt": "a red kite"}))
}

/// A runtime over `program`, and an adapter over it with these deadlines.
fn images(
    program: Result<PathBuf, String>,
    start: u64,
    whole: u64,
) -> (Arc<CodexRuntime>, CodexImages) {
    let runtime = Arc::new(CodexRuntime::new(program));
    let adapter = CodexImages::new(Arc::clone(&runtime))
        .with_deadlines(Duration::from_secs(start), Duration::from_secs(whole));
    (runtime, adapter)
}

/// Runs `test` on one tokio runtime, then shuts the Codex runtime down, as the engine does after
/// a run; returns what `test` returned and whether the shutdown verified.
fn on_one_runtime<T, F: Future<Output = T>>(
    program: Result<PathBuf, String>,
    start: u64,
    whole: u64,
    test: impl FnOnce(CodexImages) -> F,
) -> (T, bool) {
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio.block_on(async {
        let (runtime, adapter) = images(program, start, whole);
        let out = test(adapter).await;
        let verified = runtime.shutdown().await;
        (out, verified)
    })
}

/// As [`on_one_runtime`], on a Codex already started: a first call, with deadlines no busy
/// machine misses, draws the image of [`first_image`]; then `test` gets an adapter on the same
/// runtime whose deadlines are `start` and `whole`. They time the call under test only, never how
/// long the machine took to start the fake.
fn on_a_started_codex<T, F: Future<Output = T>>(
    fake: &Fake,
    start: u64,
    whole: u64,
    test: impl FnOnce(CodexImages) -> F,
) -> (T, bool) {
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio.block_on(async {
        let (runtime, first) = images(Ok(fake.program.clone()), 20, 30);
        answered(first.send(&kite().call).await);
        let adapter = CodexImages::new(Arc::clone(&runtime))
            .with_deadlines(Duration::from_secs(start), Duration::from_secs(whole));
        let out = test(adapter).await;
        let verified = runtime.shutdown().await;
        (out, verified)
    })
}

/// One call to `fake`, and the runtime shut down.
fn send(fake: &Fake) -> Sent {
    on_one_runtime(Ok(fake.program.clone()), 20, 30, |adapter| async move {
        adapter.send(&kite().call).await
    })
    .0
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

fn answered(sent: Sent) -> Answer {
    match sent {
        Sent::Answered(answer) => answer,
        other => panic!("{other:?}"),
    }
}

/// Whether the process whose pid a fake wrote to `pid_file` is gone, waiting up to 3 s.
fn gone(pid_file: &Path) -> bool {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn a_generated_image_is_the_answer_with_codex_s_usage_and_no_cost() {
    let png = media::png(4, 3, None);
    let usage = echo(
        json!({"method": "thread/tokenUsage/updated", "params": {"threadId": "thread-a",
        "turnId": "turn-a", "tokenUsage": {"total": {"inputTokens": 17393, "cachedInputTokens": 0,
            "outputTokens": 72}}}}),
    );
    let fake = fake(&format!(
        "{}{}{usage}{}",
        opening(),
        generated(&png),
        completed_a()
    ));
    let call = kite();
    let ((sent, checked), verified) =
        on_one_runtime(Ok(fake.program.clone()), 20, 30, |adapter| async move {
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
        "Codex reports no cost: the route's hold is charged"
    );
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, png);
    let usage = answer.usage.expect("Codex's usage");
    assert_eq!(
        usage.to_value(Billing::Plan),
        json!({"billing": "plan", "runtime": "codex 0.160.1", "agent_model": "agent-5.5",
            "input_tokens": 17393, "cached_input_tokens": 0, "output_tokens": 72})
    );
}

#[test]
fn one_codex_serves_every_call_of_an_invocation() {
    let (png_a, png_b) = (media::png(2, 2, None), media::png(3, 3, None));
    let script = format!(
        "{}{}{}{}{}read -r line\n",
        opening(),
        generated(&png_a),
        completed_a(),
        call_script(5, "thread-b", "turn-b").replace("thread-a", "thread-b"),
        image_item("thread-b", "turn-b", &png_b) + &completed("thread-b", "turn-b"),
    );
    let fake = fake(&script);
    let (bytes, verified) =
        on_one_runtime(Ok(fake.program.clone()), 20, 30, |adapter| async move {
            let first = answered(adapter.send(&kite().call).await);
            let second = answered(adapter.send(&kite().call).await);
            (
                first.files["image"].bytes.clone(),
                second.files["image"].bytes.clone(),
            )
        });
    assert_eq!(bytes, (png_a, png_b));
    assert!(verified);
    assert_eq!(
        fake.starts(),
        ["mcp", "app-server"],
        "listed once, started once"
    );
}

#[test]
fn the_rest_of_the_turn_is_interrupted_once_the_image_arrives() {
    let png = media::png(2, 2, None);
    let wait = r#"read -r line
case "$line" in *'"method":"turn/interrupt"'*'"turnId":"turn-a"'*) touch "$(dirname "$0")/interrupted" ;; esac
echo '{"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"interrupted","error":null}}}'
read -r line
"#;
    let fake = fake(&format!("{}{}{wait}", opening(), generated(&png)));
    assert!(matches!(send(&fake), Sent::Answered(_)));
    assert!(fake.marker("interrupted").exists());
}

#[test]
fn a_dropped_call_interrupts_its_turn() {
    let wait = r#"read -r line
case "$line" in *'"method":"turn/interrupt"'*'"turnId":"turn-b"'*) touch "$(dirname "$0")/interrupted" ;; esac
read -r line
"#;
    let png = media::png(2, 2, None);
    let script = first_image(&png) + &call_script(5, "thread-b", "turn-b") + wait;
    let fake = fake(&script);
    let marker = fake.marker("interrupted");
    let (_, verified) = on_a_started_codex(&fake, 20, 30, |adapter| async move {
        // The run is cancelled while the turn is drawing: the call's future is dropped.
        let dropped =
            tokio::time::timeout(Duration::from_secs(2), adapter.send(&kite().call)).await;
        assert!(dropped.is_err());
        let started = Instant::now();
        while !marker.exists() && started.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    assert!(
        fake.marker("interrupted").exists(),
        "the turn was not interrupted"
    );
    assert!(verified);
}

#[test]
fn only_this_thread_and_turn_count() {
    let png = media::png(2, 2, None);
    let other_image = item(
        "thread-sub",
        "turn-sub",
        json!({"type": "imageGeneration", "id": "ig-sub", "status": "completed", "result": "%%%"}),
    );
    let other_turn = echo(
        json!({"method": "turn/completed", "params": {"threadId": "thread-sub",
        "turn": {"id": "turn-sub", "status": "completed", "error": null}}}),
    );
    let script = format!(
        "{}{other_image}{other_turn}{}{}",
        opening(),
        generated(&png),
        completed_a()
    );
    assert_eq!(answered(send(&fake(&script))).files["image"].bytes, png);
}

#[test]
fn notifications_before_the_turn_s_reply_are_kept() {
    let png = media::png(2, 2, None);
    // The image arrives between turn/start and its response.
    let accepted =
        "echo '{\"id\":4,\"result\":{\"turn\":{\"id\":\"turn-a\",\"status\":\"inProgress\"}}}'\n";
    let opening = opening().replace(accepted, &(generated(&png) + accepted));
    assert_ne!(opening, self::opening());
    let script = format!("{opening}{}", completed_a());
    assert_eq!(answered(send(&fake(&script))).files["image"].bytes, png);
}

#[test]
fn the_first_good_image_wins_over_a_failed_one() {
    let png = media::png(2, 2, None);
    let bad = item(
        "thread-a",
        "turn-a",
        json!({"type": "imageGeneration", "id": "ig-0", "status": "failed", "result": "",
            "failure": {"type": "contentPolicy"}}),
    );
    let script = format!("{}{bad}{}{}", opening(), generated(&png), completed_a());
    assert_eq!(answered(send(&fake(&script))).files["image"].bytes, png);
}

#[test]
fn requests_from_codex_are_declined_and_the_turn_goes_on() {
    let png = media::png(2, 2, None);
    let ask = r#"echo '{"id":"ask-1","method":"item/commandExecution/requestApproval","params":{}}'
read -r line
case "$line" in *'"id":"ask-1"'*'-32601'*) ;; *) exit 6 ;; esac
"#;
    let script = format!("{}{ask}{}{}", opening(), generated(&png), completed_a());
    assert!(matches!(send(&fake(&script)), Sent::Answered(_)));
}

#[test]
fn only_a_chatgpt_sign_in_may_draw_and_a_refusal_holds_for_the_invocation() {
    let account = |result: Value| {
        format!(
            "{LISTING}read -r line\necho '{{\"id\":1,\"result\":{{}}}}'\nread -r line\nread -r \
             line\n{}read -r line\n",
            echo(json!({"id": 2, "result": result}))
        )
    };
    let signed_out = fake(&account(
        json!({"account": null, "requiresOpenaiAuth": true}),
    ));
    let (sent, verified) = on_one_runtime(
        Ok(signed_out.program.clone()),
        20,
        30,
        |adapter| async move {
            let first = adapter.send(&kite().call).await;
            let second = adapter.send(&kite().call).await;
            assert_eq!(first, second);
            first
        },
    );
    assert_eq!(
        sent,
        refused("Codex is not signed in: run codex login and sign in with ChatGPT")
    );
    assert!(verified);
    assert_eq!(
        signed_out.starts(),
        ["mcp", "app-server"],
        "not asked again"
    );
    assert_eq!(
        send(&fake(&account(
            json!({"account": {"type": "apiKey"}, "requiresOpenaiAuth": true})
        ))),
        refused(
            "Codex is signed in with an API key, which bills an API account: run codex login \
             and sign in with ChatGPT"
        )
    );
    let other_provider = format!(
        "{LISTING}{SIGNED_IN}read -r line\n{}read -r line\n",
        echo(json!({"id": 3, "result": {"thread": {"id": "thread-a"}, "modelProvider": "ollama"}}))
    );
    assert_eq!(
        send(&fake(&other_provider)),
        refused(
            "Codex uses the model provider ollama, not OpenAI: image generation needs Codex \
             signed in with ChatGPT"
        )
    );
}

#[test]
fn a_codex_that_exited_is_replaced_by_the_next_call() {
    let png = media::png(2, 2, None);
    // The first Codex exits during the first call's turn; the second serves the second call.
    let script = format!(
        r#"{LISTING}if [ "$(grep -c app-server "$(dirname "$0")/starts")" = 1 ]; then
{}exit 0
fi
{}{}{}"#,
        opening().trim_start_matches(LISTING),
        opening().trim_start_matches(LISTING),
        generated(&png),
        completed_a()
    );
    let fake = fake(&script);
    let (sent, _) = on_one_runtime(Ok(fake.program.clone()), 20, 30, |adapter| async move {
        let first = adapter.send(&kite().call).await;
        let second = adapter.send(&kite().call).await;
        (first, second)
    });
    assert_eq!(
        sent.0,
        failed("Codex image generation failed: Codex exited")
    );
    assert!(matches!(sent.1, Sent::Answered(_)), "{:?}", sent.1);
    assert_eq!(fake.starts(), ["mcp", "app-server", "mcp", "app-server"]);
}

#[test]
fn servers_that_cannot_be_listed_are_a_refusal() {
    assert_eq!(
        send(&fake("#!/bin/sh\nexit 1\n")),
        refused("Codex's MCP servers could not be listed, so they cannot be turned off")
    );
}

#[test]
fn a_usage_limit_ends_the_call_with_its_reset_time() {
    let limited = item(
        "thread-a",
        "turn-a",
        json!({"type": "imageGeneration", "id": "ig-a", "status": "failed", "result": "",
            "failure": {"type": "usageLimitExceeded", "limitId": "codex", "resetsAt": 1792311096}}),
    );
    let script = format!("{}{limited}{}", opening(), completed_a());
    assert_eq!(
        send(&fake(&script)),
        failed(
            "Codex image generation failed: the Codex usage limit is reached; it resets at unix \
             time 1792311096"
        )
    );
}

#[test]
fn a_turn_without_an_image_fails_with_codex_s_error_kind_only() {
    let script = format!("{}{}", opening(), completed_a());
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: Codex answered without an image")
    );
    let errored = echo(
        json!({"method": "turn/completed", "params": {"threadId": "thread-a",
        "turn": {"id": "turn-a", "status": "failed",
            "error": {"message": "see /Users/someone/.codex/config.toml",
                "codexErrorInfo": "serverOverloaded"}}}}),
    );
    let script = format!("{}{errored}read -r line\n", opening());
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: Codex's turn failed (serverOverloaded)")
    );
}

#[test]
fn bytes_that_are_not_a_png_fail_the_check() {
    let not_png = fake(&format!(
        "{}{}{}",
        opening(),
        generated(b"not a picture"),
        completed_a()
    ));
    let call = kite();
    let (checked, _) = on_one_runtime(Ok(not_png.program.clone()), 20, 30, |adapter| async move {
        let answer = answered(adapter.send(&call.call).await);
        adapter.check(&call.call, &answer)
    });
    assert!(checked.is_err());
    let garbled = item(
        "thread-a",
        "turn-a",
        json!({"type": "imageGeneration", "id": "ig-a", "status": "completed", "result": "%%%"}),
    );
    let script = format!("{}{garbled}{}", opening(), completed_a());
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: Codex's image is not valid base64")
    );
}

#[test]
fn nothing_is_ever_resent() {
    // Before the turn: Codex exits, or talks nonsense.
    assert_eq!(
        send(&fake(&format!("{LISTING}read -r line\nexit 0\n"))),
        failed("Codex image generation was not started: Codex exited")
    );
    assert_eq!(
        send(&fake(&format!(
            "{LISTING}read -r line\necho 'Welcome to Codex'\nread -r line\n"
        ))),
        failed("Codex image generation was not started: Codex sent a line that is not JSON")
    );
    // Once turn/start is written, a generation may have started.
    let accepted =
        "echo '{\"id\":4,\"result\":{\"turn\":{\"id\":\"turn-a\",\"status\":\"inProgress\"}}}'\n";
    let refused_turn = opening().replace(
        accepted,
        &echo(json!({"id": 4, "error": {"code": -32600, "message": "/Users/someone/x"}})),
    ) + "read -r line\n";
    assert_eq!(
        send(&fake(&refused_turn)),
        failed("Codex image generation failed: Codex refused turn/start (error -32600)")
    );
    assert_eq!(
        send(&fake(&format!("{}exit 0\n", opening()))),
        failed("Codex image generation failed: Codex exited")
    );
}

#[test]
fn deadlines_stop_a_call_before_and_after_its_turn_is_asked_for() {
    // The call ends at its deadline, not long after.
    let timed = |whole: u64| {
        move |adapter: CodexImages| async move {
            let began = Instant::now();
            let sent = adapter.send(&kite().call).await;
            assert!(
                began.elapsed() < Duration::from_secs(whole + 10),
                "{:?}",
                began.elapsed()
            );
            sent
        }
    };
    let before = fake(&format!("{LISTING}exec sleep 30\n"));
    assert_eq!(
        on_one_runtime(Ok(before.program.clone()), 1, 10, timed(10)).0,
        failed("Codex image generation was not started: Codex did not start the turn within 1 s")
    );
    // The second call's turn/start was written and never answered: the turn may be running.
    let png = media::png(2, 2, None);
    let accepted =
        "echo '{\"id\":6,\"result\":{\"turn\":{\"id\":\"turn-b\",\"status\":\"inProgress\"}}}'\n";
    let second = call_script(5, "thread-b", "turn-b");
    assert!(second.contains(accepted));
    let unanswered = fake(&format!(
        "{}{}exec sleep 30\n",
        first_image(&png),
        second.replace(accepted, "")
    ));
    assert_eq!(
        on_a_started_codex(&unanswered, 3, 15, timed(15)).0,
        failed("Codex image generation failed: Codex did not start the turn within 3 s")
    );
    let running = fake(&format!("{}{second}exec sleep 30\n", first_image(&png)));
    assert_eq!(
        on_a_started_codex(&running, 3, 2, timed(2)).0,
        failed("Codex image generation failed: Codex did not finish within 2 s")
    );
}

#[test]
fn nothing_codex_started_survives_the_shutdown() {
    let png = media::png(2, 2, None);
    let helper = r#"sleep 30 &
echo $! > "$(dirname "$0")/helper.pid"
"#;
    let finished = fake(&format!(
        "{}{helper}{}{}",
        opening(),
        generated(&png),
        completed_a()
    ));
    assert!(matches!(send(&finished), Sent::Answered(_)));
    assert!(
        gone(&finished.marker("helper.pid")),
        "a helper in Codex's group outlived it"
    );
    // The same when a deadline ended a call and Codex kept running until the shutdown. The helper
    // starts in the first call, so it is there whatever the deadline.
    let stalled = fake(&format!(
        "{}{helper}{}{}{}exec sleep 30\n",
        opening(),
        generated(&png),
        completed_a(),
        call_script(5, "thread-b", "turn-b")
    ));
    let (sent, verified) = on_a_started_codex(&stalled, 3, 2, |adapter| async move {
        adapter.send(&kite().call).await
    });
    assert!(matches!(sent, Sent::Failed { .. }));
    assert!(verified);
    assert!(gone(&stalled.marker("helper.pid")));
}

#[test]
fn the_check_holds_the_image_to_the_background_asked_for() {
    let (_, adapter) = images(Err(codex::NOT_INSTALLED.into()), 20, 30);
    let answer = |png: Vec<u8>| Answer::new(Value::Null, None).with_file("image", "image/png", png);
    let asked = call(json!({"prompt": "p", "size": "auto", "background": "transparent"}));
    assert_eq!(
        adapter.check(&asked.call, &answer(media::png(32, 16, Some(0)))),
        Ok(())
    );
    assert_eq!(
        adapter.check(&asked.call, &answer(media::png(32, 16, None))),
        Err("the picture asked for as transparent has no alpha channel".into())
    );
}

#[test]
fn what_codex_cannot_honour_is_refused_before_it_starts() {
    let (sent, verified) = on_one_runtime(
        Ok(PathBuf::from("/nonexistent/codex")),
        20,
        30,
        |adapter| async move {
            let mut sent = Vec::new();
            for request in [
                json!({"prompt": "p", "size": "1024x1024"}),
                json!({"prompt": "p", "references": [{"file": "a"}]}),
                json!({"prompt": "p", "seed": 1}),
                // The program vanished after it was found.
                json!({"prompt": "p"}),
            ] {
                sent.push(adapter.send(&call(request).call).await);
            }
            sent
        },
    );
    assert!(verified);
    assert_eq!(
        sent,
        [
            refused(codex::NO_EXACT_SIZE),
            refused("Codex image generation takes no references"),
            refused("image.generate takes no member seed"),
            refused("Codex's MCP servers could not be listed, so they cannot be turned off"),
        ]
    );
    let (sent, _) = on_one_runtime(
        Err(codex::NETWORK_OFF.into()),
        20,
        30,
        |adapter| async move { adapter.send(&kite().call).await },
    );
    assert_eq!(sent, refused(codex::NETWORK_OFF));
}
