//! The Codex adapter (spec/providers.md §9.6) against fake Codex programs: shell scripts that speak
//! the app-server's side of the conversation. No real Codex is ever started.
#![cfg(unix)]

use base64::Engine;
use grida_fx_providers::adapter::{Answer, RequestAdapter, Sent};
use grida_fx_providers::codex::{self, CodexImages};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Answers `codex mcp list --json` with one enabled and one disabled server; refuses anything
/// but an app-server whose settings turn off apps, plugins and the enabled server.
const LISTING: &str = r#"#!/bin/sh
case "$*" in
  *"features.apps=false"*"features.plugins=false"*"mcp list --json"*)
    echo '[{"name":"acme-docs","enabled":true},{"name":"acme-off","enabled":false}]'; exit 0 ;;
esac
case "$1" in app-server) ;; *) exit 9 ;; esac
case "$*" in *"features.apps=false"*"features.plugins=false"*"mcp_servers.acme-docs.enabled=false"*) ;; *) exit 9 ;; esac
case "$*" in *acme-off*) exit 9 ;; esac
"#;

/// `initialize`, then `initialized`, then `account/read` (a ChatGPT sign-in).
const SIGNED_IN: &str = r#"read -r line
echo '{"id":1,"result":{"userAgent":"fake"}}'
read -r line
read -r line
echo '{"id":2,"result":{"account":{"type":"chatgpt","planType":"plus"},"requiresOpenaiAuth":true}}'
"#;

/// `thread/start`, checked for its sandbox and approvals; then reads `turn/start`, checked for the
/// prompt and the pinned effort and tier.
const THREAD: &str = r#"read -r line
case "$line" in *'"sandbox":"read-only"'*) ;; *) exit 8 ;; esac
case "$line" in *'"approvalPolicy":"never"'*) ;; *) exit 8 ;; esac
case "$line" in *'"ephemeral":true'*) ;; *) exit 8 ;; esac
echo '{"id":3,"result":{"thread":{"id":"thread-a"},"model":"m","modelProvider":"openai"}}'
read -r line
case "$line" in *'<prompt>\na red kite\n</prompt>'*'"effort":"low","serviceTierForTurn":"default"'*) ;; *) exit 7 ;; esac
"#;

const ACCEPTED: &str = r#"echo '{"id":4,"result":{"turn":{"id":"turn-a","status":"inProgress"}}}'
"#;

const COMPLETED: &str = r#"echo '{"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"completed","error":null}}}'
read -r line
"#;

/// Everything up to and including Codex accepting the turn.
fn opening() -> String {
    format!("{LISTING}{SIGNED_IN}{THREAD}{ACCEPTED}")
}

struct Fake {
    dir: tempfile::TempDir,
    program: PathBuf,
}

impl Fake {
    fn marker(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// A fake Codex whose script is `body`.
fn fake(body: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("codex");
    std::fs::write(&program, body).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    Fake { dir, program }
}

fn echo(message: Value) -> String {
    format!("echo '{message}'\n")
}

fn item(thread: &str, item: Value) -> String {
    echo(json!({"method": "item/completed",
        "params": {"threadId": thread, "turnId": "turn-a", "item": item}}))
}

fn generated(bytes: &[u8]) -> String {
    item(
        "thread-a",
        json!({
            "type": "imageGeneration", "id": "ig-a", "status": "completed",
            "revisedPrompt": "a red kite", "savedPath": null,
            "result": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    )
}

fn call(request: Value) -> TestCall {
    CallBuilder::new("image.generate", "imagegen@codex")
        .contract(json!({"adapter": codex::ADAPTER, "adapter_behavior": "1"}))
        .request(request)
        .build()
}

fn adapter_with(fake: &Fake, start: u64, whole: u64) -> CodexImages {
    CodexImages::with_program(Ok(fake.program.clone()))
        .with_deadlines(Duration::from_secs(start), Duration::from_secs(whole))
}

fn send_with(adapter: &CodexImages) -> (Sent, TestCall) {
    let call = call(json!({"prompt": "a red kite"}));
    let sent = block_on(adapter.send(&call.call));
    (sent, call)
}

fn send(fake: &Fake) -> Sent {
    send_with(&adapter_with(fake, 20, 30)).0
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
fn a_generated_image_is_the_answer_and_reports_no_cost() {
    let png = media::png(4, 3, None);
    let fake = fake(&format!("{}{}{COMPLETED}", opening(), generated(&png)));
    let adapter = adapter_with(&fake, 20, 30);
    let (sent, call) = send_with(&adapter);
    let answer = answered(sent);
    assert_eq!(
        answer.cost, None,
        "Codex reports no cost: the route's hold is charged"
    );
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, png);
    assert_eq!(adapter.check(&call.call, &answer), Ok(()));
}

#[test]
fn the_rest_of_the_turn_is_interrupted_once_the_image_arrives() {
    let png = media::png(2, 2, None);
    let wait = r#"read -r line
case "$line" in *'"method":"turn/interrupt"'*'"turnId":"turn-a"'*) touch "$(dirname "$0")/interrupted" ;; esac
read -r line
"#;
    let fake = fake(&format!("{}{}{wait}", opening(), generated(&png)));
    assert!(matches!(send(&fake), Sent::Answered(_)));
    assert!(fake.marker("interrupted").exists());
}

#[test]
fn only_this_thread_and_turn_count() {
    let png = media::png(2, 2, None);
    let other_image = item(
        "thread-sub",
        json!({"type": "imageGeneration", "id": "ig-sub", "status": "completed", "result": "%%%"}),
    );
    let other_turn = echo(
        json!({"method": "turn/completed", "params": {"threadId": "thread-sub",
        "turn": {"id": "turn-sub", "status": "completed", "error": null}}}),
    );
    let script = format!(
        "{}{other_image}{other_turn}{}{COMPLETED}",
        opening(),
        generated(&png)
    );
    assert_eq!(answered(send(&fake(&script))).files["image"].bytes, png);
}

#[test]
fn notifications_before_the_turn_s_reply_are_kept() {
    let png = media::png(2, 2, None);
    let script = format!(
        "{LISTING}{SIGNED_IN}{THREAD}{}{ACCEPTED}{COMPLETED}",
        generated(&png)
    );
    assert_eq!(answered(send(&fake(&script))).files["image"].bytes, png);
}

#[test]
fn the_first_good_image_wins_over_a_failed_one() {
    let png = media::png(2, 2, None);
    let bad = item(
        "thread-a",
        json!({"type": "imageGeneration", "id": "ig-0", "status": "failed", "result": "",
            "failure": {"type": "contentPolicy"}}),
    );
    let script = format!("{}{bad}{}{COMPLETED}", opening(), generated(&png));
    assert_eq!(answered(send(&fake(&script))).files["image"].bytes, png);
}

#[test]
fn requests_from_codex_are_declined_and_the_turn_goes_on() {
    let png = media::png(2, 2, None);
    let ask = r#"echo '{"id":"ask-1","method":"item/commandExecution/requestApproval","params":{}}'
read -r line
case "$line" in *'"id":"ask-1"'*'-32601'*) ;; *) exit 6 ;; esac
"#;
    let script = format!("{}{ask}{}{COMPLETED}", opening(), generated(&png));
    assert!(matches!(send(&fake(&script)), Sent::Answered(_)));
}

#[test]
fn only_a_chatgpt_sign_in_may_draw() {
    let account = |result: Value| {
        format!(
            "{LISTING}read -r line\necho '{{\"id\":1,\"result\":{{}}}}'\nread -r line\nread -r \
             line\n{}read -r line\n",
            echo(json!({"id": 2, "result": result}))
        )
    };
    assert_eq!(
        send(&fake(&account(
            json!({"account": null, "requiresOpenaiAuth": true})
        ))),
        refused("Codex is not signed in: run codex login and sign in with ChatGPT")
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
        json!({"type": "imageGeneration", "id": "ig-a", "status": "failed", "result": "",
            "failure": {"type": "usageLimitExceeded", "limitId": "codex", "resetsAt": 1792311096}}),
    );
    let script = format!("{}{limited}{COMPLETED}", opening());
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
    let script = format!("{}{COMPLETED}", opening());
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
    let fake_png = fake(&format!(
        "{}{}{COMPLETED}",
        opening(),
        generated(b"not a picture")
    ));
    let adapter = adapter_with(&fake_png, 20, 30);
    let (sent, call) = send_with(&adapter);
    assert!(adapter.check(&call.call, &answered(sent)).is_err());
    let garbled = item(
        "thread-a",
        json!({"type": "imageGeneration", "id": "ig-a", "status": "completed", "result": "%%%"}),
    );
    let script = format!("{}{garbled}{COMPLETED}", opening());
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
    let refused_turn = format!(
        "{LISTING}{SIGNED_IN}{THREAD}{}read -r line\n",
        echo(json!({"id": 4, "error": {"code": -32600, "message": "/Users/someone/x"}}))
    );
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
fn deadlines_stop_codex_before_and_after_the_turn_is_asked_for() {
    let quick = |fake: &Fake, start: u64, whole: u64| {
        let started = Instant::now();
        let sent = send_with(&adapter_with(fake, start, whole)).0;
        assert!(
            started.elapsed() < Duration::from_secs(whole + 8),
            "{:?}",
            started.elapsed()
        );
        sent
    };
    let before = fake(&format!("{LISTING}exec sleep 30\n"));
    assert_eq!(
        quick(&before, 1, 10),
        failed("Codex image generation was not started: Codex did not start the turn within 1 s")
    );
    // turn/start was written and never answered: the turn may be running.
    let unanswered = fake(&format!("{LISTING}{SIGNED_IN}{THREAD}exec sleep 30\n"));
    assert_eq!(
        quick(&unanswered, 2, 10),
        failed("Codex image generation failed: Codex did not start the turn within 2 s")
    );
    // Long enough for a busy machine to start the fake and accept the turn.
    let running = fake(&format!("{}exec sleep 30\n", opening()));
    assert_eq!(
        quick(&running, 5, 3),
        failed("Codex image generation failed: Codex did not finish within 3 s")
    );
}

#[test]
fn nothing_codex_started_survives_it() {
    let png = media::png(2, 2, None);
    let helper = r#"sleep 30 &
echo $! > "$(dirname "$0")/helper.pid"
"#;
    let finished = fake(&format!(
        "{}{helper}{}{COMPLETED}",
        opening(),
        generated(&png)
    ));
    assert!(matches!(send(&finished), Sent::Answered(_)));
    assert!(
        gone(&finished.marker("helper.pid")),
        "a helper in Codex's group outlived it"
    );
    // The same when the deadline ends the conversation.
    let stalled = fake(&format!("{}{helper}exec sleep 30\n", opening()));
    assert!(matches!(
        send_with(&adapter_with(&stalled, 5, 3)).0,
        Sent::Failed { .. }
    ));
    assert!(gone(&stalled.marker("helper.pid")));
}

#[test]
fn the_check_holds_the_image_to_the_background_asked_for() {
    let adapter = CodexImages::with_program(Err(codex::NOT_INSTALLED.into()));
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
    let adapter = CodexImages::with_program(Ok(PathBuf::from("/nonexistent/codex")));
    for (request, reason) in [
        (
            json!({"prompt": "p", "size": "1024x1024"}),
            codex::NO_EXACT_SIZE,
        ),
        (
            json!({"prompt": "p", "references": [{"file": "a"}]}),
            "Codex image generation takes no references",
        ),
        (
            json!({"prompt": "p", "seed": 1}),
            "image.generate takes no member seed",
        ),
    ] {
        assert_eq!(block_on(adapter.send(&call(request).call)), refused(reason));
    }
    let missing = CodexImages::with_program(Err(codex::NETWORK_OFF.into()));
    assert_eq!(
        block_on(missing.send(&call(json!({"prompt": "p"})).call)),
        refused(codex::NETWORK_OFF)
    );
    // The program vanished after it was found.
    assert_eq!(
        block_on(adapter.send(&call(json!({"prompt": "p"})).call)),
        refused("Codex's MCP servers could not be listed, so they cannot be turned off")
    );
}
