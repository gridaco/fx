//! Codex: `image.generate` through the user's own signed-in Codex (spec/providers.md §9.6).
//!
//! No key and no HTTP: the adapter starts `codex app-server` and speaks its JSON-RPC, one JSON
//! message per line on stdin and stdout. Codex owns sign-in, the model and the usage; FX never
//! reads its credential files. One send, in order:
//! 1. the route: capability `image.generate` on provider `codex`, and a contract whose `adapter`
//!    (when present) is [`ADAPTER`] and whose `adapter_behavior` (when present) is
//!    [`ADAPTER_BEHAVIOR`];
//! 2. [`crate::capabilities::check_request`];
//! 3. values: a blank prompt, a `size` other than `auto` (Codex picks its own sizes: asked for
//!    1024x1024 it drew 1254x1254), an unknown `background`, references;
//! 4. the program the setup found ([`find_program`], which only [`crate::live::live_setup`]
//!    calls): `GRIDA_FX_TOOL_CODEX`, else `codex` on `PATH`, else the copy inside the ChatGPT app
//!    on macOS; none is `Sent::Refused` with [`NOT_INSTALLED`] (or [`NETWORK_OFF`]);
//! 5. one exchange ([`exchange`]): `initialize`, `thread/start` (an empty scratch folder, a
//!    read-only sandbox, no approvals, not persisted), `turn/start` with [`instructions`], then
//!    notifications until `turn/completed`. The answer is the turn's `imageGeneration` item.
//!
//! Outcomes: anything before Codex accepts the turn is `NotReceived`, so the engine may start
//! again; once the turn is running a generation may already have been made, so every failure is
//! `Failed { retryable: false }`. The route is free to FX (Codex counts it against the user's own
//! plan), so every cost is $0. Codex's stderr is dropped and its environment loses the allowlisted
//! provider keys and `CODEX_API_KEY`, so it never falls back to API-key billing.

use crate::BoxFuture;
use crate::adapter::{Adapter, Answer, CallRequest, RequestAdapter, RouteRef, Sent};
use crate::capabilities;
use crate::keys::KeyName;
use crate::registry::Adapters;
use crate::setup::Setup;
use crate::wire;
use grida_fx_core::money::Usd;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

/// The label of every reason.
pub const LABEL: &str = "Codex image generation";

/// The route contract's `adapter` this adapter serves.
pub const ADAPTER: &str = "fx-codex-image-v1";

/// The route contract's `adapter_behavior` this adapter serves. It names [`instructions`]: a
/// change to them is a new behaviour.
pub const ADAPTER_BEHAVIOR: &str = "1";

/// The whole-exchange deadline of one send.
pub const DEADLINE: Duration = Duration::from_secs(600);

/// The variable that names the program, as for any tool (spec/protocol.md §5.6).
pub const PROGRAM_VARIABLE: &str = "GRIDA_FX_TOOL_CODEX";

/// Where the ChatGPT app for macOS keeps its own copy of the command.
pub const MACOS_APP_PROGRAM: &str =
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex";

/// The longest line Codex may send (an image arrives base64 on one line).
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// The refusal when no program was found.
pub const NOT_INSTALLED: &str = "Codex is not installed: install it with npm install -g \
                                 @openai/codex and sign in with codex login, or set \
                                 GRIDA_FX_TOOL_CODEX";

/// The refusal of an exact size.
pub const NO_EXACT_SIZE: &str = "Codex image generation takes no exact size: use size auto, and \
                                 fx/image.resize@1 after it";

/// The refusal when `GRIDA_FX_NETWORK` is off: Codex generates over the network.
pub const NETWORK_OFF: &str = "Codex was not started: GRIDA_FX_NETWORK is off";

/// The variables Codex never inherits: with one of these it could bill an API account instead of
/// the user's plan.
const WITHHELD: [&str; 2] = ["CODEX_API_KEY", "OPENAI_API_KEY"];

/// Settings for every Codex this adapter starts: nothing of the user's that runs on a turn's end
/// or remembers it.
const OVERRIDES: [&str; 3] = [
    "notify=[]",
    "features.memories=false",
    "features.hooks=false",
];

/// The adapter (module doc).
#[derive(Debug, Clone)]
pub struct CodexImages {
    /// The program, or the refusal of every call.
    program: Result<PathBuf, String>,
    deadline: Duration,
}

impl CodexImages {
    /// The adapter over the setup's program.
    pub fn new(setup: &Setup) -> CodexImages {
        CodexImages::with_program(setup.codex.clone())
    }

    /// The adapter over `program` (tests: a fake Codex).
    pub fn with_program(program: Result<PathBuf, String>) -> CodexImages {
        CodexImages {
            program,
            deadline: DEADLINE,
        }
    }

    /// The same adapter with another deadline (tests).
    pub fn with_deadline(mut self, deadline: Duration) -> CodexImages {
        self.deadline = deadline;
        self
    }

    /// Steps 1 to 4 of the module doc: the program and the turn's text, or the refusal.
    fn prepare(&self, call: &CallRequest) -> Result<(PathBuf, String), String> {
        served(&call.route)?;
        capabilities::check_request(&call.route.capability, &call.request)?;
        let values = Values::read(&call.request)?;
        let program = self.program.clone()?;
        Ok((program, instructions(&values)))
    }
}

impl RequestAdapter for CodexImages {
    fn send<'a>(&'a self, call: &'a CallRequest) -> BoxFuture<'a, Sent> {
        Box::pin(async move {
            let (program, text) = match self.prepare(call) {
                Ok(prepared) => prepared,
                Err(reason) => return Sent::Refused { reason },
            };
            exchange(&program, &text, self.deadline).await
        })
    }

    fn check(&self, call: &CallRequest, answer: &Answer) -> Result<(), String> {
        let Some(image) = answer.files.get("image") else {
            return Err("the answer has no image".into());
        };
        crate::checks::image(call, image, None)
    }
}

/// Registers the adapter for `image.generate` on `codex`, over the setup's program. Codex brings
/// its own sign-in and network: no key, no transport.
pub fn register(adapters: &mut Adapters, setup: &Setup) {
    adapters.register(
        "image.generate",
        "codex",
        Adapter::Request(Arc::new(CodexImages::new(setup))),
    );
}

/// The program (step 4): `GRIDA_FX_TOOL_CODEX` when set and not blank (and then only when it
/// names a file), else the first `codex` on `PATH`, else `app` when it exists.
pub fn find_program(env: &dyn Fn(&str) -> Option<String>, app: &Path) -> Option<PathBuf> {
    if let Some(named) = env(PROGRAM_VARIABLE).filter(|v| !v.trim().is_empty()) {
        let named = PathBuf::from(named);
        return named.is_file().then_some(named);
    }
    let on_path = env("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("codex"))
            .find(|candidate| candidate.is_file())
    });
    on_path.or_else(|| app.is_file().then(|| app.to_path_buf()))
}

/// The turn's text: fixed instructions, a sentence for a transparent or opaque background, then
/// the authored prompt verbatim (behaviour [`ADAPTER_BEHAVIOR`]).
pub fn instructions(values: &Values<'_>) -> String {
    let mut text = String::from(
        "Use $imagegen to generate exactly one image from the prompt between the <prompt> tags. \
         Give it the prompt as written.",
    );
    match values.background {
        "transparent" => text.push_str(" Give it a transparent background (an alpha channel)."),
        "opaque" => text.push_str(" Give it an opaque background, with no transparent pixels."),
        _ => {}
    }
    text.push_str(
        " Do not run commands, read or edit files, use any other tool or ask questions. When the \
         image is made, reply with one short sentence.",
    );
    format!("{text}\n\n<prompt>\n{}\n</prompt>", values.prompt)
}

/// Step 1: whether the route is one this adapter serves.
fn served(route: &RouteRef) -> Result<(), String> {
    let refused = || {
        format!(
            "{} is not a {} route this adapter serves",
            route.id(),
            route.capability
        )
    };
    if route.capability != "image.generate" || route.provider != "codex" {
        return Err(refused());
    }
    let contract = match &route.contract {
        Value::Null => return Ok(()),
        Value::Object(contract) => contract,
        _ => return Err(refused()),
    };
    let text_or_absent = |key: &str, want: &str| match contract.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::String(have)) => have == want,
        Some(_) => false,
    };
    if text_or_absent("adapter", ADAPTER) && text_or_absent("adapter_behavior", ADAPTER_BEHAVIOR) {
        Ok(())
    } else {
        Err(refused())
    }
}

/// A request's values after step 3.
#[derive(Debug, PartialEq)]
pub struct Values<'a> {
    /// Verbatim: the prompt is never trimmed or rewritten.
    pub prompt: &'a str,
    /// `auto`, `opaque` or `transparent`; `auto` when absent.
    pub background: &'a str,
}

impl<'a> Values<'a> {
    /// Reads a request that passed its capability's check (step 3).
    pub fn read(request: &'a Value) -> Result<Values<'a>, String> {
        let prompt = request.get("prompt").and_then(Value::as_str).unwrap_or("");
        if prompt.trim().is_empty() {
            return Err("an image call needs its prompt as text".into());
        }
        match request.get("size") {
            None | Some(Value::Null) => {}
            Some(Value::String(size)) if size == "auto" => {}
            Some(_) => return Err(NO_EXACT_SIZE.into()),
        }
        let background = match request.get("background") {
            None | Some(Value::Null) => "auto",
            Some(Value::String(b)) if matches!(b.as_str(), "auto" | "opaque" | "transparent") => {
                b.as_str()
            }
            Some(_) => return Err("background must be auto, opaque or transparent".into()),
        };
        let references = request.get("references").and_then(Value::as_array);
        if references.is_some_and(|list| !list.is_empty()) {
            return Err("Codex image generation takes no references".into());
        }
        Ok(Values { prompt, background })
    }
}

/// Step 5: one Codex, one thread, one turn (module doc).
async fn exchange(program: &Path, text: &str, deadline: Duration) -> Sent {
    let folder = match tempfile::Builder::new().prefix("grida-fx-codex-").tempdir() {
        Ok(folder) => folder,
        Err(error) => {
            let reason = grida_fx_core::error::io_reason(&error);
            return Sent::not_received(format!("{LABEL} was not started: {reason}"));
        }
    };
    let mut command = Command::new(program);
    command.arg("app-server");
    for setting in OVERRIDES {
        command.args(["-c", setting]);
    }
    for variable in WITHHELD
        .iter()
        .copied()
        .chain(KeyName::ALL.map(KeyName::variable))
    {
        command.env_remove(variable);
    }
    command
        .current_dir(folder.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Sent::Refused {
                reason: NOT_INSTALLED.into(),
            };
        }
        Err(error) => {
            return Sent::Refused {
                reason: format!(
                    "{LABEL} cannot start codex: {}",
                    grida_fx_core::error::io_reason(&error)
                ),
            };
        }
    };
    let mut session = match Session::new(&mut child) {
        Some(session) => session,
        None => return Sent::not_received(format!("{LABEL} was not started: no pipes")),
    };
    let outcome = tokio::time::timeout(deadline, session.run(folder.path(), text)).await;
    let sent = match outcome {
        Ok(sent) => sent,
        Err(_) => {
            session.interrupt().await;
            if session.turn.is_some() {
                Sent::Failed {
                    reason: format!("{LABEL} did not finish within {} s", deadline.as_secs()),
                    cost: Some(Usd::ZERO),
                    retryable: false,
                }
            } else {
                Sent::not_received(format!(
                    "{LABEL} did not start a turn within {} s",
                    deadline.as_secs()
                ))
            }
        }
    };
    // Closing stdin lets Codex end its own children; a Codex that lingers is killed.
    drop(session);
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    sent
}

/// One app-server conversation.
struct Session {
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    next_id: u64,
    thread: Option<String>,
    /// Set once Codex accepts the turn: from then on a failure may have spent a generation.
    turn: Option<String>,
}

/// What one message from Codex is.
enum Message {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
    },
}

impl Session {
    fn new(child: &mut Child) -> Option<Session> {
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        Some(Session {
            stdin,
            lines: BufReader::new(stdout).lines(),
            next_id: 0,
            thread: None,
            turn: None,
        })
    }

    async fn run(&mut self, folder: &Path, text: &str) -> Sent {
        let started = async {
            let info =
                json!({"clientInfo": {"name": "grida-fx", "version": env!("CARGO_PKG_VERSION")}});
            self.call("initialize", info).await?;
            self.write(&json!({"jsonrpc": "2.0", "method": "initialized"}))
                .await?;
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
            let thread = text_at(&thread, &["thread", "id"])
                .ok_or_else(|| Stop::NotReceived("Codex started no thread".into()))?;
            self.thread = Some(thread.clone());
            let input = json!([{"type": "text", "text": text}]);
            let turn = self
                .call("turn/start", json!({"threadId": thread, "input": input}))
                .await?;
            self.turn = Some(text_at(&turn, &["turn", "id"]).unwrap_or_default());
            Ok::<(), Stop>(())
        };
        if let Err(stop) = started.await {
            return stop.sent();
        }
        match self.until_completed().await {
            Ok(sent) => sent,
            Err(stop) => stop.sent(),
        }
    }

    /// Reads notifications until the turn completes; the answer is its first finished image.
    async fn until_completed(&mut self) -> Result<Sent, Stop> {
        let mut image: Option<Result<Vec<u8>, String>> = None;
        loop {
            match self.read().await? {
                Message::Notification { method, params } if method == "item/completed" => {
                    let item = &params["item"];
                    if item["type"] == "imageGeneration" && image.is_none() {
                        image = Some(image_of(item));
                    }
                }
                Message::Notification { method, params } if method == "turn/completed" => {
                    return Ok(finished(&params["turn"], image));
                }
                Message::Notification { .. } | Message::Response { .. } => {}
                Message::Request { id } => self.decline(id).await?,
            }
        }
    }

    /// Sends a request and waits for its response, answering what Codex asks meanwhile.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, Stop> {
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
                    return result.map_err(|message| {
                        Stop::NotReceived(format!("Codex refused {method}: {message}"))
                    });
                }
                Message::Request { id } => self.decline(id).await?,
                _ => {}
            }
        }
    }

    /// Asks Codex to stop the running turn; best effort.
    async fn interrupt(&mut self) {
        if let (Some(thread), Some(turn)) = (self.thread.clone(), self.turn.clone()) {
            self.next_id += 1;
            let message = json!({"jsonrpc": "2.0", "id": self.next_id, "method": "turn/interrupt",
                "params": {"threadId": thread, "turnId": turn}});
            let _ = tokio::time::timeout(Duration::from_secs(2), self.write(&message)).await;
        }
    }

    /// Refuses a request from Codex (an approval, a question): this adapter grants nothing.
    async fn decline(&mut self, id: Value) -> Result<(), Stop> {
        let error = json!({"code": -32601, "message": "grida-fx answers no requests"});
        self.write(&json!({"jsonrpc": "2.0", "id": id, "error": error}))
            .await
    }

    async fn write(&mut self, message: &Value) -> Result<(), Stop> {
        let mut line = message.to_string();
        line.push('\n');
        let written = async {
            self.stdin.write_all(line.as_bytes()).await?;
            self.stdin.flush().await
        };
        written.await.map_err(|error| {
            self.stop(format!(
                "Codex stopped reading: {}",
                grida_fx_core::error::io_reason(&error)
            ))
        })
    }

    async fn read(&mut self) -> Result<Message, Stop> {
        loop {
            let line = match self.lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => return Err(self.stop("Codex exited".into())),
                Err(error) => {
                    return Err(self.stop(format!(
                        "Codex's output is unreadable: {}",
                        grida_fx_core::error::io_reason(&error)
                    )));
                }
            };
            if line.len() > MAX_LINE_BYTES {
                return Err(self.stop("Codex sent a message over 64 MiB".into()));
            }
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                return Err(self.stop("Codex sent a line that is not JSON".into()));
            };
            if let Some(message) = message_of(value) {
                return Ok(message);
            }
        }
    }

    /// A stop: not received until the turn is running, then failed.
    fn stop(&self, reason: String) -> Stop {
        if self.turn.is_some() {
            Stop::Failed(reason)
        } else {
            Stop::NotReceived(reason)
        }
    }
}

/// Why a conversation ended early.
enum Stop {
    NotReceived(String),
    Failed(String),
}

impl Stop {
    fn sent(self) -> Sent {
        match self {
            Stop::NotReceived(reason) => {
                Sent::not_received(format!("{LABEL} was not sent: {reason}"))
            }
            Stop::Failed(reason) => failed(format!("{LABEL} failed: {reason}")),
        }
    }
}

/// A failure after the turn started: never retried, and free.
fn failed(reason: String) -> Sent {
    Sent::Failed {
        reason,
        cost: Some(Usd::ZERO),
        retryable: false,
    }
}

/// One line's message, or `None` for anything else JSON-RPC allows.
fn message_of(value: Value) -> Option<Message> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = value.get("id").cloned();
    match (method, id) {
        (Some(_), Some(id)) => Some(Message::Request { id }),
        (Some(method), None) => Some(Message::Notification {
            method,
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let result = match value.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("an error")
                    .to_string()),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(Message::Response { id, result })
        }
        (None, None) => None,
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
fn image_of(item: &Value) -> Result<Vec<u8>, String> {
    if let Some(failure) = item.get("failure").filter(|f| !f.is_null()) {
        return Err(failure_reason(failure));
    }
    let status = item.get("status").and_then(Value::as_str).unwrap_or("");
    if status != "completed" {
        return Err(format!("Codex's image generation ended {status}"));
    }
    item.get("result")
        .and_then(Value::as_str)
        .and_then(wire::strict_base64)
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| "Codex's image is not valid base64".to_string())
}

/// An image failure as a sentence.
fn failure_reason(failure: &Value) -> String {
    if failure["type"] == "usageLimitExceeded" {
        return match failure.get("resetsAt").and_then(Value::as_i64) {
            Some(at) => format!("the Codex usage limit is reached; it resets at unix time {at}"),
            None => "the Codex usage limit is reached".to_string(),
        };
    }
    let kind = failure
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    format!("Codex's image generation failed ({kind})")
}

/// The outcome of a completed turn.
fn finished(turn: &Value, image: Option<Result<Vec<u8>, String>>) -> Sent {
    match image {
        Some(Ok(bytes)) => {
            let kind = wire::sniff_image(&bytes).unwrap_or("application/octet-stream");
            Sent::Answered(
                Answer::new(Value::Null, Some(Usd::ZERO)).with_file("image", kind, bytes),
            )
        }
        Some(Err(reason)) => failed(format!("{LABEL} failed: {reason}")),
        None => {
            let error = turn
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str);
            match error {
                Some(message) => failed(format!("{LABEL} failed: {message}")),
                None => failed(format!("{LABEL} failed: Codex answered without an image")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(provider: &str, contract: Value) -> RouteRef {
        RouteRef {
            capability: "image.generate".into(),
            model: "imagegen".into(),
            provider: provider.into(),
            contract,
        }
    }

    #[test]
    fn routes_are_served_by_provider_and_contract() {
        assert_eq!(
            served(&route(
                "codex",
                json!({"adapter": ADAPTER, "adapter_behavior": "1"})
            )),
            Ok(())
        );
        assert_eq!(served(&route("codex", Value::Null)), Ok(()));
        for contract in [
            json!({"adapter": "fx-openai-image-v1"}),
            json!({"adapter_behavior": 1}),
            json!([]),
        ] {
            assert!(served(&route("codex", contract)).is_err());
        }
        assert_eq!(
            served(&route("openai", json!({}))),
            Err("imagegen@openai is not a image.generate route this adapter serves".into())
        );
    }

    #[test]
    fn values_refuse_what_codex_does_not_honour() {
        let read = |request: Value| {
            Values::read(&request).map(|v| (v.prompt.to_string(), v.background.to_string()))
        };
        assert_eq!(
            read(json!({"prompt": " a kite "})),
            Ok((" a kite ".into(), "auto".into()))
        );
        assert_eq!(
            read(json!({"prompt": "p", "size": "auto", "background": "transparent"})),
            Ok(("p".into(), "transparent".into()))
        );
        assert_eq!(
            read(json!({"prompt": "p", "size": null, "references": []})),
            Ok(("p".into(), "auto".into()))
        );
        for (request, reason) in [
            (
                json!({"prompt": " "}),
                "an image call needs its prompt as text",
            ),
            (json!({"prompt": "p", "size": "1024x1024"}), NO_EXACT_SIZE),
            (
                json!({"prompt": "p", "background": "clear"}),
                "background must be auto, opaque or transparent",
            ),
            (
                json!({"prompt": "p", "references": [{"file": "a"}]}),
                "Codex image generation takes no references",
            ),
        ] {
            assert_eq!(read(request.clone()), Err(reason.to_string()), "{request}");
        }
    }

    #[test]
    fn the_instructions_name_the_background_then_the_prompt_verbatim() {
        let plain = Values {
            prompt: "a red\n kite ",
            background: "auto",
        };
        assert_eq!(
            instructions(&plain),
            "Use $imagegen to generate exactly one image from the prompt between the <prompt> \
             tags. Give it the prompt as written. Do not run commands, read or edit files, use \
             any other tool or ask questions. When the image is made, reply with one short \
             sentence.\n\n<prompt>\na red\n kite \n</prompt>"
        );
        let icon = Values {
            prompt: "p",
            background: "transparent",
        };
        assert!(instructions(&icon).contains(
            "as written. Give it a transparent background (an alpha channel). Do not run"
        ));
        let opaque = Values {
            prompt: "p",
            background: "opaque",
        };
        assert!(
            instructions(&opaque)
                .contains(" Give it an opaque background, with no transparent pixels. ")
        );
    }

    #[test]
    fn the_program_is_the_variable_then_path_then_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let on_path = dir.path().join("codex");
        std::fs::write(&on_path, "").unwrap();
        let app = dir.path().join("app-codex");
        std::fs::write(&app, "").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let env = |pairs: &'static [(&'static str, &'static str)], path: Option<String>| {
            move |name: &str| {
                if name == "PATH" {
                    return path.clone();
                }
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        let named = env(
            &[(PROGRAM_VARIABLE, "/nonexistent/codex")],
            Some(path.clone()),
        );
        assert_eq!(
            find_program(&named, &app),
            None,
            "a named program is the only one"
        );
        let app_text: &'static str = Box::leak(app.to_string_lossy().into_owned().into_boxed_str());
        let pairs: &'static [(&'static str, &'static str)] =
            Box::leak(vec![(PROGRAM_VARIABLE, app_text)].into_boxed_slice());
        let existing = env(pairs, Some(path.clone()));
        assert_eq!(
            find_program(&existing, &dir.path().join("missing")),
            Some(app.clone())
        );
        let blank = env(&[(PROGRAM_VARIABLE, " ")], Some(path.clone()));
        assert_eq!(find_program(&blank, &app), Some(on_path));
        let none = env(&[], None);
        assert_eq!(find_program(&none, &app), Some(app.clone()));
        assert_eq!(find_program(&none, &dir.path().join("missing")), None);
    }

    #[test]
    fn failures_name_the_usage_limit() {
        let item = json!({"type": "imageGeneration", "status": "failed", "result": "",
            "failure": {"type": "usageLimitExceeded", "limitId": "codex", "resetsAt": 1792311096}});
        assert_eq!(
            image_of(&item),
            Err("the Codex usage limit is reached; it resets at unix time 1792311096".into())
        );
        let unfinished = json!({"type": "imageGeneration", "status": "in_progress", "result": ""});
        assert_eq!(
            image_of(&unfinished),
            Err("Codex's image generation ended in_progress".into())
        );
        assert_eq!(
            finished(&json!({"status": "completed", "error": null}), None),
            failed("Codex image generation failed: Codex answered without an image".into())
        );
    }
}
