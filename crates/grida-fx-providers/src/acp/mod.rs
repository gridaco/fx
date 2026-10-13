//! A client of the Agent Client Protocol (ACP v1) over a program's stdin and stdout
//! (src/grok/README.md): JSON-RPC 2.0, one message per line. It keeps ACP's own vocabulary at
//! this boundary; what a session means is the driver's (Grok Build's, [`crate::grok`]).
//!
//! Two tasks serve a connection: a writer, which owns the program's stdin and writes the lines it
//! is sent, so even a dropped call can cancel its session without waiting; and a reader, which
//! hands each response to the request waiting for it, each notification that names a session
//! (`params.sessionId`) to that session, and answers each request the agent sends:
//! - `session/request_permission`: `allow_once` for a tool the connection allows, by the name the
//!   driver reads from the tool call (from the request, or from the session's earlier
//!   `tool_call` update of the same `toolCallId`), else `reject_once`; never `allow_always`;
//! - anything else (`fs/*`, `terminal/*`, an extension): error `-32601`. The client declares no
//!   file system and no terminal, and grants nothing.
//!
//! The client speaks ACP's own methods ([`Connection::initialize`], [`Connection::new_session`],
//! [`Connection::set_config_option`], [`Connection::prompt`], [`Connection::cancel`]); a driver's
//! extensions go through [`Connection::request`].
//!
//! A `session/new` response opens its session's notifications before the reader reads on, so
//! none of a prompt's is missed. Reasons are fixed sentences: an agent's own text can name private
//! paths (spec/adapters.md §7).

use crate::installed::{locked, read_line};
use crate::process_group::Group;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// The error code a client answers a request it does not serve with.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// Why a request got no result.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The agent answered with an error: its code.
    Refused(i64),
    /// The connection ended, and why.
    Ended(String),
}

/// What the reader needs from the driver: the agent's name for reasons, the tools a permission
/// request may be granted for, and how to read a tool's name from a tool call.
#[derive(Clone)]
pub struct Policy {
    /// Such as `Grok Build`.
    pub label: &'static str,
    pub allowed_tools: &'static [&'static str],
    pub tool_name: fn(&Value) -> Option<String>,
}

/// The `params` of a session's notifications.
pub type Notes = mpsc::UnboundedReceiver<Value>;

/// A `session/new` response: the session it started and its notifications, if any, or the error
/// code.
type SessionOpened = Result<Option<(String, Notes)>, i64>;

/// What waits for a response.
enum Pending {
    Plain(oneshot::Sender<Result<Value, i64>>),
    /// A `session/new`: its session's notifications open with the response.
    Session(oneshot::Sender<SessionOpened>),
}

/// What the reader and the callers share.
struct Routes {
    pending: Mutex<HashMap<u64, Pending>>,
    sessions: Mutex<HashMap<String, Session>>,
    ended: Mutex<Option<String>>,
    policy: Policy,
}

/// A session's notifications, and the tool names its tool calls carried.
struct Session {
    notes: mpsc::UnboundedSender<Value>,
    tools: HashMap<String, String>,
}

/// One running agent.
pub struct Connection {
    writer: Mutex<Option<mpsc::UnboundedSender<Value>>>,
    routes: Arc<Routes>,
    next_id: AtomicU64,
    process: Mutex<Option<(Child, Group)>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Connection {
    /// Serves a started agent's stdin and stdout.
    pub(crate) fn start(
        mut child: Child,
        group: Group,
        policy: Policy,
    ) -> Result<Connection, String> {
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(format!("{} has no pipes", policy.label));
        };
        let routes = Arc::new(Routes {
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            ended: Mutex::new(None),
            policy,
        });
        let (writer, mut lines) = mpsc::unbounded_channel::<Value>();
        let write = tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(message) = lines.recv().await {
                let mut line = message.to_string();
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let read = tokio::spawn(read_all(stdout, Arc::clone(&routes), writer.downgrade()));
        Ok(Connection {
            writer: Mutex::new(Some(writer)),
            routes,
            next_id: AtomicU64::new(0),
            process: Mutex::new(Some((child, group))),
            tasks: Mutex::new(vec![write, read]),
        })
    }

    /// Whether the agent is still there to talk to.
    pub fn alive(&self) -> bool {
        locked(&self.routes.ended).is_none() && locked(&self.tasks).iter().all(|t| !t.is_finished())
    }

    /// Why the connection ended.
    pub fn ended(&self) -> String {
        locked(&self.routes.ended)
            .clone()
            .unwrap_or_else(|| format!("{} exited", self.routes.policy.label))
    }

    fn write(&self, message: Value) -> bool {
        locked(&self.writer)
            .as_ref()
            .is_some_and(|writer| writer.send(message).is_ok())
    }

    /// Sends a notification. Never waits: a dropped call can use it.
    pub fn notify(&self, method: &str, params: Value) {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Sends a request whose answer nobody waits for. Its id is text, so its answer is never taken
    /// for another request's. Never waits: a dropped call can use it.
    pub fn fire(&self, method: &str, params: Value) {
        let id = format!("fx-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn send(&self, method: &str, params: Value, pending: Pending) -> Result<(), Failure> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        locked(&self.routes.pending).insert(id, pending);
        if self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            Ok(())
        } else {
            locked(&self.routes.pending).remove(&id);
            Err(Failure::Ended(self.ended()))
        }
    }

    /// Sends a request and waits for its result.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, Failure> {
        let (answer, response) = oneshot::channel();
        self.send(method, params, Pending::Plain(answer))?;
        match response.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(code)) => Err(Failure::Refused(code)),
            Err(_) => Err(Failure::Ended(self.ended())),
        }
    }

    /// Sends `initialize` as a client that declares no file system and no terminal: the agent's
    /// answer.
    pub async fn initialize(&self, name: &str, version: &str) -> Result<Value, Failure> {
        let info = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
            "clientInfo": {"name": name, "version": version},
        });
        self.request("initialize", info).await
    }

    /// Sends `session/new`: the session's id and its notifications.
    pub async fn new_session(&self, params: Value) -> Result<(String, Notes), Failure> {
        let (answer, response) = oneshot::channel();
        self.send("session/new", params, Pending::Session(answer))?;
        match response.await {
            Ok(Ok(Some(opened))) => Ok(opened),
            Ok(Ok(None)) => Err(Failure::Ended(format!(
                "{} started no session",
                self.routes.policy.label
            ))),
            Ok(Err(code)) => Err(Failure::Refused(code)),
            Err(_) => Err(Failure::Ended(self.ended())),
        }
    }

    /// Sends `session/set_config_option`, setting `config` to `value`.
    pub async fn set_config_option(
        &self,
        session: &str,
        config: &str,
        value: &str,
    ) -> Result<Value, Failure> {
        let params = json!({"sessionId": session, "configId": config, "value": value});
        self.request("session/set_config_option", params).await
    }

    /// Sends `session/prompt` with one text block, and waits for the turn to end.
    pub async fn prompt(&self, session: &str, text: &str) -> Result<Value, Failure> {
        let params = json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]});
        self.request("session/prompt", params).await
    }

    /// Asks the agent to stop `session`'s turn. Never waits: a dropped call can use it.
    pub fn cancel(&self, session: &str) {
        self.notify("session/cancel", json!({"sessionId": session}));
    }

    /// Stops routing `session`'s notifications.
    pub fn forget(&self, session: &str) {
        locked(&self.routes.sessions).remove(session);
    }

    /// Closes stdin, ends the process group and waits for both tasks. Whether everything ended
    /// verifiably.
    pub async fn shutdown(&self) -> bool {
        locked(&self.writer).take();
        let mut verified = true;
        let process = locked(&self.process).take();
        if let Some((mut child, group)) = process {
            verified &= group.finish(&mut child).await;
        }
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *locked(&self.tasks));
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        verified
    }
}

/// What one line from the agent is.
enum Message {
    Response {
        id: u64,
        result: Result<Value, i64>,
    },
    Notification {
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

/// One line's message; `None` for anything else JSON-RPC allows (a response to a request whose
/// answer nobody waits for). An error response that names no request means the agent could not
/// read one of ours.
fn message_of(value: Value, label: &str) -> Result<Option<Message>, String> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = value.get("id").filter(|id| !id.is_null()).cloned();
    let params = || value.get("params").cloned().unwrap_or(Value::Null);
    Ok(match (method, id) {
        (Some(method), Some(id)) => Some(Message::Request {
            id,
            method,
            params: params(),
        }),
        (Some(_), None) => Some(Message::Notification { params: params() }),
        (None, Some(id)) => id.as_u64().map(|id| Message::Response {
            id,
            result: match value.get("error") {
                Some(error) => Err(error.get("code").and_then(Value::as_i64).unwrap_or(0)),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            },
        }),
        (None, None) if value.get("error").is_some() => {
            return Err(format!("{label} could not read a request"));
        }
        (None, None) => None,
    })
}

/// Reads every line of the agent's stdout and routes it (module doc); at the end, every waiting
/// call learns why.
async fn read_all(
    stdout: ChildStdout,
    routes: Arc<Routes>,
    writer: mpsc::WeakUnboundedSender<Value>,
) {
    let label = routes.policy.label;
    let mut stdout = BufReader::new(stdout);
    let reason = loop {
        let line = match read_line(&mut stdout, label).await {
            Ok(Some(line)) => line,
            Ok(None) => break format!("{label} exited"),
            Err(reason) => break reason,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            break format!("{label} sent a line that is not JSON");
        };
        match message_of(value, label) {
            Ok(Some(message)) => route(&routes, &writer, message),
            Ok(None) => {}
            Err(reason) => break reason,
        }
    };
    *locked(&routes.ended) = Some(reason);
    locked(&routes.pending).clear();
    locked(&routes.sessions).clear();
}

fn route(routes: &Routes, writer: &mpsc::WeakUnboundedSender<Value>, message: Message) {
    match message {
        Message::Response { id, result } => match locked(&routes.pending).remove(&id) {
            Some(Pending::Plain(answer)) => {
                let _ = answer.send(result);
            }
            Some(Pending::Session(answer)) => {
                let opened = result.map(|result| {
                    let session = result.get("sessionId").and_then(Value::as_str)?;
                    let (sender, notes) = mpsc::unbounded_channel();
                    let entry = Session {
                        notes: sender,
                        tools: HashMap::new(),
                    };
                    locked(&routes.sessions).insert(session.to_string(), entry);
                    Some((session.to_string(), notes))
                });
                let _ = answer.send(opened);
            }
            None => {}
        },
        Message::Notification { params } => {
            let Some(id) = params.get("sessionId").and_then(Value::as_str) else {
                return;
            };
            let mut sessions = locked(&routes.sessions);
            let Some(session) = sessions.get_mut(id) else {
                return;
            };
            let update = &params["update"];
            if let (Some(call), Some(name)) = (
                update.get("toolCallId").and_then(Value::as_str),
                (routes.policy.tool_name)(update),
            ) {
                session.tools.insert(call.to_string(), name);
            }
            let _ = session.notes.send(params);
        }
        Message::Request { id, method, params } => {
            let answer = if method == "session/request_permission" {
                json!({"jsonrpc": "2.0", "id": id, "result": permission(routes, &params)})
            } else {
                let error =
                    json!({"code": METHOD_NOT_FOUND, "message": "grida-fx answers no requests"});
                json!({"jsonrpc": "2.0", "id": id, "error": error})
            };
            if let Some(writer) = writer.upgrade() {
                let _ = writer.send(answer);
            }
        }
    }
}

/// The answer to a `session/request_permission` (module doc).
fn permission(routes: &Routes, params: &Value) -> Value {
    let call = &params["toolCall"];
    let named = (routes.policy.tool_name)(call).or_else(|| {
        let session = params.get("sessionId").and_then(Value::as_str)?;
        let id = call.get("toolCallId").and_then(Value::as_str)?;
        locked(&routes.sessions)
            .get(session)?
            .tools
            .get(id)
            .cloned()
    });
    let allowed = named.is_some_and(|name| routes.policy.allowed_tools.contains(&name.as_str()));
    let kind = if allowed { "allow_once" } else { "reject_once" };
    let option = params["options"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|option| option.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|option| option.get("optionId").cloned());
    match option {
        Some(id) => json!({"outcome": {"outcome": "selected", "optionId": id}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes(allowed: &'static [&'static str]) -> Routes {
        Routes {
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            ended: Mutex::new(None),
            policy: Policy {
                label: "Acme",
                allowed_tools: allowed,
                tool_name: |call| {
                    call.get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                },
            },
        }
    }

    fn options() -> Value {
        json!([
            {"optionId": "a1", "name": "Allow", "kind": "allow_once"},
            {"optionId": "a2", "name": "Always", "kind": "allow_always"},
            {"optionId": "r1", "name": "Reject", "kind": "reject_once"},
        ])
    }

    #[test]
    fn permission_is_granted_once_for_an_allowed_tool_only() {
        let routes = routes(&["draw"]);
        let ask = |title: &str| {
            permission(
                &routes,
                &json!({"sessionId": "s", "toolCall": {"toolCallId": "c", "title": title}, "options": options()}),
            )
        };
        assert_eq!(
            ask("draw"),
            json!({"outcome": {"outcome": "selected", "optionId": "a1"}})
        );
        assert_eq!(
            ask("shell"),
            json!({"outcome": {"outcome": "selected", "optionId": "r1"}})
        );
        let none = permission(
            &routes,
            &json!({"sessionId": "s", "toolCall": {"toolCallId": "c", "title": "shell"}, "options": []}),
        );
        assert_eq!(none, json!({"outcome": {"outcome": "cancelled"}}));
    }

    #[test]
    fn a_permission_names_its_tool_by_the_session_s_earlier_tool_call() {
        let routes = routes(&["draw"]);
        let (notes, _keep) = mpsc::unbounded_channel();
        let mut tools = HashMap::new();
        tools.insert("c".to_string(), "draw".to_string());
        locked(&routes.sessions).insert("s".into(), Session { notes, tools });
        let asked = permission(
            &routes,
            &json!({"sessionId": "s", "toolCall": {"toolCallId": "c"}, "options": options()}),
        );
        assert_eq!(
            asked,
            json!({"outcome": {"outcome": "selected", "optionId": "a1"}})
        );
        let other = permission(
            &routes,
            &json!({"sessionId": "t", "toolCall": {"toolCallId": "c"}, "options": options()}),
        );
        assert_eq!(
            other,
            json!({"outcome": {"outcome": "selected", "optionId": "r1"}})
        );
    }

    #[test]
    fn messages_are_read_by_their_shape() {
        let read = |value: Value| message_of(value, "Acme");
        assert!(matches!(
            read(json!({"id": 3, "result": {}})),
            Ok(Some(Message::Response {
                id: 3,
                result: Ok(_)
            }))
        ));
        assert!(matches!(
            read(json!({"id": 3, "error": {"code": -32602, "message": "/Users/a"}})),
            Ok(Some(Message::Response {
                id: 3,
                result: Err(-32602)
            }))
        ));
        assert!(matches!(
            read(json!({"id": 7, "method": "session/request_permission", "params": {}})),
            Ok(Some(Message::Request { .. }))
        ));
        assert!(matches!(
            read(json!({"method": "session/update", "params": {}})),
            Ok(Some(Message::Notification { .. }))
        ));
        // The answer to a request nobody waits for.
        assert!(matches!(
            read(json!({"id": "fx-4", "result": {}})),
            Ok(None)
        ));
        assert_eq!(
            read(json!({"id": null, "error": {"code": -32700}})).err(),
            Some("Acme could not read a request".to_string())
        );
    }
}
