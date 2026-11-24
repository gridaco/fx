//! Private launch handoff between the project service and an independent runner process.

use grida_fx_core::Error;
use grida_fx_core::providers::Declarations;
use grida_fx_viewer::service::{LaunchAccepted, LaunchRefused, RunLauncher};
use std::{path::Path, sync::Arc};

pub(crate) const CHILD_VERB: &str = "__viewer-run";

type Outcome = Result<LaunchAccepted, LaunchRefused>;

fn refusal(code: &str, message: &str, unknown: bool) -> LaunchRefused {
    LaunchRefused {
        code: code.into(),
        message: message.into(),
        unknown,
    }
}

#[cfg(unix)]
pub(super) struct Receipt(std::cell::RefCell<Option<std::os::unix::net::UnixStream>>);

#[cfg(not(unix))]
pub(super) struct Receipt;

impl Receipt {
    fn publish(&self, outcome: Outcome) {
        #[cfg(unix)]
        if let Some(stream) = self.0.borrow_mut().take() {
            // The service can stop listening while the engine is starting. Delivery never
            // interrupts a committed invocation or prevents it from dispatching its steps.
            let _ = process::write_message(&stream, &serde_json::json!(outcome));
        }
        #[cfg(not(unix))]
        let _ = outcome;
    }

    pub(super) fn refuse(&self, code: &str, message: &str) -> Error {
        self.publish(Err(refusal(code, message, false)));
        Error::usage(message)
    }

    pub(super) fn accept(&self, folder: &Path, invocation: &str) {
        if let Ok(folder) = folder.canonicalize() {
            self.publish(Ok(LaunchAccepted {
                folder,
                invocation_id: invocation.into(),
            }));
        } else {
            self.publish(Err(start_unknown()));
        }
    }

    /// The deduplication journal found a run that already committed its initial invocation.
    pub(super) fn accept_existing(&self, folder: &Path) -> Result<(), Error> {
        let events = super::read_log(folder, "accepted attempt", true)?;
        let invocation = events
            .first()
            .filter(|event| event["event"] == "run_started")
            .and_then(|event| event["invocation_id"].as_str())
            .filter(|invocation| !invocation.is_empty())
            .ok_or_else(|| Error::usage("the accepted attempt's start is unavailable"))?;
        self.accept(folder, invocation);
        Ok(())
    }
}

pub(super) fn launcher(declarations: &Declarations) -> RunLauncher {
    #[cfg(unix)]
    {
        use grida_fx_core::docs::project::Project;

        let startup_variables = declarations.variables();
        Arc::new(move |request| {
            let unavailable_project = || {
                refusal(
                    "context_unavailable",
                    "FX couldn’t find the project that started this run.",
                    false,
                )
            };
            let root = &request.project_root;
            if !root.is_dir() || root.canonicalize().ok().as_ref() != Some(root) {
                return Err(unavailable_project());
            }
            let project = Project::find(root).map_err(|_| unavailable_project())?;
            if &project.root != root {
                return Err(unavailable_project());
            }
            // A long-lived service may predate newly configured provider variables.
            let mut withheld = startup_variables.clone();
            withheld.extend(project.declarations().variables());
            process::launch(request, &withheld)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = declarations;
        Arc::new(|_| Err(unavailable()))
    }
}

pub(crate) fn child() -> Result<u8, Error> {
    #[cfg(unix)]
    {
        process::child()
    }
    #[cfg(not(unix))]
    {
        Err(Error::unavailable(unavailable().message))
    }
}

fn unavailable() -> LaunchRefused {
    refusal(
        "launch_unavailable",
        "FX couldn’t start the run from the viewer. Try in the terminal.",
        false,
    )
}

fn start_unknown() -> LaunchRefused {
    refusal(
        "start_unknown",
        "FX couldn’t tell if the attempt started. Check the run’s status before retrying.",
        true,
    )
}

#[cfg(unix)]
mod process {
    use super::{CHILD_VERB, Outcome, Receipt, refusal, start_unknown, unavailable};
    use grida_fx_core::{Error, value::sha256_hex};
    use grida_fx_runtime::{attempts, run_index};
    use grida_fx_viewer::service::{LaunchAccepted, LaunchOperation, LaunchRequest};
    use serde_json::Value;
    use std::{
        cell::RefCell,
        io::{self, BufRead, BufReader, Read, Write},
        net::Shutdown,
        os::{
            fd::OwnedFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };

    const MAX_MESSAGE: u64 = 32 * 1024;
    const TRANSPORT: Duration = Duration::from_secs(5);
    const START_DEADLINE: Duration = Duration::from_secs(30);

    pub(super) fn write_message(mut stream: &UnixStream, value: &Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        if bytes.len() as u64 >= MAX_MESSAGE {
            return Err(io::Error::other("viewer launch message is too large"));
        }
        bytes.push(b'\n');
        stream.write_all(&bytes)
    }

    fn read_message(stream: &UnixStream) -> io::Result<Value> {
        let mut bytes = Vec::new();
        BufReader::new(stream.take(MAX_MESSAGE)).read_until(b'\n', &mut bytes)?;
        if bytes.last() != Some(&b'\n') {
            return Err(io::Error::other("viewer launch message is incomplete"));
        }
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }

    pub(super) fn launch(request: LaunchRequest, remove: &[String]) -> Outcome {
        let executable = std::env::current_exe().map_err(|_| unavailable())?;
        let (parent, child) = UnixStream::pair().map_err(|_| unavailable())?;
        parent
            .set_write_timeout(Some(TRANSPORT))
            .map_err(|_| unavailable())?;
        parent
            .set_read_timeout(Some(START_DEADLINE))
            .map_err(|_| unavailable())?;
        let value = serde_json::to_value(&request).map_err(|_| unavailable())?;
        let mut command = Command::new(executable);
        command
            .arg(CHILD_VERB)
            .current_dir(&request.project_root)
            .stdin(Stdio::from(OwnedFd::from(child)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("GRIDA_FX_DISABLE_DOTENV", "1")
            .env("GRIDA_FX_NETWORK", "off")
            .env_remove("GRIDA_FX_CANCEL_SOURCE")
            .process_group(0);
        for name in remove {
            command.env_remove(name);
        }
        let (spawned, started) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("fx-viewer-runner".into())
            .spawn(move || {
                let launched = command.spawn();
                drop(command);
                let _ = spawned.send(launched.is_ok());
                if let Ok(mut child) = launched {
                    let _ = child.wait();
                }
            })
            .map_err(|_| unavailable())?;
        // Send the whole request before treating later channel failures as an unknown start.
        if started.recv_timeout(TRANSPORT) != Ok(true) {
            return Err(unavailable());
        }
        write_message(&parent, &value).map_err(|_| unavailable())?;
        let _ = parent.shutdown(Shutdown::Write);
        let value = read_message(&parent).map_err(|_| start_unknown())?;
        let accepted = serde_json::from_value::<Outcome>(value).map_err(|_| start_unknown())??;
        if verified_acceptance(&request, &accepted) {
            Ok(accepted)
        } else {
            Err(start_unknown())
        }
    }

    fn verified_acceptance(request: &LaunchRequest, accepted: &LaunchAccepted) -> bool {
        let folder = &accepted.folder;
        if folder.canonicalize().ok().as_ref() != Some(folder)
            || run_index::recording_project(folder, &request.project_root).as_ref()
                != Some(&request.project_root)
        {
            return false;
        }
        let Ok(Some(attempt)) = attempts::read(folder) else {
            return false;
        };
        if attempt.run_id != request.run_id {
            return false;
        }
        let Ok(events) = super::super::read_log(folder, "accepted attempt", true) else {
            return false;
        };
        let mut invocations = events
            .iter()
            .filter(|event| event["event"] == "run_started")
            .filter_map(|event| event["invocation_id"].as_str());
        match request.operation {
            LaunchOperation::Resume => {
                folder == &request.folder
                    && attempt.attempt_number == request.source_attempt
                    && invocations
                        .skip_while(|id| *id != request.expected_invocation)
                        .nth(1)
                        == Some(&accepted.invocation_id)
            }
            LaunchOperation::Rerun => {
                let key = sha256_hex(request.request_id.as_bytes());
                attempt.attempt_number > request.source_attempt
                    && invocations.next() == Some(&accepted.invocation_id)
                    && attempts::allocations(&request.project_root, &request.run_id).is_ok_and(
                        |allocations| {
                            allocations.iter().any(|allocation| {
                                allocation.attempt_number == attempt.attempt_number
                                    && allocation.source_attempt == request.source_attempt
                                    && allocation.request_digest.as_ref() == Some(&key)
                                    && allocation
                                        .existing_folder(&request.project_root)
                                        .ok()
                                        .flatten()
                                        .as_ref()
                                        == Some(folder)
                            })
                        },
                    )
            }
        }
    }

    pub(super) fn child() -> Result<u8, Error> {
        let fd = rustix::io::fcntl_dupfd_cloexec(std::io::stdin(), 3)
            .map_err(|_| Error::unavailable("The private viewer launch socket is unavailable."))?;
        let stream = UnixStream::from(fd);
        // The runner uses this descriptor privately. Author hosts must not inherit fd 0's
        // copy of the socket, even if a subprocess does not replace standard input.
        rustix::io::fcntl_setfd(std::io::stdin(), rustix::io::FdFlags::CLOEXEC)
            .map_err(|_| Error::unavailable("The private viewer launch socket is unavailable."))?;
        stream
            .set_read_timeout(Some(TRANSPORT))
            .map_err(|error| Error::io("viewer launch", &error))?;
        stream
            .set_write_timeout(Some(TRANSPORT))
            .map_err(|error| Error::io("viewer launch", &error))?;
        let value = read_message(&stream).map_err(|error| Error::io("viewer launch", &error))?;
        let request = serde_json::from_value(value)
            .map_err(|_| Error::usage("The private viewer launch request is invalid."))?;
        let receipt = Receipt(RefCell::new(Some(stream)));
        let outcome = super::super::resume::viewer_start(&request, &receipt);
        // Pre-start refusal without a safe categorized explanation, or an engine error.
        // Builder output and private paths in CLI diagnostics do not go to the viewer.
        receipt.publish(Err(refusal(
            "launch_refused",
            "FX couldn’t start this attempt with the information it saved. Use the terminal for details.",
            false,
        )));
        outcome
    }
}
