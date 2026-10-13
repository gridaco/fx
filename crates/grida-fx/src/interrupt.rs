//! Interruptions: Ctrl-C (SIGINT) and SIGTERM, for the whole command.
//!
//! [`install`] starts a thread that listens for both signals for as long as the command runs, so
//! neither ends the command by its default action. That action would leave node hosts running: a
//! host leads a process group of its own, so the terminal's SIGINT never reaches it, and a host
//! busy in user code (a module that loops while it imports) never reads the end of its input.
//!
//! Who acts on an interruption depends on what the command is doing:
//! - while a run is running (the `run` verb, from persisted readiness, [`runner_started`], until
//!   the engine is shut down, [`runner_ended`]) the runner owns acceptance: accepted cancellation
//!   stops work, settles reservations, writes `run_cancelled` and exits 130. Finalization can
//!   win first and preserve the actual terminal outcome. Repeated SIGTERM stays
//!   cooperative. A second explicit SIGINT is emergency force, with cleanup unverified.
//!   There is no elapsed-time escalation while the runner owns cleanup.
//! - at any other time (planning, `at: plan` steps, every other verb) the command ends at once:
//!   the process group of every node host it started is killed
//!   ([`grida_fx_runtime::host::end_every_host_now`]) and it exits 130. Nothing is written: planning
//!   spends nothing, and the store's writes are atomic.
//!
//! Ending at once waits at most two seconds for what TypeScript hosts wrote to reach the
//! command's stderr, however slowly (or not at all) it is read
//! ([`grida_fx_runtime::host::end_every_host_now`]). The forced exit's message is written after
//! that, so it comes after what the hosts wrote, never under the stderr lock a copy of a host's
//! stderr given up may still hold, and with half a second at most for the stream to take it
//! ([`grida_fx_runtime::stderr::write_within`]): an interrupted command always ends.

use crate::verbs::run::INTERRUPTED;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// The command runs a workflow: a runner takes interruptions once initialization has ended.
static RUNNER_EXPECTED: AtomicBool = AtomicBool::new(false);
/// The runner takes interruptions now.
static RUNNER_OWNS: AtomicBool = AtomicBool::new(false);
/// One explicit SIGINT has arrived; a second grants emergency force authority.
static SIGINT_SEEN: AtomicBool = AtomicBool::new(false);
/// Held by an end at once from its start until it exits the process, and by the command's own
/// return ([`ending`]): a command that finishes while it is forced to end waits for that end,
/// so the forced exit's message is never cut short by the command's own exit.
static ENDING: Mutex<()> = Mutex::new(());

/// Starts listening (module doc). Returns once the listeners are in place; a platform that has
/// no listener for these signals keeps their default action.
pub fn install() {
    let (ready, listening) = std::sync::mpsc::sync_channel(1);
    let started = std::thread::Builder::new()
        .name("grida-fx-signals".into())
        .spawn(move || listen(ready));
    if started.is_ok() {
        let _ = listening.recv();
    }
}

/// The command runs a workflow (the `run` verb): a runner will own initialized execution.
pub fn expect_runner() {
    RUNNER_EXPECTED.store(true, Ordering::SeqCst);
}

/// `run_started` and local control readiness are established: the runner now owns cleanup.
pub fn runner_started() {
    if RUNNER_EXPECTED.load(Ordering::SeqCst) {
        RUNNER_OWNS.store(true, Ordering::SeqCst);
    }
}

/// The runner has completed local cleanup. Later interruptions are process-owned; the
/// recorded runner outcome determines the exit status of an earlier accepted cancellation.
pub fn runner_ended() {
    RUNNER_OWNS.store(false, Ordering::SeqCst);
}

/// How long the forced exit's message may wait for the command's stderr to take it.
const LAST_WORD_WAIT: Duration = Duration::from_millis(500);

/// Kills every node host's group, waits two seconds at most for their stderr copies, writes
/// `last_word` when there is one ([`LAST_WORD_WAIT`] at most), then exits 130. What the command
/// printed is flushed by the exit, which never waits for a verb that holds its output.
fn end_now(last_word: Option<&str>) -> ! {
    let _ending = ENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    grida_fx_runtime::host::end_every_host_now();
    if let Some(text) = last_word {
        grida_fx_runtime::stderr::write_within(text, LAST_WORD_WAIT);
    }
    std::process::exit(i32::from(INTERRUPTED))
}

/// The command returns: once an end at once under way has exited the process, or at once when
/// none is. Hold the guard until the command has returned.
pub fn ending() -> MutexGuard<'static, ()> {
    ENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One interruption (module doc). `terminate`: it was a SIGTERM.
fn interrupted(terminate: bool) {
    if !RUNNER_OWNS.load(Ordering::SeqCst) {
        end_now(None);
    }
    if !terminate && SIGINT_SEEN.swap(true, Ordering::SeqCst) {
        end_now(Some(
            "interrupted again: forcing exit; run completion and cleanup are unverified\n",
        ));
    }
}

/// The listening thread: a runtime of its own, so the signals are heard whatever the engine's
/// runtime is doing.
fn listen(ready: std::sync::mpsc::SyncSender<()>) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        let _ = ready.send(());
        return;
    };
    runtime.block_on(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut interrupt), Ok(mut terminate)) = (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
            ) else {
                let _ = ready.send(());
                return;
            };
            let _ = ready.send(());
            loop {
                let terminated = tokio::select! {
                    Some(()) = interrupt.recv() => false,
                    Some(()) = terminate.recv() => true,
                    else => return,
                };
                interrupted(terminated);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ready.send(());
            while tokio::signal::ctrl_c().await.is_ok() {
                interrupted(false);
            }
        }
    });
}
