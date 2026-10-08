//! One invocation's serialized cancellation, execution admission and terminal commitment.

use crate::engine::Cancel;
use crate::events::{Event, EventLog};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// The bounded, portable source of cancellation intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelSource {
    Cli,
    Signal,
    Sdk,
}

impl CancelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Signal => "signal",
            Self::Sdk => "sdk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlPhase {
    Running,
    CancelRequested,
    RecordError,
    Finishing,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelAcceptance {
    Accepted,
    AlreadyRequested,
    Finishing,
    AlreadyTerminal,
    RecordError,
}

struct State {
    phase: ControlPhase,
    admission_open: bool,
    active: usize,
    accepted: bool,
    terminal: Option<Value>,
}

/// The socket, signal listener and scheduler share this synchronization boundary.
pub struct RunControl {
    state: Mutex<State>,
    admission_changed: tokio::sync::Notify,
    events: Option<Arc<EventLog>>,
    cancel: Cancel,
}

impl RunControl {
    pub fn new(events: Arc<EventLog>, cancel: Cancel) -> Arc<Self> {
        Self::with_events(Some(events), cancel)
    }

    /// Admission for planning or injected test services without a run record.
    /// It cannot acknowledge cancellation because there is no durable event log.
    pub fn unrecorded(cancel: Cancel) -> Arc<Self> {
        Self::with_events(None, cancel)
    }

    fn with_events(events: Option<Arc<EventLog>>, cancel: Cancel) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                phase: ControlPhase::Running,
                admission_open: true,
                active: 0,
                accepted: false,
                terminal: None,
            }),
            events,
            cancel,
            admission_changed: tokio::sync::Notify::new(),
        })
    }

    /// Close admission and flush the acceptance event before returning an acknowledgment.
    /// Persistence failure still closes admission and cancels work, but is never acceptance.
    pub fn request_cancel(&self, source: CancelSource) -> CancelAcceptance {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match state.phase {
            ControlPhase::Terminal => return CancelAcceptance::AlreadyTerminal,
            ControlPhase::Finishing => return CancelAcceptance::Finishing,
            ControlPhase::CancelRequested => return CancelAcceptance::AlreadyRequested,
            ControlPhase::RecordError => return CancelAcceptance::RecordError,
            ControlPhase::Running => {}
        }
        state.admission_open = false;
        let persisted = self
            .events
            .as_ref()
            .is_some_and(|events| events.emit(&Event::CancelRequested { source }).is_ok());
        state.accepted = persisted;
        state.phase = if persisted {
            ControlPhase::CancelRequested
        } else {
            ControlPhase::RecordError
        };
        self.cancel.cancel();
        if persisted {
            CancelAcceptance::Accepted
        } else {
            CancelAcceptance::RecordError
        }
    }

    /// Reserve one dispatch, authored attempt or provider operation before it starts.
    /// Work holding a permit may proceed after acceptance; later work cannot obtain one.
    pub fn admit(self: &Arc<Self>) -> Option<Admission> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.admission_open
            || state.phase != ControlPhase::Running
            || self.cancel.is_cancelled()
        {
            return None;
        }
        state.active += 1;
        Some(Admission {
            control: Arc::clone(self),
        })
    }

    /// Internal failure/draining closes admission without claiming cancellation acceptance.
    pub fn close_admission(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admission_open = false;
    }

    /// Close admission, then wait for every admitted local operation, including incoming
    /// node-host RPC tasks that may outlive their top-level run response.
    pub async fn drain_admissions(&self) {
        self.close_admission();
        loop {
            let changed = self.admission_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().unwrap_or_else(|e| e.into_inner()).active == 0 {
                return;
            }
            changed.await;
        }
    }

    /// Only a runner which drained authored work and paid bookkeeping may begin finishing.
    /// `false` means cancellation won; an error means work is still active.
    pub fn begin_finalization(&self) -> Result<bool, &'static str> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.admission_open = false;
        if state.active != 0 {
            return Err("the run cannot finish while admitted work is active");
        }
        match state.phase {
            ControlPhase::Running => {
                state.phase = ControlPhase::Finishing;
                Ok(true)
            }
            ControlPhase::Finishing => Ok(true),
            ControlPhase::CancelRequested | ControlPhase::RecordError => Ok(false),
            ControlPhase::Terminal => Err("the run already committed its terminal event"),
        }
    }

    /// Commit at most one terminal event and retain its exact flushed record for the receipt.
    pub fn emit_terminal(&self, event: &Event) -> std::io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.terminal.is_some() {
            return Err(std::io::Error::other(
                "the run already committed its terminal event",
            ));
        }
        if state.active != 0 || state.admission_open {
            return Err(std::io::Error::other(
                "the run cannot commit a terminal event while execution is admitted",
            ));
        }
        if !matches!(
            event,
            Event::RunFinished { .. } | Event::RunCancelled { .. }
        ) {
            return Err(std::io::Error::other(
                "a terminal record must end the invocation",
            ));
        }
        let events = self
            .events
            .as_ref()
            .ok_or_else(|| std::io::Error::other("planning has no run terminal record"))?;
        let terminal = events.emit_record(event)?;
        state.terminal = Some(terminal);
        state.phase = ControlPhase::Terminal;
        state.admission_open = false;
        Ok(())
    }

    pub fn phase(&self) -> ControlPhase {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).phase
    }

    pub fn cancellation_requested(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accepted
    }

    pub fn terminal(&self) -> Option<Value> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .terminal
            .clone()
    }
}

/// Tracks admitted work until its local execution/bookkeeping ends.
pub struct Admission {
    control: Arc<RunControl>,
}

impl Drop for Admission {
    fn drop(&mut self) {
        let mut state = self.control.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active -= 1;
        self.control.admission_changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::read_events;

    fn control(path: &std::path::Path) -> Arc<RunControl> {
        RunControl::new(
            Arc::new(EventLog::open(path, "owner-a", &"0".repeat(64)).unwrap()),
            Cancel::new(),
        )
    }

    #[test]
    fn acceptance_is_durable_idempotent_and_closes_admission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let control = control(&path);
        let admitted = control.admit().unwrap();
        assert_eq!(
            control.request_cancel(CancelSource::Cli),
            CancelAcceptance::Accepted
        );
        assert_eq!(
            control.request_cancel(CancelSource::Signal),
            CancelAcceptance::AlreadyRequested
        );
        assert!(control.admit().is_none());
        assert!(control.begin_finalization().is_err());
        drop(admitted);
        assert_eq!(control.begin_finalization(), Ok(false));
        let events = read_events(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "cancel_requested");
        assert_eq!(events[0]["source"], "cli");
    }

    #[test]
    fn finishing_wins_only_after_admitted_work_ends() {
        let dir = tempfile::tempdir().unwrap();
        let control = control(&dir.path().join("events.jsonl"));
        let admitted = control.admit().unwrap();
        assert!(control.begin_finalization().is_err());
        drop(admitted);
        assert_eq!(control.begin_finalization(), Ok(true));
        assert_eq!(
            control.request_cancel(CancelSource::Cli),
            CancelAcceptance::Finishing
        );
        assert!(!control.cancellation_requested());
    }

    #[test]
    fn concurrent_requests_append_exactly_one_acceptance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let control = control(&path);
        let barrier = Arc::new(std::sync::Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let control = Arc::clone(&control);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let accepted = control.request_cancel(CancelSource::Cli);
                assert!(control.admit().is_none());
                accepted
            }));
        }
        barrier.wait();
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == CancelAcceptance::Accepted)
                .count(),
            1
        );
        assert!(outcomes.iter().all(|outcome| matches!(
            outcome,
            CancelAcceptance::Accepted | CancelAcceptance::AlreadyRequested
        )));
        assert_eq!(read_events(&path).unwrap().len(), 1);
    }

    #[test]
    fn finalization_and_cancellation_have_one_winner() {
        for _ in 0..32 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("events.jsonl");
            let control = control(&path);
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let requested = {
                let control = Arc::clone(&control);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    control.request_cancel(CancelSource::Cli)
                })
            };
            barrier.wait();
            let finishing = control.begin_finalization().unwrap();
            let accepted = requested.join().unwrap();
            assert_eq!(finishing, accepted == CancelAcceptance::Finishing);
            assert_eq!(read_events(&path).unwrap().len(), usize::from(!finishing));
            assert!(control.admit().is_none());
        }
    }

    #[test]
    fn terminal_is_exactly_the_flushed_selected_invocation_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let control = control(&path);
        let terminal = Event::RunCancelled {
            reason: "interrupted".into(),
            charged_usd: grida_fx_core::money::Usd::ZERO,
        };
        assert!(control.emit_terminal(&terminal).is_err());
        assert_eq!(
            control.request_cancel(CancelSource::Cli),
            CancelAcceptance::Accepted
        );
        control.emit_terminal(&terminal).unwrap();
        assert!(control.emit_terminal(&terminal).is_err());
        assert_eq!(
            control.request_cancel(CancelSource::Cli),
            CancelAcceptance::AlreadyTerminal
        );
        let recorded = read_events(&path).unwrap();
        assert_eq!(control.terminal(), recorded.last().cloned());
        assert_eq!(control.terminal().unwrap()["invocation_id"], "owner-a");
    }

    #[tokio::test]
    async fn cancellation_drains_an_rpc_permit_that_outlives_node_and_paid_work() {
        let dir = tempfile::tempdir().unwrap();
        let control = control(&dir.path().join("events.jsonl"));
        let incoming_rpc = control.admit().unwrap();
        assert_eq!(
            control.request_cancel(CancelSource::Cli),
            CancelAcceptance::Accepted
        );
        let draining = {
            let control = Arc::clone(&control);
            tokio::spawn(async move { control.drain_admissions().await })
        };
        tokio::task::yield_now().await;
        assert!(!draining.is_finished());
        assert!(control.admit().is_none());
        drop(incoming_rpc);
        draining.await.unwrap();
        assert_eq!(control.begin_finalization(), Ok(false));
    }
}
