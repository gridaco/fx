//! The Codex adapter (spec/providers.md §9.6) against fake Codex programs: shell scripts that speak
//! the app-server's side of the conversation. No real Codex is ever started.
#![cfg(unix)]

use base64::Engine;
use grida_fx_core::money::Usd;
use grida_fx_providers::adapter::{RequestAdapter, Sent};
use grida_fx_providers::codex::{self, CodexImages};
use grida_fx_providers::testing::{CallBuilder, TestCall, block_on, media};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

/// The fake's opening: it answers `initialize`, reads `initialized`, answers `thread/start`, then
/// checks the turn's text and answers `turn/start`. `after` runs once the turn is accepted.
const OPENING: &str = r#"#!/bin/sh
[ "$1" = app-server ] || exit 9
read -r line
echo '{"id":1,"result":{"userAgent":"fake"}}'
read -r line
read -r line
case "$line" in *'"sandbox":"read-only"'*'"ephemeral":true'*|*'"ephemeral":true'*'"sandbox":"read-only"'*) ;; *) exit 8 ;; esac
echo '{"id":2,"result":{"thread":{"id":"thread-a"}}}'
read -r line
case "$line" in *'<prompt>\na red kite\n</prompt>'*) ;; *) exit 7 ;; esac
"#;

const ACCEPTED: &str = r#"echo '{"id":3,"result":{"turn":{"id":"turn-a","status":"inProgress"}}}'
"#;

const COMPLETED: &str = r#"echo '{"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"completed","error":null}}}'
read -r line
"#;

struct Fake {
    _dir: tempfile::TempDir,
    program: PathBuf,
}

/// A fake Codex whose script is `body`.
fn fake(body: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("codex");
    std::fs::write(&program, body).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    Fake { _dir: dir, program }
}

fn image_item(item: Value) -> String {
    let line =
        json!({"method": "item/completed", "params": {"threadId": "thread-a", "item": item}});
    format!("echo '{line}'\n")
}

fn generated(bytes: &[u8]) -> String {
    image_item(json!({
        "type": "imageGeneration", "id": "ig-a", "status": "completed",
        "revisedPrompt": "a red kite", "savedPath": null,
        "result": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

fn call(request: Value) -> TestCall {
    CallBuilder::new("image.generate", "imagegen@codex")
        .contract(json!({"adapter": codex::ADAPTER, "adapter_behavior": "1"}))
        .request(request)
        .build()
}

fn send_with(fake: &Fake, deadline: Duration) -> (Sent, CodexImages, TestCall) {
    let adapter = CodexImages::with_program(Ok(fake.program.clone())).with_deadline(deadline);
    let call = call(json!({"prompt": "a red kite"}));
    let sent = block_on(adapter.send(&call.call));
    (sent, adapter, call)
}

fn send(fake: &Fake) -> Sent {
    send_with(fake, Duration::from_secs(20)).0
}

fn failed(reason: &str) -> Sent {
    Sent::Failed {
        reason: reason.into(),
        cost: Some(Usd::ZERO),
        retryable: false,
    }
}

#[test]
fn a_generated_image_is_the_answer_and_costs_nothing() {
    let png = media::png(4, 3, None);
    let script = format!("{OPENING}{ACCEPTED}{}{COMPLETED}", generated(&png));
    let (sent, adapter, call) = send_with(&fake(&script), Duration::from_secs(20));
    let Sent::Answered(answer) = sent else {
        panic!("{sent:?}");
    };
    assert_eq!(answer.cost, Some(Usd::ZERO));
    assert_eq!(answer.data, Value::Null);
    assert_eq!(answer.files["image"].kind, "image/png");
    assert_eq!(answer.files["image"].bytes, png);
    assert_eq!(adapter.check(&call.call, &answer), Ok(()));
}

#[test]
fn requests_from_codex_are_declined_and_the_turn_goes_on() {
    let png = media::png(2, 2, None);
    let ask = r#"echo '{"id":"ask-1","method":"item/commandExecution/requestApproval","params":{}}'
read -r line
case "$line" in *'"id":"ask-1"'*'-32601'*) ;; *) exit 6 ;; esac
"#;
    let script = format!("{OPENING}{ACCEPTED}{ask}{}{COMPLETED}", generated(&png));
    assert!(matches!(send(&fake(&script)), Sent::Answered(_)));
}

#[test]
fn a_usage_limit_ends_the_call_with_its_reset_time() {
    let item = image_item(json!({
        "type": "imageGeneration", "id": "ig-a", "status": "failed", "result": "",
        "failure": {"type": "usageLimitExceeded", "limitId": "codex", "resetsAt": 1792311096},
    }));
    let script = format!("{OPENING}{ACCEPTED}{item}{COMPLETED}");
    assert_eq!(
        send(&fake(&script)),
        failed(
            "Codex image generation failed: the Codex usage limit is reached; it resets at unix \
             time 1792311096"
        )
    );
}

#[test]
fn a_turn_without_an_image_fails_and_is_not_repeated() {
    let script = format!("{OPENING}{ACCEPTED}{COMPLETED}");
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: Codex answered without an image")
    );
    let errored = r#"echo '{"method":"turn/completed","params":{"turn":{"id":"turn-a","status":"failed","error":{"message":"You are not signed in"}}}}'
read -r line
"#;
    let script = format!("{OPENING}{ACCEPTED}{errored}");
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: You are not signed in")
    );
}

#[test]
fn the_check_holds_the_image_to_the_background_asked_for() {
    let adapter = CodexImages::with_program(Err(codex::NOT_INSTALLED.into()));
    let answer = |png: Vec<u8>| {
        grida_fx_providers::adapter::Answer::new(Value::Null, Some(Usd::ZERO)).with_file(
            "image",
            "image/png",
            png,
        )
    };
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
fn bytes_that_are_not_a_png_fail_the_check() {
    let script = format!(
        "{OPENING}{ACCEPTED}{}{COMPLETED}",
        generated(b"not a picture")
    );
    let (sent, adapter, call) = send_with(&fake(&script), Duration::from_secs(20));
    let Sent::Answered(answer) = sent else {
        panic!("{sent:?}");
    };
    assert!(adapter.check(&call.call, &answer).is_err());
    let script = format!(
        "{OPENING}{ACCEPTED}{}{COMPLETED}",
        image_item(
            json!({"type": "imageGeneration", "id": "ig-a", "status": "completed", "result": "%%%"})
        )
    );
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: Codex's image is not valid base64")
    );
}

#[test]
fn nothing_before_the_turn_is_received() {
    let exited = "#!/bin/sh\nread -r line\nexit 0\n";
    assert_eq!(
        send(&fake(exited)),
        Sent::not_received("Codex image generation was not sent: Codex exited")
    );
    let refused = format!(
        "{OPENING}echo '{{\"id\":3,\"error\":{{\"code\":-32600,\"message\":\"not signed in\"}}}}'\nread -r line\n"
    );
    assert_eq!(
        send(&fake(&refused)),
        Sent::not_received(
            "Codex image generation was not sent: Codex refused turn/start: not signed in"
        )
    );
    let noise = "#!/bin/sh\nread -r line\necho 'Welcome to Codex'\nread -r line\n";
    assert_eq!(
        send(&fake(noise)),
        Sent::not_received(
            "Codex image generation was not sent: Codex sent a line that is not JSON"
        )
    );
}

#[test]
fn exiting_during_the_turn_fails() {
    let script = format!("{OPENING}{ACCEPTED}exit 0\n");
    assert_eq!(
        send(&fake(&script)),
        failed("Codex image generation failed: Codex exited")
    );
}

#[test]
fn the_deadline_stops_a_turn_that_never_ends() {
    let script = format!("{OPENING}{ACCEPTED}sleep 30\n");
    let started = std::time::Instant::now();
    // Long enough for a busy machine to start the fake and accept the turn.
    let (sent, _, _) = send_with(&fake(&script), Duration::from_secs(3));
    assert_eq!(
        sent,
        failed("Codex image generation did not finish within 3 s")
    );
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "{:?}",
        started.elapsed()
    );
    let silent = "#!/bin/sh\nsleep 30\n";
    let (sent, _, _) = send_with(&fake(silent), Duration::from_secs(1));
    assert_eq!(
        sent,
        Sent::not_received("Codex image generation did not start a turn within 1 s")
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
        assert_eq!(
            block_on(adapter.send(&call(request).call)),
            Sent::Refused {
                reason: reason.into()
            }
        );
    }
    assert_eq!(
        block_on(adapter.send(&call(json!({"prompt": "p"})).call)),
        Sent::Refused {
            reason: codex::NOT_INSTALLED.into()
        }
    );
    let missing = CodexImages::with_program(Err(codex::NETWORK_OFF.into()));
    assert_eq!(
        block_on(missing.send(&call(json!({"prompt": "p"})).call)),
        Sent::Refused {
            reason: codex::NETWORK_OFF.into()
        }
    );
}
