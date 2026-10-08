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
//!   ([`grida_fx_runtime::host::end_every_host`]) and it exits 130. Nothing is written: planning
//!   spends nothing, and the store's writes are atomic.

use crate::verbs::run::INTERRUPTED;
use std::sync::atomic::{AtomicBool, Ordering};

/// The command runs a workflow: a runner takes interruptions once initialization has ended.
static RUNNER_EXPECTED: AtomicBool = AtomicBool::new(false);
/// The runner takes interruptions now.
static RUNNER_OWNS: AtomicBool = AtomicBool::new(false);
/// One explicit SIGINT has arrived; a second grants emergency force authority.
static SIGINT_SEEN: AtomicBool = AtomicBool::new(false);

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

/// Kills every node host's group, then exits 130. What the command printed is flushed by the
/// exit, which never waits for a verb that holds its output.
fn end_now() -> ! {
    grida_fx_runtime::host::end_every_host();
    std::process::exit(i32::from(INTERRUPTED))
}

/// One interruption (module doc). `terminate`: it was a SIGTERM.
fn interrupted(terminate: bool) {
    if !RUNNER_OWNS.load(Ordering::SeqCst) {
        end_now();
    }
    if !terminate && SIGINT_SEEN.swap(true, Ordering::SeqCst) {
        eprintln!("interrupted again: forcing exit; run completion and cleanup are unverified");
        end_now();
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
