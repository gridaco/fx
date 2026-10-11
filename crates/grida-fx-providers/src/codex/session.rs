//! One conversation with `codex app-server` (spec/providers.md §9.6): JSON-RPC 2.0, one JSON
//! message per line on Codex's stdin and stdout.
//!
//! The conversation is `initialize`, `account/read`, `thread/start`, `turn/start`, then
//! notifications of this thread and turn until the first good `imageGeneration` item (the answer)
//! or `turn/completed` (no image). Notifications that arrive while a request waits for its
//! response are kept for later; a request Codex sends (an approval, a question) is declined.
//! Reasons are fixed sentences with bounded tokens from Codex's closed vocabularies, never
//! Codex's own text, which can name private paths (spec/providers.md §8).

use super::Stop;
use crate::wire;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// The longest line Codex may send (an image arrives base64 on one line).
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// The reasoning effort and service tier of every turn: the user's own settings (a high effort,
/// a priority tier) would spend their plan faster for nothing an image needs.
const EFFORT: &str = "low";
const SERVICE_TIER: &str = "default";

/// What one line from Codex is.
enum Message {
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

pub(super) struct Session {
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// Notifications read while a request waited for its response.
    kept: VecDeque<Message>,
    thread: Option<String>,
    turn: Option<String>,
    /// Set just before `turn/start` is written: from then on a generation may have started.
    pub(super) turn_requested: bool,
}

impl Session {
    pub(super) fn new(child: &mut Child) -> Option<Session> {
        Some(Session {
            stdin: Some(child.stdin.take()?),
            stdout: BufReader::new(child.stdout.take()?),
            next_id: 0,
            kept: VecDeque::new(),
            thread: None,
            turn: None,
            turn_requested: false,
        })
    }

    /// Everything up to Codex accepting the turn: `initialize`, the account check, the thread and
    /// the turn.
    pub(super) async fn start(&mut self, folder: &Path, text: &str) -> Result<(), Stop> {
        let info =
            json!({"clientInfo": {"name": "grida-fx", "version": env!("CARGO_PKG_VERSION")}});
        self.call("initialize", info).await?;
        self.write(&json!({"jsonrpc": "2.0", "method": "initialized"}))
            .await?;
        let account = self.call("account/read", json!({})).await?;
        check_account(&account)?;
        let thread = self
            .call(
                "thread/start",
                json!({
                    "cwd": folder.to_string_lossy(),
                    "sandbox": "read-only",
                    "approvalPolicy": "never",
                    "ephemeral": true,
                }),
            )
            .await?;
        match thread.get("modelProvider").and_then(Value::as_str) {
            None | Some("openai") => {}
            Some(other) => {
                return Err(Stop::Refused(format!(
                    "Codex uses the model provider {}, not OpenAI: image generation needs \
                     Codex signed in with ChatGPT",
                    token(other)
                )));
            }
        }
        let thread = text_at(&thread, &["thread", "id"])
            .ok_or_else(|| Stop::Failed("Codex started no thread".into()))?;
        self.thread = Some(thread.clone());
        self.turn_requested = true;
        let turn = self
            .call(
                "turn/start",
                json!({
                    "threadId": thread,
                    "input": [{"type": "text", "text": text}],
                    "effort": EFFORT,
                    "serviceTierForTurn": SERVICE_TIER,
                }),
            )
            .await?;
        self.turn = text_at(&turn, &["turn", "id"]);
        Ok(())
    }

    /// Reads this turn's notifications: the first good image is the answer (the rest of the turn
    /// is interrupted); a turn that completes without one is a failure.
    pub(super) async fn image(&mut self) -> Result<Vec<u8>, Stop> {
        let mut failure: Option<String> = None;
        loop {
            match self.next().await? {
                Message::Notification { method, params } if method == "item/completed" => {
                    let item = &params["item"];
                    if item["type"] != "imageGeneration" || !self.ours(&params) {
                        continue;
                    }
                    match image_of(item) {
                        Ok(bytes) => {
                            self.interrupt().await;
                            return Ok(bytes);
                        }
                        Err(reason) => {
                            failure.get_or_insert(reason);
                        }
                    }
                }
                Message::Notification { method, params } if method == "turn/completed" => {
                    let turn = &params["turn"];
                    if self.ours(&params) && self.our_turn(turn) {
                        return Err(Stop::Failed(failure.unwrap_or_else(|| turn_failure(turn))));
                    }
                }
                Message::Request { id } => self.decline(id).await?,
                Message::Notification { .. } | Message::Response { .. } => {}
            }
        }
    }

    /// Asks Codex to stop the running turn; best effort, without waiting for an answer.
    pub(super) async fn interrupt(&mut self) {
        if let (Some(thread), Some(turn)) = (self.thread.clone(), self.turn.clone()) {
            self.next_id += 1;
            let message = json!({"jsonrpc": "2.0", "id": self.next_id, "method": "turn/interrupt",
                "params": {"threadId": thread, "turnId": turn}});
            let _ = tokio::time::timeout(Duration::from_secs(2), self.write(&message)).await;
        }
    }

    /// Closes Codex's stdin: Codex ends when it reads the end.
    pub(super) fn close(&mut self) {
        self.stdin = None;
    }

    /// A notification of this thread (and of this turn, once it is known).
    fn ours(&self, params: &Value) -> bool {
        let thread = params.get("threadId").and_then(Value::as_str);
        let turn = params.get("turnId").and_then(Value::as_str);
        thread.is_some_and(|t| Some(t) == self.thread.as_deref())
            && turn.is_none_or(|t| self.turn.as_deref().is_none_or(|ours| t == ours))
    }

    fn our_turn(&self, turn: &Value) -> bool {
        let id = turn.get("id").and_then(Value::as_str);
        match (&self.turn, id) {
            (Some(ours), Some(id)) => id == ours,
            _ => true,
        }
    }

    /// Sends a request and waits for its response, keeping the notifications that come first and
    /// declining Codex's requests. An error response is a refusal of the request.
    async fn call(&mut self, method: &'static str, params: Value) -> Result<Value, Stop> {
        self.next_id += 1;
        let id = self.next_id;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            match self.read().await? {
                Message::Response {
                    id: answered,
                    result,
                } if answered == id => {
                    return result.map_err(|code| {
                        Stop::Failed(format!("Codex refused {method} (error {code})"))
                    });
                }
                Message::Response { .. } => {}
                Message::Request { id } => self.decline(id).await?,
                notification => self.kept.push_back(notification),
            }
        }
    }

    /// Declines a request from Codex: this client grants nothing.
    async fn decline(&mut self, id: Value) -> Result<(), Stop> {
        let error = json!({"code": -32601, "message": "grida-fx answers no requests"});
        self.write(&json!({"jsonrpc": "2.0", "id": id, "error": error}))
            .await
    }

    async fn write(&mut self, message: &Value) -> Result<(), Stop> {
        let mut line = message.to_string();
        line.push('\n');
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Stop::Failed("Codex's input is closed".into()));
        };
        let written = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        };
        written
            .await
            .map_err(|_| Stop::Failed("Codex stopped reading".into()))
    }

    /// The next message: a kept one, else one read.
    async fn next(&mut self) -> Result<Message, Stop> {
        match self.kept.pop_front() {
            Some(message) => Ok(message),
            None => self.read().await,
        }
    }

    async fn read(&mut self) -> Result<Message, Stop> {
        loop {
            let Some(line) = self.read_line().await? else {
                return Err(Stop::Failed("Codex exited".into()));
            };
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                return Err(Stop::Failed("Codex sent a line that is not JSON".into()));
            };
            if let Some(message) = message_of(value)? {
                return Ok(message);
            }
        }
    }

    /// One line, without its newline, refused as soon as it passes [`MAX_LINE_BYTES`]; `None` at
    /// the end of the output.
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, Stop> {
        let mut line = Vec::new();
        loop {
            let available = self
                .stdout
                .fill_buf()
                .await
                .map_err(|_| Stop::Failed("Codex's output is unreadable".into()))?;
            if available.is_empty() {
                return Ok((!line.is_empty()).then_some(line));
            }
            let (taken, ended) = match available.iter().position(|&byte| byte == b'\n') {
                Some(at) => {
                    line.extend_from_slice(&available[..at]);
                    (at + 1, true)
                }
                None => {
                    line.extend_from_slice(available);
                    (available.len(), false)
                }
            };
            self.stdout.consume(taken);
            if line.len() > MAX_LINE_BYTES {
                return Err(Stop::Failed("Codex sent a message over 64 MiB".into()));
            }
            if ended {
                return Ok(Some(line));
            }
        }
    }
}

/// One line's message; `None` for anything else JSON-RPC allows. An error response that names
/// no request means Codex could not read one of ours.
fn message_of(value: Value) -> Result<Option<Message>, Stop> {
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
fn check_account(read: &Value) -> Result<(), Stop> {
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
fn text_at(value: &Value, path: &[&str]) -> Option<String> {
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
fn token(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
