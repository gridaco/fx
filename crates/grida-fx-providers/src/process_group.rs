//! Ending everything a runtime the user installed started: Codex and Grok Build
//! (src/codex/README.md; src/grok/README.md).
//!
//! Each runtime runs in a process group of its own, registered here until it is ended. Its MCP
//! servers and helpers join that group, so ending the group ends them too:
//! - a runtime that shuts down asks its program to stop first (stdin closed, then `SIGTERM` to the
//!   group), and then kills the group ([`Group::finish`]);
//! - a runtime that is dropped without shutting down kills the group ([`Group`]'s `Drop`);
//! - a command that exits on an interrupt kills every group and lets none start after
//!   ([`end_every_runtime`]).
//!
//! Process groups are a Unix notion: elsewhere `kill_on_drop` ends the program itself, and nothing
//! else.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::process::Child;

/// How long a program that was asked to stop has before its group is killed.
const GRACE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Groups {
    live: Vec<u32>,
    /// The command is ending: no runtime may start.
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

/// A started runtime's process group, killed when dropped.
pub(crate) struct Group(Option<u32>);

impl Group {
    /// Registers the group `child` leads (`process_group(0)`: its id is the child's pid). `None`
    /// when the command is ending, and the child is then killed.
    pub(crate) fn enlist(child: &mut Child) -> Option<Group> {
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

    /// Ends a program whose stdin is closed: it gets [`GRACE`] to exit, then the group gets
    /// `SIGTERM` and [`GRACE`] more, then the group is killed (also when the program exited by
    /// itself, for whatever it left in the group). Whether the group verifiably ended.
    pub(crate) async fn finish(mut self, child: &mut Child) -> bool {
        if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
            if let Some(id) = self.0 {
                signal(id, Signal::Term);
            }
            if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        self.end()
    }

    fn end(&mut self) -> bool {
        match self.0.take() {
            Some(id) => {
                groups().live.retain(|&live| live != id);
                signal(id, Signal::Kill)
            }
            None => true,
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.end();
    }
}

/// Kills every runtime group this process started and has not ended, and lets none start after
/// it: for a command that is about to exit because it was interrupted. Whether every group was
/// verifiably ended.
pub fn end_every_runtime() -> bool {
    let live = {
        let mut groups = groups();
        groups.closed = true;
        std::mem::take(&mut groups.live)
    };
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
