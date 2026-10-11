//! Starting Codex, and ending everything it started (spec/providers.md §9.6).
//!
//! Each Codex runs in a process group of its own, registered here until it is ended. Its MCP
//! servers and helpers join that group, so ending the group ends them too:
//! - a conversation that ends is asked to stop first (stdin closed, then `SIGTERM` to the group),
//!   and the group is then killed ([`Group::finish`]);
//! - a conversation that is dropped (its run was cancelled) kills the group ([`Group`]'s `Drop`);
//! - a command that exits on an interrupt kills every group and lets none start after
//!   ([`end_every_codex`]); a run's cleanup kills any that is left ([`kill_every_codex`]).
//!
//! Process groups are a Unix notion: elsewhere `kill_on_drop` ends Codex itself, and nothing else.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::process::Child;

/// How long a Codex that was asked to stop has before its group is killed.
const GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Groups {
    live: Vec<u32>,
    /// The command is ending: no Codex may start.
    closed: bool,
}

static GROUPS: Mutex<Groups> = Mutex::new(Groups {
    live: Vec::new(),
    closed: false,
});

fn groups() -> MutexGuard<'static, Groups> {
    GROUPS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A started Codex's process group, killed when dropped.
pub(super) struct Group(Option<u32>);

impl Group {
    /// Registers the group `child` leads (`process_group(0)`: its id is the child's pid). `None`
    /// when the command is ending, and the child is then killed.
    pub(super) fn enlist(child: &mut Child) -> Option<Group> {
        let id = if cfg!(unix) { child.id() } else { None };
        let mut groups = groups();
        if groups.closed {
            drop(groups);
            let _ = child.start_kill();
            return None;
        }
        groups.live.extend(id);
        Some(Group(id))
    }

    /// Ends a conversation whose stdin is closed: Codex gets [`GRACE`] to exit, then the group gets
    /// `SIGTERM` and [`GRACE`] more, then the group is killed (also when Codex exited by itself, for
    /// whatever it left in the group).
    pub(super) async fn finish(mut self, child: &mut Child) {
        if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
            if let Some(id) = self.0 {
                signal(id, Signal::Term);
            }
            if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        self.end();
    }

    fn end(&mut self) {
        if let Some(id) = self.0.take() {
            groups().live.retain(|&live| live != id);
            signal(id, Signal::Kill);
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.end();
    }
}

/// Kills every Codex group this process started and has not ended, and lets none start after it:
/// for a command that is about to exit because it was interrupted. Whether every group was
/// verifiably ended.
pub fn end_every_codex() -> bool {
    let live = {
        let mut groups = groups();
        groups.closed = true;
        std::mem::take(&mut groups.live)
    };
    live.into_iter()
        .fold(true, |verified, id| verified & signal(id, Signal::Kill))
}

/// Kills every Codex group this process started and has not ended: for a run's cleanup, after its
/// calls ended. Whether every group was verifiably ended.
pub fn kill_every_codex() -> bool {
    let live = std::mem::take(&mut groups().live);
    live.into_iter()
        .fold(true, |verified, id| verified & signal(id, Signal::Kill))
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// Sends `signal` to the group `id`. Delivered, or the group is already gone (`ESRCH`): `true`.
#[cfg(unix)]
fn signal(id: u32, signal: Signal) -> bool {
    use rustix::io::Errno;
    use rustix::process::{Pid, Signal as Sig, kill_process_group};
    // 0 names the caller's own group and 1 would mean every process: never an owned group.
    let Some(pid) = i32::try_from(id)
        .ok()
        .filter(|&raw| raw > 1)
        .and_then(Pid::from_raw)
    else {
        return false;
    };
    let sig = match signal {
        Signal::Term => Sig::TERM,
        Signal::Kill => Sig::KILL,
    };
    matches!(kill_process_group(pid, sig), Ok(()) | Err(Errno::SRCH))
}

#[cfg(not(unix))]
fn signal(_id: u32, _signal: Signal) -> bool {
    true
}
