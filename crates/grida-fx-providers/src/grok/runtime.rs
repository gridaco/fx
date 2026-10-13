//! The user's Grok Build as a runtime (spec/adapters.md §1; src/grok/README.md): one `grok agent
//! --no-leader stdio` an invocation starts on its first Grok call and keeps for the others, and
//! shuts down with the run ([`crate::adapter::Runtime`]).
//!
//! Starting it, once: Grok Build in a scratch folder and a process group of its own, with the
//! environment of [`super::environment`]; then `initialize` (its version, at least
//! [`super::MIN_VERSION`], and its sign-in methods) and `authenticate` with `cached_token`. A Grok
//! Build that is not signed in, is older, or bills an API account is refused for the rest of the
//! invocation, and so is one whose image tools turned out to need another subscription
//! ([`GrokRuntime::refuse_the_rest`]). A connection whose Grok Build exited is replaced by the next
//! call. Shutting down closes stdin and ends the process group.

use super::{LABEL, Stop};
use crate::BoxFuture;
use crate::acp::{Connection, Failure, Policy};
use crate::adapter::Runtime;
use crate::installed::locked;
use crate::process_group::Group;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a shutdown waits for Grok Build to delete the sessions calls left open, in all.
const CLOSING: Duration = Duration::from_secs(5);

/// One running Grok Build.
pub(super) struct Started {
    pub(super) connection: Connection,
    /// Grok Build's version, from `initialize`.
    pub(super) version: Option<String>,
    /// The sessions of calls that have not deleted theirs.
    open: Mutex<BTreeSet<String>>,
    /// The tasks of dropped calls that end their sessions.
    endings: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Grok Build's working folder, and every session's (each call's pictures in a folder of
    /// their own within it), removed after it ends. One folder keeps what Grok Build records per
    /// working folder, such as its prompt history, to one per invocation.
    folder: tempfile::TempDir,
}

impl Started {
    pub(super) fn folder(&self) -> &Path {
        self.folder.path()
    }

    /// A call's session, open until [`Started::closed`].
    pub(super) fn opened(&self, session: &str) {
        locked(&self.open).insert(session.to_string());
    }

    pub(super) fn closed(&self, session: &str) {
        locked(&self.open).remove(session);
    }

    /// A dropped call's task that ends its session.
    pub(super) fn ending(&self, task: tokio::task::JoinHandle<()>) {
        let mut endings = locked(&self.endings);
        endings.retain(|ending| !ending.is_finished());
        endings.push(task);
    }

    /// Waits for dropped calls to end their sessions, then deletes any session still open, [`CLOSING`] at
    /// most in all; then ends Grok Build.
    async fn shutdown(self: &Arc<Self>) -> bool {
        let deadline = tokio::time::Instant::now() + CLOSING;
        let endings: Vec<_> = std::mem::take(&mut *locked(&self.endings));
        let _ = tokio::time::timeout_at(deadline, async {
            for task in endings {
                let _ = task.await;
            }
        })
        .await;
        let open: Vec<String> = std::mem::take(&mut *locked(&self.open))
            .into_iter()
            .collect();
        let deleting: Vec<_> = open
            .into_iter()
            .map(|session| {
                let started = Arc::clone(self);
                tokio::spawn(async move {
                    started.connection.cancel(&session);
                    let _ = started
                        .connection
                        .request("_x.ai/session/delete", json!({"sessionId": session}))
                        .await;
                })
            })
            .collect();
        let _ = tokio::time::timeout_at(deadline, async {
            for task in deleting {
                let _ = task.await;
            }
        })
        .await;
        self.connection.shutdown().await
    }
}

/// The runtime (module doc): no Grok Build runs until a call needs one.
pub struct GrokRuntime {
    /// The program, or the refusal of every call.
    program: Result<PathBuf, String>,
    running: tokio::sync::Mutex<Option<Arc<Started>>>,
    /// Grok Build cannot draw on the user's subscription: every later call of the invocation is
    /// refused.
    refused: Mutex<Option<String>>,
}

impl GrokRuntime {
    pub fn new(program: Result<PathBuf, String>) -> GrokRuntime {
        GrokRuntime {
            program,
            running: tokio::sync::Mutex::new(None),
            refused: Mutex::new(None),
        }
    }

    /// Grok Build cannot draw for this account: no later call starts.
    pub(super) fn refuse_the_rest(&self, reason: String) {
        *locked(&self.refused) = Some(reason);
    }

    /// The running Grok Build, started when there is none or the last one ended.
    pub(super) async fn started(&self) -> Result<Arc<Started>, Stop> {
        let mut running = self.running.lock().await;
        if let Some(reason) = locked(&self.refused).clone() {
            return Err(Stop::Refused(reason));
        }
        if let Some(started) = running.take() {
            if started.connection.alive() {
                *running = Some(Arc::clone(&started));
                return Ok(started);
            }
            started.connection.shutdown().await;
        }
        let program = self.program.clone().map_err(Stop::Refused)?;
        match open(&program).await {
            Ok(started) => {
                let started = Arc::new(started);
                *running = Some(Arc::clone(&started));
                Ok(started)
            }
            Err(Stop::Refused(reason)) => {
                self.refuse_the_rest(reason.clone());
                Err(Stop::Refused(reason))
            }
            Err(stop) => Err(stop),
        }
    }
}

impl Runtime for GrokRuntime {
    fn shutdown(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            match self.running.lock().await.take() {
                Some(started) => started.shutdown().await,
                None => true,
            }
        })
    }
}

/// What the reader grants: Grok's image tools, by the name Grok gives a tool call.
fn policy() -> Policy {
    Policy {
        label: "Grok Build",
        allowed_tools: &super::TOOLS,
        tool_name: super::protocol::tool_name,
    }
}

/// Starts a Grok Build and checks its version and sign-in (module doc).
async fn open(program: &Path) -> Result<Started, Stop> {
    let folder = tempfile::Builder::new()
        .prefix("grida-fx-grok-")
        .tempdir()
        .map_err(|error| Stop::Failed(grida_fx_core::error::io_reason(&error)))?;
    let mut child = match super::command(program, folder.path()).spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Stop::Refused(super::NOT_INSTALLED.into()));
        }
        Err(error) => {
            return Err(Stop::Refused(format!(
                "{LABEL} cannot start grok: {}",
                grida_fx_core::error::io_reason(&error)
            )));
        }
    };
    let Some(group) = Group::enlist(&mut child) else {
        return Err(Stop::Failed("the command is stopping".into()));
    };
    let connection = Connection::start(child, group, policy()).map_err(Stop::Failed)?;
    let greeted = async {
        let initialized = connection
            .initialize("grida-fx", env!("CARGO_PKG_VERSION"))
            .await
            .map_err(|failure| stop("initialize", failure))?;
        let version = super::protocol::version_of(&initialized);
        super::protocol::check_version(version.as_deref())?;
        if !super::protocol::offers_cached_token(&initialized) {
            return Err(Stop::Refused(super::NOT_SIGNED_IN.into()));
        }
        let account = connection
            .request("authenticate", json!({"methodId": "cached_token"}))
            .await
            .map_err(|failure| match failure {
                Failure::Refused(_) => Stop::Refused(super::NOT_SIGNED_IN.into()),
                Failure::Ended(reason) => Stop::Failed(reason),
            })?;
        super::protocol::check_account(&account)?;
        Ok::<Option<String>, Stop>(version)
    };
    match greeted.await {
        Ok(version) => Ok(Started {
            connection,
            version,
            open: Mutex::new(BTreeSet::new()),
            endings: Mutex::new(Vec::new()),
            folder,
        }),
        Err(stop) => {
            connection.shutdown().await;
            Err(stop)
        }
    }
}

/// A request that got no result: a failure, never Grok Build's own text.
pub(super) fn stop(method: &str, failure: Failure) -> Stop {
    match failure {
        Failure::Refused(code) => {
            Stop::Failed(format!("Grok Build refused {method} (error {code})"))
        }
        Failure::Ended(reason) => Stop::Failed(reason),
    }
}
