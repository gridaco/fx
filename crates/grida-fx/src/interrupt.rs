//! Interruptions: Ctrl-C (SIGINT) and SIGTERM, for the whole command.
//!
//! [`install`] starts a thread that listens for both signals for as long as the command runs, so
//! neither ends the command by its default action. That action would leave node hosts running: a
//! host leads a process group of its own, so the terminal's SIGINT never reaches it, and a host
//! busy in user code (a module that loops while it imports) never reads the end of its input.
//!
//! Who acts on an interruption depends on what the command is doing:
//! - while a run is running (the `run` verb, from the end of planning, [`planning_ended`], until
//!   the engine is shut down, [`runner_ended`]) the runner does: it stops the run, settles what it
//!   reserved, writes `run_cancelled`, and the command exits 130. The runner listens for SIGINT,
//!   so a SIGTERM is passed on to the command itself as a SIGINT. A second interruption, or the
//!   command still running [`FALLBACK`] after the first, ends the command as below; so does an
//!   interruption the runner took when the engine is shut down.
//! - at any other time (planning, `at: plan` steps, every other verb) the command ends at once:
//!   the process group of every node host it started is killed
//!   ([`grida_fx_runtime::host::end_every_host`]) and it exits 130. Nothing is written: planning
//!   spends nothing, and the store's writes are atomic.

use crate::verbs::run::INTERRUPTED;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the runner has to stop a run before the command ends anyway: three times the 5
/// seconds a node host has to answer `$/cancel`.
pub const FALLBACK: Duration = Duration::from_secs(15);

/// The command runs a workflow: a runner takes interruptions once planning has ended.
static RUNNER_EXPECTED: AtomicBool = AtomicBool::new(false);
/// The runner takes interruptions now.
static RUNNER_OWNS: AtomicBool = AtomicBool::new(false);
/// An interruption has arrived.
static INTERRUPTION: AtomicBool = AtomicBool::new(false);
/// A SIGINT this command sent itself (for the runner) and has not received yet.
static FORWARDED: AtomicBool = AtomicBool::new(false);

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

/// The command runs a workflow (the `run` verb): a runner takes interruptions once planning has
/// ended.
pub fn expect_runner() {
    RUNNER_EXPECTED.store(true, Ordering::SeqCst);
}

/// Planning has ended: the runner, when the command runs a workflow, takes interruptions from now
/// on.
pub fn planning_ended() {
    if RUNNER_EXPECTED.load(Ordering::SeqCst) {
        RUNNER_OWNS.store(true, Ordering::SeqCst);
    }
}

/// The run has ended and the engine is about to shut down: the command takes interruptions
/// again, and one that arrived meanwhile ends it now.
pub fn runner_ended() {
    RUNNER_OWNS.store(false, Ordering::SeqCst);
    if INTERRUPTION.load(Ordering::SeqCst) {
        end_now();
    }
}

/// Kills every node host's group, then exits 130. What the command printed is flushed by the
/// exit, which never waits for a verb that holds its output.
fn end_now() -> ! {
    grida_fx_runtime::host::end_every_host();
    std::process::exit(i32::from(INTERRUPTED))
}

/// One interruption (module doc). `terminate`: it was a SIGTERM.
fn interrupted(terminate: bool) {
    if !terminate && FORWARDED.swap(false, Ordering::SeqCst) {
        // The SIGINT this command sent itself for the runner.
        return;
    }
    let first = !INTERRUPTION.swap(true, Ordering::SeqCst);
    if !first || !RUNNER_OWNS.load(Ordering::SeqCst) {
        end_now();
    }
    if terminate {
        forward_to_runner();
    }
    let _ = std::thread::Builder::new()
        .name("grida-fx-fallback".into())
        .spawn(|| {
            std::thread::sleep(FALLBACK);
            end_now();
        });
}

/// Sends this process a SIGINT, the signal the runner listens for.
#[cfg(unix)]
fn forward_to_runner() {
    FORWARDED.store(true, Ordering::SeqCst);
    let sent = std::process::Command::new("kill")
        .args(["-s", "INT", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !sent {
        FORWARDED.store(false, Ordering::SeqCst);
        end_now();
    }
}

#[cfg(not(unix))]
fn forward_to_runner() {}

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
