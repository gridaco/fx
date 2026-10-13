//! The user's Codex as a runtime (spec/providers.md §1, §9.6): one `codex app-server` an
//! invocation starts on its first Codex call and keeps for the others, and shuts down with the run
//! ([`crate::adapter::Runtime`]).
//!
//! Starting it, once: check its version ([`super::version`]), list the user's MCP servers to turn
//! them off, start Codex in a process group of its own ([`crate::process_group`]), `initialize`,
//! then `account/read`; a Codex that is too old or not signed in with ChatGPT is refused for the
//! rest of the invocation. Two tasks then serve the connection: a writer, which owns Codex's
//! stdin and writes the lines it is sent, so even a dropped call can ask for its turn to be
//! interrupted without waiting; and a reader, which hands each response to the request waiting
//! for it and each notification to the thread it names, and declines every request Codex sends
//! (an approval, a question). A call has a thread of its own
//! ([`Connection::start_thread`]); a response to `thread/start` opens the thread's notifications
//! before the reader reads on, so none is missed. A response to `turn/start` whose call is gone
//! (dropped before Codex named its turn) has the reader interrupt the turn it names
//! ([`Connection::start_turn`]). A request Codex rejects as overloaded was not received
//! ([`super::Stop::NotReceived`]).
//!
//! A connection whose Codex exited is replaced by the next call. Shutting down closes stdin and
//! ends the process group ([`Group::finish`]).

use super::Stop;
use super::protocol::{self, Message};
use crate::BoxFuture;
use crate::adapter::Runtime;
use crate::installed::{Program, locked, read_line};
use crate::process_group::Group;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// A notification of one thread.
pub(super) struct Note {
    pub(super) method: String,
    pub(super) params: Value,
}

/// A `thread/start` response, with its thread's notifications when it started one.
type ThreadOpened = (Result<Value, i64>, Option<mpsc::UnboundedReceiver<Note>>);

/// What waits for a response.
enum Pending {
    Plain(oneshot::Sender<Result<Value, i64>>),
    /// A `thread/start`: its thread's notifications open with the response.
    Thread(oneshot::Sender<ThreadOpened>),
    /// A `turn/start` of `thread`: the reader interrupts the turn it starts when nobody is left
    /// to read the response.
    Turn {
        thread: String,
        answer: oneshot::Sender<Result<Value, i64>>,
    },
}

/// What the reader and the callers share.
#[derive(Default)]
struct Routes {
    pending: Mutex<HashMap<u64, Pending>>,
    threads: Mutex<HashMap<String, mpsc::UnboundedSender<Note>>>,
    /// Why the connection ended, once it has.
    ended: Mutex<Option<String>>,
}

/// One running Codex.
pub(super) struct Connection {
    writer: Mutex<Option<mpsc::UnboundedSender<Value>>>,
    routes: Arc<Routes>,
    next_id: AtomicU64,
    /// Codex's version, from `initialize`.
    pub(super) version: Option<String>,
    process: tokio::sync::Mutex<Option<(Child, Group)>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Codex's working folder, removed after it ends.
    _folder: tempfile::TempDir,
}

impl Connection {
    /// Starts Codex in `folder` with `args` and serves its stdin and stdout.
    fn start(
        mut child: Child,
        group: Group,
        folder: tempfile::TempDir,
    ) -> Result<Connection, Stop> {
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(Stop::Failed("Codex has no pipes".into()));
        };
        let routes = Arc::new(Routes::default());
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
            version: None,
            process: tokio::sync::Mutex::new(Some((child, group))),
            tasks: Mutex::new(vec![write, read]),
            _folder: folder,
        })
    }

    /// Whether Codex is still there to talk to.
    fn alive(&self) -> bool {
        locked(&self.routes.ended).is_none() && locked(&self.tasks).iter().all(|t| !t.is_finished())
    }

    pub(super) fn ended(&self) -> String {
        locked(&self.routes.ended)
            .clone()
            .unwrap_or_else(|| "Codex exited".into())
    }

    /// Sends a notification, or any message that wants no answer. Never waits: a dropped call
    /// can use it.
    pub(super) fn notify(&self, message: Value) {
        if let Some(writer) = locked(&self.writer).as_ref() {
            let _ = writer.send(message);
        }
    }

    fn send(&self, method: &str, params: Value, pending: Pending) -> Result<u64, Stop> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        locked(&self.routes.pending).insert(id, pending);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let sent = locked(&self.writer)
            .as_ref()
            .is_some_and(|writer| writer.send(message).is_ok());
        if sent {
            Ok(id)
        } else {
            locked(&self.routes.pending).remove(&id);
            Err(Stop::Failed(self.ended()))
        }
    }

    /// Sends a request and waits for its response. An error response is a refusal of the request.
    pub(super) async fn request(&self, method: &'static str, params: Value) -> Result<Value, Stop> {
        let (answer, response) = oneshot::channel();
        self.send(method, params, Pending::Plain(answer))?;
        self.answer(method, response.await)
    }

    /// Sends `turn/start` on `thread` and waits for its response. Dropped before it, the call
    /// leaves the response to the reader, which interrupts the turn it names.
    pub(super) async fn start_turn(&self, thread: &str, params: Value) -> Result<Value, Stop> {
        let (answer, response) = oneshot::channel();
        let pending = Pending::Turn {
            thread: thread.to_string(),
            answer,
        };
        self.send("turn/start", params, pending)?;
        self.answer("turn/start", response.await)
    }

    fn answer(
        &self,
        method: &str,
        response: Result<Result<Value, i64>, oneshot::error::RecvError>,
    ) -> Result<Value, Stop> {
        match response {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(code)) => Err(refusal(method, code)),
            Err(_) => Err(Stop::Failed(self.ended())),
        }
    }

    /// Starts a thread: its `thread/start` result and its notifications.
    pub(super) async fn start_thread(
        &self,
        params: Value,
    ) -> Result<(Value, mpsc::UnboundedReceiver<Note>), Stop> {
        let (answer, response) = oneshot::channel();
        self.send("thread/start", params, Pending::Thread(answer))?;
        match response.await {
            Ok((Ok(result), Some(notes))) => Ok((result, notes)),
            Ok((Ok(_), None)) => Err(Stop::Failed("Codex started no thread".into())),
            Ok((Err(code), _)) => Err(refusal("thread/start", code)),
            Err(_) => Err(Stop::Failed(self.ended())),
        }
    }

    /// Stops routing `thread`'s notifications.
    pub(super) fn forget(&self, thread: &str) {
        locked(&self.routes.threads).remove(thread);
    }

    /// Closes stdin, ends the process group and waits for both tasks. Whether everything ended
    /// verifiably.
    async fn shutdown(&self) -> bool {
        locked(&self.writer).take();
        let mut verified = true;
        if let Some((mut child, group)) = self.process.lock().await.take() {
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

/// An error response to `method`: not received when Codex was overloaded, else a failure.
fn refusal(method: &str, code: i64) -> Stop {
    if code == protocol::OVERLOADED {
        Stop::NotReceived(protocol::OVERLOADED_REASON.into())
    } else {
        Stop::Failed(format!("Codex refused {method} (error {code})"))
    }
}

/// Reads every line of Codex's stdout and routes it (module doc); at the end, every waiting call
/// learns why.
async fn read_all(
    stdout: ChildStdout,
    routes: Arc<Routes>,
    writer: mpsc::WeakUnboundedSender<Value>,
) {
    let mut stdout = BufReader::new(stdout);
    let reason = loop {
        let line = match read_line(&mut stdout, "Codex").await {
            Ok(Some(line)) => line,
            Ok(None) => break "Codex exited".to_string(),
            Err(reason) => break reason,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            break "Codex sent a line that is not JSON".to_string();
        };
        match protocol::message_of(value) {
            Ok(Some(message)) => route(&routes, &writer, message),
            Ok(None) => {}
            Err(stop) => break stop.into_reason(),
        }
    };
    *locked(&routes.ended) = Some(reason);
    locked(&routes.pending).clear();
    locked(&routes.threads).clear();
}

fn route(routes: &Routes, writer: &mpsc::WeakUnboundedSender<Value>, message: Message) {
    match message {
        Message::Response { id, result } => match locked(&routes.pending).remove(&id) {
            Some(Pending::Plain(answer)) => {
                let _ = answer.send(result);
            }
            Some(Pending::Thread(answer)) => {
                let thread = result
                    .as_ref()
                    .ok()
                    .and_then(|result| protocol::text_at(result, &["thread", "id"]));
                let notes = thread.map(|thread| {
                    let (sender, notes) = mpsc::unbounded_channel();
                    locked(&routes.threads).insert(thread, sender);
                    notes
                });
                let _ = answer.send((result, notes));
            }
            Some(Pending::Turn { thread, answer }) => {
                // The call is gone: stop the turn nobody will read.
                if let Err(Ok(result)) = answer.send(result)
                    && let Some(turn) = protocol::text_at(&result, &["turn", "id"])
                    && let Some(writer) = writer.upgrade()
                {
                    let _ = writer.send(protocol::interrupt(&thread, &turn));
                }
            }
            None => {}
        },
        Message::Notification { method, params } => {
            let thread = params.get("threadId").and_then(Value::as_str);
            if let Some(sender) = thread.and_then(|t| locked(&routes.threads).get(t).cloned()) {
                let _ = sender.send(Note { method, params });
            }
        }
        Message::Request { id } => {
            let error = json!({"code": -32601, "message": "grida-fx answers no requests"});
            if let Some(writer) = writer.upgrade() {
                let _ = writer.send(json!({"jsonrpc": "2.0", "id": id, "error": error}));
            }
        }
    }
}

enum State {
    Idle,
    /// Codex cannot draw on the user's plan: every call of the invocation is refused.
    Refused(String),
    Running(Arc<Connection>),
}

/// The runtime (module doc): no Codex runs until a call needs one.
pub struct CodexRuntime {
    /// The program, or the refusal of every call.
    program: Result<Program, String>,
    state: tokio::sync::Mutex<State>,
    /// The user's usage limit, once a call found it reached: `Some(resets_at)`, the unix time it
    /// resets when Codex gave it. No call starts while it holds.
    limit: Mutex<Option<Option<i64>>>,
}

impl CodexRuntime {
    pub fn new(program: Result<Program, String>) -> CodexRuntime {
        CodexRuntime {
            program,
            state: tokio::sync::Mutex::new(State::Idle),
            limit: Mutex::new(None),
        }
    }

    /// A call found the usage limit reached; it holds until `resets_at` (for the rest of the
    /// invocation when Codex gave no time).
    pub(super) fn reached(&self, resets_at: Option<i64>) {
        *locked(&self.limit) = Some(resets_at);
    }

    /// The refusal of a new call while the usage limit holds, at unix time `now`.
    pub(super) fn limited(&self, now: i64) -> Option<String> {
        let mut limit = locked(&self.limit);
        match *limit {
            None => None,
            Some(Some(at)) if now >= at => {
                *limit = None;
                None
            }
            Some(Some(at)) => Some(format!(
                "the Codex usage limit is reached; it resets at unix time {at}, and no Codex \
                 call starts before then"
            )),
            Some(None) => Some(
                "the Codex usage limit is reached, and no Codex call starts again in this run"
                    .to_string(),
            ),
        }
    }

    /// The running Codex, started when there is none or the last one ended.
    pub(super) async fn connection(&self) -> Result<Arc<Connection>, Stop> {
        let mut state = self.state.lock().await;
        match &*state {
            State::Refused(reason) => return Err(Stop::Refused(reason.clone())),
            State::Running(connection) if connection.alive() => return Ok(Arc::clone(connection)),
            State::Running(connection) => {
                connection.shutdown().await;
            }
            State::Idle => {}
        }
        *state = State::Idle;
        let program = self.program.clone().map_err(Stop::Refused)?;
        match open(&program).await {
            Ok(connection) => {
                let connection = Arc::new(connection);
                *state = State::Running(Arc::clone(&connection));
                Ok(connection)
            }
            Err(Stop::Refused(reason)) => {
                *state = State::Refused(reason.clone());
                Err(Stop::Refused(reason))
            }
            Err(stop) => Err(stop),
        }
    }
}

impl Runtime for CodexRuntime {
    fn shutdown(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            let verified = match &*state {
                State::Running(connection) => connection.shutdown().await,
                State::Idle | State::Refused(_) => true,
            };
            *state = State::Idle;
            verified
        })
    }
}

/// Starts a Codex and checks its version and sign-in (module doc).
async fn open(program: &Program) -> Result<Connection, Stop> {
    let folder = tempfile::Builder::new()
        .prefix("grida-fx-codex-")
        .tempdir()
        .map_err(|error| Stop::Failed(grida_fx_core::error::io_reason(&error)))?;
    let version = super::version(&program.path, folder.path())
        .await
        .map_err(Stop::Refused)?;
    super::check_version(program, &version).map_err(Stop::Refused)?;
    let program = program.path.as_path();
    let servers = super::mcp_servers(program, folder.path())
        .await
        .map_err(Stop::Refused)?;
    let mut args = vec!["app-server".to_string()];
    args.extend(super::settings(servers));
    let mut child = super::command(program, &args, folder.path())
        .spawn()
        .map_err(|error| Stop::Refused(super::not_started(&error)))?;
    let Some(group) = Group::enlist(&mut child) else {
        return Err(Stop::Failed("the command is stopping".into()));
    };
    let mut connection = Connection::start(child, group, folder)?;
    let greeted = async {
        let info =
            json!({"clientInfo": {"name": "grida-fx", "version": env!("CARGO_PKG_VERSION")}});
        let initialized = connection.request("initialize", info).await?;
        connection.notify(json!({"jsonrpc": "2.0", "method": "initialized"}));
        let account = connection.request("account/read", json!({})).await?;
        protocol::check_account(&account)?;
        Ok::<Value, Stop>(initialized)
    };
    match greeted.await {
        Ok(initialized) => {
            connection.version = protocol::version_of(&initialized);
            Ok(connection)
        }
        Err(stop) => {
            connection.shutdown().await;
            Err(stop)
        }
    }
}
