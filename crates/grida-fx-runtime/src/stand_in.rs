//! Stand-in answers (spec/protocol.md §5.7; §6.1 "Stand-in answers"; spec/store.md §8 "Stand-in
//! runs").
//!
//! A stand-in run answers the paid calls the call cache cannot answer with a function the caller
//! supplies, instead of a provider. The engine reaches the function through an [`Answerer`]: a
//! peer that speaks the node protocol's transport and only answers. [`ConnectionAnswerer`] is the
//! one answerer FX has, over a [`Connection`], for both sources:
//! - `--stand-in <file>.py#<function>`: a **stand-in host** ([`ConnectionAnswerer::start_host`]),
//!   the node host program started in the engine's working directory, outside the pool, with the
//!   planning project as its project; `initialize`, then `stand_in.load {path, function}`;
//! - `--stand-in -`: a socket the answerer already holds the other end of
//!   ([`ConnectionAnswerer::start_socket`]); `initialize` only.
//!
//! Asking ([`Answerer::answer`]) sends `stand_in.answer` and waits for its result or for the
//! calling instance's cancellation, whichever comes first. On cancellation it sends `$/cancel`
//! for the request, ends at once with [`AnswererError::Cancelled`], and discards whatever arrives
//! later. The result maps as §5.7 says:
//! - an answer: [`Reply::Answer`], its files decoded from base64 ([`StandInFile`]);
//! - `{"decline": true}`: [`Reply::Decline`];
//! - a `capability_refused` error: [`AnswererError::Refused`] with its message; a `call_failed`
//!   error: [`AnswererError::Failed`];
//! - anything else is a fault of the stand-in ([`AnswererError::Fault`], the run's `stopped`
//!   sentence): another error, `the stand-in failed: <message>`; a result FX cannot read, `the
//!   stand-in answered something FX cannot read: <where>`; the answerer gone, `the stand-in
//!   answerer exited`.
//!
//! **Losing the answerer.** A [`ConnectionAnswerer`] watches its peer from the moment it has
//! started, not only when it next asks: [`Answerer::lost`] resolves once the answerer is gone
//! before [`Answerer::shutdown`] began, and the runner then stops the run with
//! [`ANSWERER_EXITED`]. A peer on a socket is gone when its end of the connection ends. A stand-in
//! host is gone when its connection ends or its process exits, whichever comes first; the
//! process is waited on, not only its output, because a process it forked can hold the protocol
//! pipes open after it has gone (`host::process`, module doc). When it exits, its process group
//! is killed, what it wrote before it exited is still read for 5 seconds, and then the
//! connection is ended, so a pending `stand_in.answer` fails with [`ANSWERER_EXITED`] instead of
//! waiting for an answer that cannot come.
//!
//! [`StandIn`] is what the engine holds in a stand-in run: the answerer and the answer checks
//! ([`StandInChecks`], from `grida_fx_providers`). The call path (`calls`) asks it at step 4 of
//! spec/protocol.md §6.1; [`StandIn::shutdown`] ends the answerer with the run (`shutdown`,
//! `exit`; a stand-in host is then waited for 5 seconds and killed).

use crate::engine::Cancel;
use crate::host::connection::{Connection, ConnectionError, NoIncoming};
use crate::host::process::{HostProcess, HostSpec};
use base64::Engine as _;
use grida_fx_core::{ENGINE_NAME, ENGINE_VERSION};
use grida_fx_protocol::{
    EngineInfo, ErrorCode, ErrorData, InitializeParams, InitializeResult, PROTOCOL,
    StandInAnswerParams, StandInAnswerResult, StandInLoadParams, method,
};
use grida_fx_providers::BoxFuture;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

pub use grida_fx_providers::stand_in::{StandInChecks, StandInFile};

/// The `stopped` sentence of an answerer that went away (spec/protocol.md §5.7).
pub const ANSWERER_EXITED: &str = "the stand-in answerer exited";

/// How long a polite end of an answerer on a socket may take.
const GRACE: Duration = Duration::from_secs(5);

/// What a stand-in answered a call with (module doc).
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// An answer: its files by name, and its `data`.
    Answer {
        files: IndexMap<String, StandInFile>,
        data: Value,
    },
    /// `{"decline": true}`: the call goes on at the live check.
    Decline,
}

/// Why a stand-in gave no [`Reply`] (module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswererError {
    /// `capability_refused`: the message is the call's reason.
    Refused(String),
    /// `call_failed`: the message is the call's reason.
    Failed(String),
    /// A fault of the stand-in, which stops the run: the run's `stopped` sentence.
    Fault(String),
    /// The calling instance was cancelled before the answer came.
    Cancelled,
}

/// A peer that answers `stand_in.answer` (module doc).
pub trait Answerer: Send + Sync {
    /// Asks for the answer to one call, waiting for it or for `cancel`.
    fn answer<'a>(
        &'a self,
        params: StandInAnswerParams,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Reply, AnswererError>>;

    /// Ends the answerer: `shutdown`, `exit`, and its end of the channel closed.
    fn shutdown(&self) -> BoxFuture<'_, ()>;

    /// Resolves once the answerer is gone before [`Answerer::shutdown`] began (module doc,
    /// "Losing the answerer"); never, for an answerer that cannot be lost.
    fn lost(&self) -> BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

/// What a stand-in run's engine holds (module doc).
pub struct StandIn {
    answerer: Arc<dyn Answerer>,
    checks: StandInChecks,
}

impl StandIn {
    /// A stand-in answered by `answerer`, its answers held to the checks of
    /// spec/protocol.md §6.1.
    pub fn new(answerer: Arc<dyn Answerer>) -> StandIn {
        StandIn {
            answerer,
            checks: StandInChecks::new(),
        }
    }

    /// The answerer.
    pub fn answerer(&self) -> &Arc<dyn Answerer> {
        &self.answerer
    }

    /// The checks a stand-in's request and answer are held to.
    pub fn checks(&self) -> &StandInChecks {
        &self.checks
    }

    /// Ends the answerer (module doc).
    pub async fn shutdown(&self) {
        self.answerer.shutdown().await;
    }

    /// Resolves once the answerer is gone while the run still runs ([`Answerer::lost`]).
    pub async fn lost(&self) {
        self.answerer.lost().await;
    }
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StandIn").finish_non_exhaustive()
    }
}

/// An answerer over a protocol connection (module doc).
pub struct ConnectionAnswerer {
    connection: Connection,
    /// The stand-in host behind the connection, when the engine started one.
    host: Arc<tokio::sync::Mutex<Option<HostProcess>>>,
    /// Fired when [`Answerer::shutdown`] begins: from then on, the peer going away is expected.
    ending: Cancel,
    /// Set once the peer is gone before the shutdown began (module doc, "Losing the answerer").
    lost: Arc<tokio::sync::watch::Sender<bool>>,
}

impl ConnectionAnswerer {
    /// An answerer on a connection whose session is already initialized, with `host` behind it
    /// (a stand-in host). Call it on the runtime: it starts watching the peer (module doc).
    fn over(connection: Connection, host: Option<HostProcess>) -> ConnectionAnswerer {
        let answerer = ConnectionAnswerer {
            connection,
            host: Arc::new(tokio::sync::Mutex::new(host)),
            ending: Cancel::new(),
            lost: Arc::new(tokio::sync::watch::channel(false).0),
        };
        answerer.watch();
        answerer
    }

    /// Watches the peer until it is gone or the shutdown begins (module doc, "Losing the
    /// answerer").
    fn watch(&self) {
        let connection = self.connection.clone();
        let host = Arc::clone(&self.host);
        let ending = self.ending.clone();
        let lost = Arc::clone(&self.lost);
        tokio::spawn(async move {
            let gone = async {
                let mut host = host.lock().await;
                let Some(process) = host.as_mut() else {
                    connection.closed().await;
                    return;
                };
                tokio::select! {
                    biased;
                    () = connection.closed() => {}
                    _ = process.exited() => {
                        // Its group is killed by now, which closed the pipes a forked process
                        // held; what it wrote before it exited is still read.
                        if tokio::time::timeout(GRACE, connection.closed()).await.is_err() {
                            connection.abandon();
                        }
                    }
                }
            };
            tokio::select! {
                biased;
                () = ending.cancelled() => {}
                () = gone => {
                    if !ending.is_cancelled() {
                        lost.send_replace(true);
                    }
                }
            }
        });
    }

    /// The answerer on the other end of a socket (`--stand-in -`): starts the connection on the
    /// current runtime and sends `initialize` with the planning project's root and sources. The
    /// error is the reason, for `the stand-in on standard input could not be started: <reason>`.
    pub async fn start_socket<R, W>(
        reader: R,
        writer: W,
        project_root: &Path,
        sources: &[String],
    ) -> Result<ConnectionAnswerer, String>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let connection = Connection::start(reader, writer, Arc::new(NoIncoming));
        match initialize(&connection, project_root, sources).await {
            Ok(()) => Ok(ConnectionAnswerer::over(connection, None)),
            Err(reason) => {
                connection.close_input().await;
                Err(reason)
            }
        }
    }

    /// A stand-in host (`--stand-in <file>.py#<function>`): the node host program of `spec`
    /// (the planning project's interpreter, root and sources) started in `cwd`, then
    /// `stand_in.load {path, function}` with the file's absolute `path`. The error is the reason,
    /// for `the stand-in <file>#<function> could not be loaded: <reason>`.
    pub async fn start_host(
        spec: &HostSpec,
        cwd: &Path,
        path: &Path,
        function: &str,
    ) -> Result<ConnectionAnswerer, String> {
        let Some(path) = path.to_str() else {
            return Err("its path is not UTF-8, which the node protocol needs".into());
        };
        let mut host = HostProcess::start_in(spec, cwd, Arc::new(NoIncoming)).await?;
        let params = serde_json::to_value(StandInLoadParams {
            path: path.to_string(),
            function: function.to_string(),
        })
        .map_err(|error| crate::host::read_reason(&error))?;
        let connection = host.connection().clone();
        let loaded = {
            let asking = connection.clone();
            let request = asking.request(method::STAND_IN_LOAD, Some(params));
            tokio::pin!(request);
            host.answer_or_exit(request, crate::host::GRACE).await
        };
        let failure = match loaded {
            Ok(Value::Object(members)) if members.is_empty() => {
                return Ok(ConnectionAnswerer::over(connection, Some(host)));
            }
            Ok(_) => {
                "the stand-in host answered stand_in.load with something FX cannot read".to_string()
            }
            Err(ConnectionError::Rpc(error)) => error.message,
            Err(ConnectionError::Closed) => {
                let status = host.exit_status();
                format!(
                    "the stand-in host {} before it answered stand_in.load",
                    crate::host::exit_text(status)
                )
            }
            Err(ConnectionError::Protocol(message)) => {
                format!("the stand-in host broke the protocol: {message}")
            }
        };
        host.shutdown().await;
        Err(failure)
    }

    /// The connection to the answerer.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

/// `initialize` on a fresh connection (spec/protocol.md §2): the answer's protocol must be ours.
async fn initialize(
    connection: &Connection,
    project_root: &Path,
    sources: &[String],
) -> Result<(), String> {
    let Some(project_root) = project_root.to_str() else {
        return Err("the project folder's path is not UTF-8, which the node protocol needs".into());
    };
    let params = serde_json::to_value(InitializeParams {
        protocol: PROTOCOL.into(),
        engine: EngineInfo {
            name: ENGINE_NAME.into(),
            version: ENGINE_VERSION.into(),
        },
        project_root: project_root.into(),
        sources: sources.to_vec(),
    })
    .map_err(|error| crate::host::read_reason(&error))?;
    match connection.request(method::INITIALIZE, Some(params)).await {
        Ok(value) => match serde_json::from_value::<InitializeResult>(value) {
            Ok(info) if info.protocol == PROTOCOL => Ok(()),
            Ok(info) => Err(crate::host::mismatch(
                Some(&info.host.sdk_version),
                &info.protocol,
            )),
            Err(error) => Err(format!(
                "it answered initialize with something FX cannot read: {}",
                crate::host::read_reason(&error)
            )),
        },
        Err(ConnectionError::Rpc(error)) if error.kind() == Some(ErrorCode::ProtocolMismatch) => {
            let host_protocol = error
                .data
                .clone()
                .and_then(|data| serde_json::from_value::<ErrorData>(data).ok())
                .and_then(|data| data.host_protocol);
            Err(match host_protocol {
                Some(host_protocol) => crate::host::mismatch(None, &host_protocol),
                None => error.message,
            })
        }
        Err(ConnectionError::Rpc(error)) => {
            Err(format!("it refused initialize: {}", error.message))
        }
        Err(ConnectionError::Closed) => {
            Err("it closed the socket before it answered initialize".into())
        }
        Err(ConnectionError::Protocol(message)) => Err(format!("it broke the protocol: {message}")),
    }
}

impl Answerer for ConnectionAnswerer {
    fn answer<'a>(
        &'a self,
        params: StandInAnswerParams,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Reply, AnswererError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(AnswererError::Cancelled);
            }
            let params = serde_json::to_value(params).map_err(|error| {
                AnswererError::Fault(format!(
                    "the stand-in failed: its request cannot be written: {}",
                    crate::host::read_reason(&error)
                ))
            })?;
            // The request runs on a task of its own: when the instance is cancelled it still sends
            // `$/cancel`, and an answer that arrives later is discarded with it.
            let connection = self.connection.clone();
            let token = cancel.clone();
            let asked = tokio::spawn(async move {
                connection
                    .request_cancellable(method::STAND_IN_ANSWER, Some(params), &token)
                    .await
            });
            let answered = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(AnswererError::Cancelled),
                joined = asked => joined.unwrap_or(Err(ConnectionError::Closed)),
            };
            reply(answered, cancel)
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.ending.cancel();
            if let Some(host) = self.host.lock().await.take() {
                host.shutdown().await;
                return;
            }
            if self.connection.is_open() {
                let connection = self.connection.clone();
                let _ = tokio::time::timeout(GRACE, async move {
                    let _ = connection.request(method::SHUTDOWN, None).await;
                    let _ = connection.notify(method::EXIT, None).await;
                })
                .await;
            }
            let _ = tokio::time::timeout(GRACE, self.connection.close_input()).await;
        })
    }

    fn lost(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut lost = self.lost.subscribe();
            if lost.wait_for(|lost| *lost).await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}

/// A `stand_in.answer` result as a [`Reply`] (module doc).
fn reply(
    answered: Result<Value, ConnectionError>,
    cancel: &Cancel,
) -> Result<Reply, AnswererError> {
    let unreadable = |place: String| AnswererError::Fault(format!("{UNREADABLE}: {place}"));
    let value = match answered {
        Ok(value) => value,
        Err(ConnectionError::Rpc(error)) => {
            return Err(match error.kind() {
                Some(ErrorCode::CapabilityRefused) => AnswererError::Refused(error.message),
                Some(ErrorCode::CallFailed) => AnswererError::Failed(error.message),
                Some(ErrorCode::Cancelled) if cancel.is_cancelled() => AnswererError::Cancelled,
                _ => AnswererError::Fault(format!("the stand-in failed: {}", error.message)),
            });
        }
        Err(ConnectionError::Closed) => {
            return Err(AnswererError::Fault(ANSWERER_EXITED.to_string()));
        }
        Err(ConnectionError::Protocol(message)) => return Err(unreadable(message)),
    };
    let result: StandInAnswerResult = serde_json::from_value(value)
        .map_err(|error| unreadable(crate::host::read_reason(&error)))?;
    match result {
        StandInAnswerResult::Decline { .. } => Ok(Reply::Decline),
        StandInAnswerResult::Answer { files, data } => {
            let mut decoded = IndexMap::with_capacity(files.len());
            for (name, file) in files {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(file.base64.as_bytes())
                    .map_err(|_| {
                        unreadable(format!(
                            "files.{name}: its bytes are not base64 with padding"
                        ))
                    })?;
                decoded.insert(
                    name,
                    StandInFile {
                        kind: file.kind,
                        bytes,
                    },
                );
            }
            Ok(Reply::Answer {
                files: decoded,
                data,
            })
        }
    }
}

/// The start of the sentence for a result FX cannot read.
const UNREADABLE: &str = "the stand-in answered something FX cannot read";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn results_map_to_replies() {
        let cancel = Cancel::new();
        let answered = json!({
            "files": {"image": {"base64": "aGVsbG8=", "kind": "image/png"}, "raw": {"base64": ""}},
            "data": {"seen": 1}
        });
        let Ok(Reply::Answer { files, data }) = reply(Ok(answered), &cancel) else {
            panic!("an answer");
        };
        assert_eq!(files["image"].bytes, b"hello");
        assert_eq!(files["image"].kind.as_deref(), Some("image/png"));
        assert_eq!(files["raw"].bytes, b"");
        assert_eq!(files["raw"].kind, None);
        assert_eq!(data, json!({"seen": 1}));
        assert_eq!(
            reply(Ok(json!({"decline": true})), &cancel),
            Ok(Reply::Decline)
        );
    }

    #[test]
    fn errors_and_unreadable_results() {
        let cancel = Cancel::new();
        let rpc = |code, message: &str| {
            Err(ConnectionError::Rpc(grida_fx_protocol::RpcError::new(
                code, message,
            )))
        };
        assert_eq!(
            reply(rpc(ErrorCode::CapabilityRefused, "no kites"), &cancel),
            Err(AnswererError::Refused("no kites".into()))
        );
        assert_eq!(
            reply(rpc(ErrorCode::CallFailed, "torn"), &cancel),
            Err(AnswererError::Failed("torn".into()))
        );
        assert_eq!(
            reply(rpc(ErrorCode::Internal, "AssertionError: 2 != 3"), &cancel),
            Err(AnswererError::Fault(
                "the stand-in failed: AssertionError: 2 != 3".into()
            ))
        );
        // `cancelled` is a fault unless the engine cancelled the request.
        assert!(matches!(
            reply(rpc(ErrorCode::Cancelled, "cancelled"), &cancel),
            Err(AnswererError::Fault(_))
        ));
        cancel.cancel();
        assert_eq!(
            reply(rpc(ErrorCode::Cancelled, "cancelled"), &cancel),
            Err(AnswererError::Cancelled)
        );
        let cancel = Cancel::new();
        assert_eq!(
            reply(Err(ConnectionError::Closed), &cancel),
            Err(AnswererError::Fault(ANSWERER_EXITED.into()))
        );
        for (value, place) in [
            (json!({"decline": false}), ""),
            (json!({"files": {}}), ""),
            (json!(7), ""),
            (
                json!({"files": {"image": {"base64": "not base64!"}}, "data": null}),
                "files.image: its bytes are not base64 with padding",
            ),
        ] {
            let Err(AnswererError::Fault(sentence)) = reply(Ok(value.clone()), &cancel) else {
                panic!("{value} is a fault");
            };
            assert!(sentence.starts_with(UNREADABLE), "{sentence}");
            assert!(sentence.ends_with(place), "{sentence}");
        }
    }
}
