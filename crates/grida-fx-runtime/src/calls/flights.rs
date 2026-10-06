//! The paid calls of one invocation, by call key: which are in flight, which ended for good, and
//! how many calls are running at all.
//!
//! - **One flight per key.** A call key in flight is sent once. The first caller of a key leads
//!   ([`Joined::Lead`]): it makes the call, and when it ends it lands its outcome
//!   ([`Leader::land`]). A caller of the same key while it is in flight follows
//!   ([`Joined::Follow`]): it waits for the leader's outcome instead of sending, or reading a job
//!   record the leader is still writing. That covers two instances with one request, and a body
//!   run again (`retry: engine`) while its earlier run's call is still out. A leader that goes
//!   away without landing (its task was dropped) lets the next caller lead.
//! - **Ended keys.** A key whose call ended `call_failed`, `capability_refused` or `job_unsettled`
//!   is answered with that same error by every later call of the key in the invocation, without
//!   sending anything; `ceiling_exceeded` likewise, for later calls of the instance it refused (a
//!   rerun). Six sends of one request in all, however often a body runs.
//! - **Calls running.** Every call counts while it runs ([`Flights::track`]), followers included,
//!   and [`Flights::settled`] waits until none does: a run waits for it before it ends, so no paid
//!   call is left behind with a hold open (a step that timed out, or whose host exited, leaves its
//!   call to complete and settle; a stopped invocation's calls settle their holds in full).

use crate::calls::{CallAnswer, CallError};
use grida_fx_protocol::{ErrorCode, RpcError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// How a leader's call ended, as its followers see it.
pub type Landed = Arc<Result<CallAnswer, CallError>>;

/// One key's state.
enum Flight {
    /// In flight under the leader numbered `leader`: its outcome, once landed.
    Flying {
        leader: u64,
        outcome: watch::Receiver<Option<Landed>>,
    },
    /// Ended for good (module doc, "Ended keys"): the error, and the instance a ceiling refused.
    Ended {
        error: RpcError,
        only_for: Option<String>,
    },
}

/// The calls of one invocation (module doc).
#[derive(Default)]
pub struct Flights {
    keys: Mutex<HashMap<String, Flight>>,
    /// Numbers leaders, so a leader that goes away clears only its own flight.
    leaders: std::sync::atomic::AtomicU64,
    running: Running,
}

impl std::fmt::Debug for Flights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flights")
            .field("running", &self.running())
            .finish_non_exhaustive()
    }
}

/// What a caller of a key does.
pub enum Joined {
    /// Answer with this error: the key ended for good.
    Ended(RpcError),
    /// Make the call, then land its outcome.
    Lead(Leader),
    /// Wait for the leader's outcome.
    Follow(watch::Receiver<Option<Landed>>),
}

/// The leader of a key's flight. Dropped without landing, it lets the next caller lead.
pub struct Leader {
    flights: Arc<Flights>,
    key: String,
    instance_id: String,
    number: u64,
    sender: Option<watch::Sender<Option<Landed>>>,
}

impl Flights {
    pub fn new() -> Flights {
        Flights::default()
    }

    /// Joins `key`'s flight as `instance_id` (module doc).
    pub fn join(self: &Arc<Self>, key: &str, instance_id: &str) -> Joined {
        let mut keys = self.lock();
        match keys.get(key) {
            Some(Flight::Ended { error, only_for })
                if only_for.as_deref().is_none_or(|only| only == instance_id) =>
            {
                return Joined::Ended(error.clone());
            }
            // A leader that went away without landing closed its sender: lead in its place.
            Some(Flight::Flying { outcome, .. }) if outcome.has_changed().is_ok() => {
                return Joined::Follow(outcome.clone());
            }
            _ => {}
        }
        let number = self
            .leaders
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, outcome) = watch::channel(None);
        keys.insert(
            key.to_string(),
            Flight::Flying {
                leader: number,
                outcome,
            },
        );
        Joined::Lead(Leader {
            flights: Arc::clone(self),
            key: key.to_string(),
            instance_id: instance_id.to_string(),
            number,
            sender: Some(sender),
        })
    }

    /// Counts a call as running until the guard is dropped.
    pub fn track(self: &Arc<Self>) -> Tracked {
        self.running.add();
        Tracked {
            flights: Arc::clone(self),
        }
    }

    /// How many calls are running.
    pub fn running(&self) -> usize {
        *self.running.count.borrow()
    }

    /// Resolves once no call is running (module doc).
    pub async fn settled(&self) {
        let mut receiver = self.running.count.subscribe();
        // The sender lives as long as `self`, so the wait ends only when the count is 0.
        let _ = receiver.wait_for(|running| *running == 0).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Flight>> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Leader {
    /// Publishes how the call ended to its followers, and keeps an error that ends the key for
    /// good (module doc).
    pub fn land(mut self, outcome: &Result<CallAnswer, CallError>) {
        let ended = match outcome {
            Err(CallError::Rpc(error)) => match error.kind() {
                Some(
                    ErrorCode::CallFailed | ErrorCode::CapabilityRefused | ErrorCode::JobUnsettled,
                ) => Some((error.clone(), None)),
                Some(ErrorCode::CeilingExceeded) => {
                    Some((error.clone(), Some(self.instance_id.clone())))
                }
                _ => None,
            },
            _ => None,
        };
        {
            let mut keys = self.flights.lock();
            match ended {
                Some((error, only_for)) => {
                    keys.insert(self.key.clone(), Flight::Ended { error, only_for });
                }
                None => {
                    keys.remove(&self.key);
                }
            }
        }
        if let Some(sender) = self.sender.take() {
            sender.send_replace(Some(Arc::new(outcome.clone())));
        }
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        if self.sender.is_none() {
            return;
        }
        // Gone without landing: the key is free again, and its followers lead in turn (their
        // wait ends when the sender is dropped right after this).
        let mut keys = self.flights.lock();
        if matches!(keys.get(&self.key), Some(Flight::Flying { leader, .. }) if *leader == self.number)
        {
            keys.remove(&self.key);
        }
    }
}

/// A running call ([`Flights::track`]).
pub struct Tracked {
    flights: Arc<Flights>,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.flights.running.remove();
    }
}

/// The count of running calls.
struct Running {
    count: watch::Sender<usize>,
}

impl Default for Running {
    fn default() -> Running {
        Running {
            count: watch::channel(0).0,
        }
    }
}

impl Running {
    fn add(&self) {
        self.count.send_modify(|running| *running += 1);
    }

    fn remove(&self) {
        self.count
            .send_modify(|running| *running = running.saturating_sub(1));
    }
}
