//! Interruptions: Ctrl-C (SIGINT) and SIGTERM, for the whole command.
//!
//! [`install`] starts a thread that listens for both signals for as long as the command runs, so
//! neither ends the command by its default action. That action would leave node hosts running: a
//! host leads a process group of its own, so the terminal's SIGINT never reaches it, and a host
//! busy in user code (a module that loops while it imports) never reads the end of its input.
//!
//! Who acts on an interruption depends on what the command is doing:
//! - while a run is running (from persisted readiness, [`runner_started`], until
//!   [`runner_ended`]) the runner owns acceptance: accepted cancellation
//!   stops work, settles reservations, writes `run_cancelled` and exits 130. Finalization can
//!   win first and preserve the actual terminal outcome. Repeated SIGTERM stays
//!   cooperative. A second explicit SIGINT is emergency force, with cleanup unverified.
//!   There is no elapsed-time escalation while the runner owns cleanup.
//! - while `add` holds a staged template, an interruption accepted before publication stops
//!   setup after dropping staging. A second explicit SIGINT can force exit before cleanup
//!   finishes. Repeated SIGTERM stays cooperative. Once publication starts, FX finishes the
//!   rename and staging cleanup before resuming immediate-exit handling.
//! - after an inline call has recorded its run, output delivery returns to immediate-exit
//!   handling. FX attempts to unlink the staged output without waiting for its writer and
//!   bounds the time it waits for the destination filesystem. The run and any published output
//!   are outside that cleanup.
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
use grida_fx_core::Error;
use grida_fx_runtime::folder::same_file;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// The command runs a workflow: a runner takes interruptions once initialization has ended.
static RUNNER_EXPECTED: AtomicBool = AtomicBool::new(false);
/// The runner takes interruptions now.
static RUNNER_OWNS: AtomicBool = AtomicBool::new(false);
/// One explicit SIGINT has arrived; a second grants emergency force authority.
static SIGINT_SEEN: AtomicBool = AtomicBool::new(false);
const IDLE: u8 = 0;
const EXITING: u8 = 1;
const STAGING: u8 = 2;
const CANCELLED: u8 = 3;
const CANCELLED_BY_SIGINT: u8 = 4;
const PUBLISHING: u8 = 5;
static INSTALL: AtomicU8 = AtomicU8::new(IDLE);
/// The temporary file of a call delivering an output, never its publication path.
static DELIVERY_STAGING: Mutex<Option<StagedDelivery>> = Mutex::new(None);
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

/// Creates and registers an output staging file before its writer can block.
pub fn with_delivery_staging<T>(
    parent: &Path,
    deliver: impl FnOnce(tempfile::NamedTempFile) -> io::Result<T>,
) -> io::Result<T> {
    let staged = {
        let mut active = delivery_staging();
        if INSTALL.load(Ordering::SeqCst) == EXITING {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let staged = tempfile::NamedTempFile::new_in(parent)?;
        *active = Some(StagedDelivery {
            file: staged.as_file().try_clone()?,
            path: staged.path().to_path_buf(),
        });
        staged
    };
    let registration = DeliveryRegistration;
    let result = deliver(staged);
    drop(registration);
    result
}

struct DeliveryRegistration;

struct StagedDelivery {
    file: File,
    path: PathBuf,
}

impl Drop for DeliveryRegistration {
    fn drop(&mut self) {
        *delivery_staging() = None;
    }
}

fn delivery_staging() -> MutexGuard<'static, Option<StagedDelivery>> {
    DELIVERY_STAGING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn discard_delivery_staging() {
    if DELIVERY_STAGING
        .try_lock()
        .is_ok_and(|active| active.is_none())
    {
        return;
    }
    let (done, discarded) = std::sync::mpsc::sync_channel(1);
    if std::thread::Builder::new()
        .name("grida-fx-delivery-cleanup".into())
        .spawn(move || {
            let staged = delivery_staging().take();
            if let Some(staged) = staged
                && same_file(&staged.file, &staged.path)
            {
                let _ = std::fs::remove_file(staged.path);
            }
            let _ = done.send(());
        })
        .is_ok()
    {
        // The lock may be held by a blocked file creation, and unlink can block too.
        let _ = discarded.recv_timeout(Duration::from_millis(500));
    }
}

/// Gives the installer the first interruption while it owns a temporary directory. Unless a
/// second SIGINT forces exit, the closure drops the directory before its caller returns.
pub fn with_template_staging<T>(
    stage: impl FnOnce() -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    if INSTALL
        .compare_exchange(IDLE, STAGING, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(None);
    }
    let outcome = stage();
    if matches!(
        INSTALL.swap(IDLE, Ordering::SeqCst),
        CANCELLED | CANCELLED_BY_SIGINT | EXITING
    ) {
        Ok(None)
    } else {
        outcome.map(Some)
    }
}

/// Checks for cancellation between entries while materializing a template.
pub fn check_template_staging() -> Result<(), Error> {
    if matches!(
        INSTALL.load(Ordering::SeqCst),
        CANCELLED | CANCELLED_BY_SIGINT | EXITING
    ) {
        return Err(Error::usage("template installation interrupted"));
    }
    Ok(())
}

/// Claims publication. A signal that already cancelled staging wins; later signals leave the
/// atomic rename and staging cleanup to the installer.
pub fn begin_template_publication() -> Result<(), Error> {
    if INSTALL
        .compare_exchange(STAGING, PUBLISHING, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(Error::usage("template installation interrupted"));
    }
    Ok(())
}

/// How long the forced exit's message may wait for the command's stderr to take it.
const LAST_WORD_WAIT: Duration = Duration::from_millis(500);

/// Kills every node host's group, waits two seconds at most for their stderr copies, writes
/// `last_word` when there is one ([`LAST_WORD_WAIT`] at most), then exits 130. What the command
/// printed is flushed by the exit, which never waits for a verb that holds its output.
fn end_now(_ending: MutexGuard<'static, ()>, last_word: Option<&str>) -> ! {
    INSTALL.store(EXITING, Ordering::SeqCst);
    discard_delivery_staging();
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

#[derive(Debug, PartialEq, Eq)]
enum SetupInterrupt {
    Continue,
    Exit,
    ForceStaging,
}

/// Settles an interruption against the installer and any concurrent publication claim.
fn interrupt_setup(install: &AtomicU8, terminate: bool) -> SetupInterrupt {
    use SetupInterrupt::{Continue, Exit, ForceStaging};

    loop {
        let state = install.load(Ordering::SeqCst);
        let (next, action) = match (state, terminate) {
            (IDLE, _) => (EXITING, Exit),
            (STAGING, true) => (CANCELLED, Continue),
            (STAGING | CANCELLED, false) => (CANCELLED_BY_SIGINT, Continue),
            (CANCELLED_BY_SIGINT, false) => (EXITING, ForceStaging),
            _ => return Continue,
        };
        if install
            .compare_exchange(state, next, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return action;
        }
    }
}

/// One interruption (module doc). `terminate`: it was a SIGTERM.
fn interrupted(terminate: bool) {
    if !RUNNER_OWNS.load(Ordering::SeqCst) {
        // Hold this before accepting a forced transition. The command may finish staging and
        // reach its normal return immediately after the transition.
        let ending = ending();
        match interrupt_setup(&INSTALL, terminate) {
            SetupInterrupt::Continue => return,
            SetupInterrupt::Exit => end_now(ending, None),
            SetupInterrupt::ForceStaging => end_now(
                ending,
                Some("interrupted again: forcing exit; template staging may remain\n"),
            ),
        }
    }
    if !terminate && SIGINT_SEEN.swap(true, Ordering::SeqCst) {
        end_now(
            ending(),
            Some("interrupted again: forcing exit; run completion and cleanup are unverified\n"),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn delivery_cleanup_unlinks_only_the_staged_file_without_waiting_for_its_writer() {
        use std::io::Write;

        let root = tempfile::tempdir().unwrap();
        let saved = root.path().join("saved.png");
        let output = root.path().join("output.png");
        std::fs::write(&saved, b"saved image").unwrap();

        with_delivery_staging(root.path(), |mut staged| {
            staged.write_all(b"unpublished image")?;
            assert!(staged.path().is_file());
            discard_delivery_staging();
            assert!(!staged.path().exists());
            assert!(!output.exists());
            Ok(())
        })
        .unwrap();

        with_delivery_staging(root.path(), |mut staged| {
            staged.write_all(b"published image")?;
            let path = staged.path().to_path_buf();
            staged.persist_noclobber(&output)?;
            std::fs::write(&path, b"another owner's file")?;
            discard_delivery_staging();
            assert_eq!(std::fs::read(path)?, b"another owner's file");
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(saved).unwrap(), b"saved image");
        assert_eq!(std::fs::read(output).unwrap(), b"published image");
    }

    #[test]
    fn interrupted_template_staging_cleans_up_before_returning() {
        let root = tempfile::tempdir().unwrap();
        let outcome = with_template_staging(|| {
            let staging = tempfile::tempdir_in(root.path()).unwrap();
            std::fs::write(staging.path().join("fx.yaml"), "fx: project/v1\n").unwrap();
            interrupted(false);
            assert!(staging.path().is_dir());
            assert!(check_template_staging().is_err());
            begin_template_publication()
        })
        .unwrap();
        assert!(outcome.is_none());
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    fn a_second_explicit_interrupt_can_force_cancelled_staging() {
        use SetupInterrupt::{Continue, ForceStaging};

        let install = AtomicU8::new(STAGING);
        assert_eq!(interrupt_setup(&install, true), Continue);
        assert_eq!(interrupt_setup(&install, true), Continue);
        assert_eq!(install.load(Ordering::SeqCst), CANCELLED);
        assert_eq!(interrupt_setup(&install, false), Continue);
        assert_eq!(interrupt_setup(&install, true), Continue);
        assert_eq!(install.load(Ordering::SeqCst), CANCELLED_BY_SIGINT);
        assert_eq!(interrupt_setup(&install, false), ForceStaging);
        assert_eq!(install.load(Ordering::SeqCst), EXITING);

        install.store(PUBLISHING, Ordering::SeqCst);
        assert_eq!(interrupt_setup(&install, false), Continue);
        assert_eq!(interrupt_setup(&install, false), Continue);
        assert_eq!(interrupt_setup(&install, true), Continue);
        assert_eq!(install.load(Ordering::SeqCst), PUBLISHING);
    }
}
