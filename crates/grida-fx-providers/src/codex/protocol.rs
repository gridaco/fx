//! What `codex app-server` says, read (spec/providers.md §9.6): JSON-RPC 2.0 messages, the
//! account, image items, turn failures and token usage. Reasons are fixed sentences with bounded
//! tokens from Codex's closed vocabularies, never Codex's own text, which can name private paths
//! (spec/providers.md §8).

use super::Stop;
use crate::adapter::Usage;
use crate::wire;
use serde_json::Value;

/// The longest line Codex may send (an image arrives base64 on one line).
pub(super) const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// The reasoning effort and service tier of every turn: the user's own settings (a high effort,
/// a priority tier) would spend their plan faster for nothing an image needs.
pub(super) const EFFORT: &str = "low";
pub(super) const SERVICE_TIER: &str = "default";

/// What one line from Codex is.
pub(super) enum Message {
    Response {
        id: u64,
        /// The error's code when Codex answered with an error.
        result: Result<Value, i64>,
    },
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
    },
}

/// One line's message; `None` for anything else JSON-RPC allows. An error response that names
/// no request means Codex could not read one of ours.
pub(super) fn message_of(value: Value) -> Result<Option<Message>, Stop> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = value.get("id").filter(|id| !id.is_null()).cloned();
    Ok(match (method, id) {
        (Some(_), Some(id)) => Some(Message::Request { id }),
        (Some(method), None) => Some(Message::Notification {
            method,
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(id)) => id.as_u64().map(|id| Message::Response {
            id,
            result: match value.get("error") {
                Some(error) => Err(error.get("code").and_then(Value::as_i64).unwrap_or(0)),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            },
        }),
        (None, None) if value.get("error").is_some() => {
            return Err(Stop::Failed("Codex could not read a request".into()));
        }
        (None, None) => None,
    })
}

/// Only a ChatGPT sign-in draws on the user's plan; an API key or another account bills
/// elsewhere, and no account cannot draw at all.
pub(super) fn check_account(read: &Value) -> Result<(), Stop> {
    match read
        .get("account")
        .and_then(|a| a.get("type"))
        .and_then(Value::as_str)
    {
        Some("chatgpt") => Ok(()),
        None => Err(Stop::Refused(
            "Codex is not signed in: run codex login and sign in with ChatGPT".into(),
        )),
        Some("apiKey") => Err(Stop::Refused(
            "Codex is signed in with an API key, which bills an API account: run codex login \
             and sign in with ChatGPT"
                .into(),
        )),
        Some(other) => Err(Stop::Refused(format!(
            "Codex is signed in with {}, not ChatGPT: run codex login and sign in with ChatGPT",
            token(other)
        ))),
    }
}

/// The text at `path` in `value`.
pub(super) fn text_at(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |at, key| at.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// An `imageGeneration` item's bytes, or why it has none.
pub(super) fn image_of(item: &Value) -> Result<Vec<u8>, String> {
    if let Some(failure) = item.get("failure").filter(|f| !f.is_null()) {
        if failure["type"] == "usageLimitExceeded" {
            return Err(match failure.get("resetsAt").and_then(Value::as_i64) {
                Some(at) => {
                    format!("the Codex usage limit is reached; it resets at unix time {at}")
                }
                None => "the Codex usage limit is reached".to_string(),
            });
        }
        return Err(format!(
            "Codex's image generation failed ({})",
            token_of(failure.get("type"))
        ));
    }
    let status = item.get("status").and_then(Value::as_str).unwrap_or("");
    if status != "completed" {
        return Err(format!("Codex's image generation ended {}", token(status)));
    }
    item.get("result")
        .and_then(Value::as_str)
        .and_then(wire::strict_base64)
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| "Codex's image is not valid base64".to_string())
}

/// Why a turn that completed without an image did: its error's kind (`codexErrorInfo`, a closed
/// vocabulary), else its status.
pub(super) fn turn_failure(turn: &Value) -> String {
    let info = &turn["error"]["codexErrorInfo"];
    let kind = match info {
        Value::String(kind) => Some(kind.as_str()),
        Value::Object(variant) => variant.keys().next().map(String::as_str),
        _ => None,
    };
    match (kind, turn.get("error").filter(|e| !e.is_null())) {
        (Some("usageLimitExceeded"), _) => "the Codex usage limit is reached".to_string(),
        (Some("unauthorized"), _) => {
            "Codex is not signed in: run codex login and sign in with ChatGPT".to_string()
        }
        (Some(kind), _) => format!("Codex's turn failed ({})", token(kind)),
        (None, Some(_)) => "Codex's turn failed".to_string(),
        (None, None) => match turn.get("status").and_then(Value::as_str) {
            Some("completed") | None => "Codex answered without an image".to_string(),
            Some(status) => format!("Codex's turn ended {} without an image", token(status)),
        },
    }
}

/// A word from one of Codex's closed vocabularies, safe to show: letters, digits and `_`, at most
/// 40 of them; anything else is `unknown`.
pub(super) fn token(text: &str) -> String {
    let safe = !text.is_empty()
        && text.len() <= 40
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if safe {
        text.to_string()
    } else {
        "unknown".to_string()
    }
}

fn token_of(value: Option<&Value>) -> String {
    token(value.and_then(Value::as_str).unwrap_or(""))
}

/// A name Codex gives (a model's), safe to keep: letters, digits, `.`, `-` and `_`, at most 60 of
/// them.
pub(super) fn name(text: &str) -> Option<String> {
    let safe = !text.is_empty()
        && text.len() <= 60
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    safe.then(|| text.to_string())
}

/// Codex's version from `initialize`'s `userAgent` (`<client>/<version> (…`), when it is one.
pub(super) fn version_of(initialized: &Value) -> Option<String> {
    let agent = initialized.get("userAgent").and_then(Value::as_str)?;
    let version = agent.split_whitespace().next()?.split_once('/')?.1;
    let safe = !version.is_empty()
        && version.len() <= 40
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
    safe.then(|| version.to_string())
}

/// A thread's total token usage from `thread/tokenUsage/updated`, onto `usage`.
pub(super) fn add_tokens(usage: &mut Usage, params: &Value) {
    let total = &params["tokenUsage"]["total"];
    let count = |name: &str| total.get(name).and_then(Value::as_u64);
    if let Some(input) = count("inputTokens") {
        usage.input_tokens = Some(input);
        usage.cached_input_tokens = count("cachedInputTokens");
        usage.output_tokens = count("outputTokens");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_chatgpt_account_is_admitted() {
        assert!(check_account(&json!({"account": {"type": "chatgpt", "planType": "pro"}})).is_ok());
        let refused = |read: Value| match check_account(&read) {
            Err(Stop::Refused(reason)) => reason,
            other => panic!("{:?}", other.err().map(|s| s.reason())),
        };
        assert!(
            refused(json!({"account": null, "requiresOpenaiAuth": true}))
                .starts_with("Codex is not signed in")
        );
        assert!(refused(json!({"account": {"type": "apiKey"}})).contains("bills an API account"));
        assert_eq!(
            refused(json!({"account": {"type": "amazonBedrock"}})),
            "Codex is signed in with amazonBedrock, not ChatGPT: run codex login and sign in with ChatGPT"
        );
        assert!(refused(json!({"account": {"type": "/Users/a/secret"}})).contains(" unknown, "));
    }

    #[test]
    fn failures_use_codex_s_closed_vocabularies_only() {
        let item = json!({"type": "imageGeneration", "status": "failed", "result": "",
            "failure": {"type": "usageLimitExceeded", "limitId": "codex", "resetsAt": 1792311096}});
        assert_eq!(
            image_of(&item),
            Err("the Codex usage limit is reached; it resets at unix time 1792311096".into())
        );
        let odd = json!({"type": "imageGeneration", "status": "failed", "result": "",
            "failure": {"type": "see /Users/a/.codex/config.toml"}});
        assert_eq!(
            image_of(&odd),
            Err("Codex's image generation failed (unknown)".into())
        );
        let unfinished = json!({"type": "imageGeneration", "status": "inProgress", "result": ""});
        assert_eq!(
            image_of(&unfinished),
            Err("Codex's image generation ended inProgress".into())
        );
        let failed = |error: Value| turn_failure(&json!({"status": "failed", "error": error}));
        assert_eq!(
            failed(json!({"message": "at /Users/a/x", "codexErrorInfo": "serverOverloaded"})),
            "Codex's turn failed (serverOverloaded)"
        );
        assert_eq!(
            failed(
                json!({"message": "m", "codexErrorInfo": {"httpConnectionFailed": {"httpStatusCode": 502}}})
            ),
            "Codex's turn failed (httpConnectionFailed)"
        );
        assert!(
            failed(json!({"message": "m", "codexErrorInfo": "unauthorized"}))
                .starts_with("Codex is not signed in")
        );
        assert_eq!(
            failed(json!({"message": "/Users/a/x"})),
            "Codex's turn failed"
        );
        assert_eq!(
            turn_failure(&json!({"status": "completed", "error": null})),
            "Codex answered without an image"
        );
        assert_eq!(
            turn_failure(&json!({"status": "interrupted", "error": null})),
            "Codex's turn ended interrupted without an image"
        );
    }

    #[test]
    fn the_version_and_tokens_are_read_when_codex_gives_them() {
        let agent = |text: &str| version_of(&json!({"userAgent": text}));
        assert_eq!(
            agent("grida-fx/0.160.1 (Mac OS 26.6.0; arm64) unknown (grida-fx; 0.2.1)"),
            Some("0.160.1".into())
        );
        assert_eq!(agent("grida-fx/../../etc (x)"), None);
        assert_eq!(version_of(&json!({})), None);
        let mut usage = Usage::default();
        add_tokens(
            &mut usage,
            &json!({"tokenUsage": {"total": {"inputTokens": 17393,
            "cachedInputTokens": 0, "outputTokens": 72}}}),
        );
        assert_eq!(
            (
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.output_tokens
            ),
            (Some(17393), Some(0), Some(72))
        );
        add_tokens(&mut usage, &json!({"tokenUsage": {}}));
        assert_eq!(
            usage.input_tokens,
            Some(17393),
            "a report without counts changes nothing"
        );
    }

    #[test]
    fn messages_are_read_by_their_shape() {
        let read = |value: Value| message_of(value).map_err(|stop| stop.reason());
        assert!(matches!(
            read(json!({"id": 3, "result": {}})),
            Ok(Some(Message::Response {
                id: 3,
                result: Ok(_)
            }))
        ));
        assert!(matches!(
            read(json!({"id": 3, "error": {"code": -32600, "message": "/Users/a"}})),
            Ok(Some(Message::Response {
                id: 3,
                result: Err(-32600)
            }))
        ));
        assert!(matches!(
            read(json!({"id": "ask-1", "method": "x"})),
            Ok(Some(Message::Request { .. }))
        ));
        assert!(matches!(
            read(json!({"method": "turn/started", "params": {}})),
            Ok(Some(Message::Notification { .. }))
        ));
        assert_eq!(
            read(json!({"id": null, "error": {"code": -32700}})).err(),
            Some("Codex could not read a request".to_string())
        );
        assert!(matches!(read(json!({"jsonrpc": "2.0"})), Ok(None)));
    }
}
