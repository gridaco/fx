//! Private same-user invocation control (spec/control.md). No service or author code is used.

use crate::run_control::{CancelAcceptance, CancelSource, ControlPhase, RunControl};
use grida_fx_core::value::{digest, is_digest, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_PAYLOAD: usize = 16 * 1024;
const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;
const MAX_LOG_BYTES: usize = 64 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONTROL_EVENTS: usize = 65_536;

type EvidenceError = (&'static str, &'static str);

fn bounded_file(path: &Path, limit: usize, missing_ok: bool) -> Result<Vec<u8>, EvidenceError> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if missing_ok && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(_) => return Err(("invalid_target", "the selected run evidence cannot be read")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit as u64 {
        return Err((
            "invalid_target",
            "the selected run evidence is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take((limit + 1) as u64).read_to_end(&mut bytes))
        .map_err(|_| ("invalid_target", "the selected run evidence cannot be read"))?;
    if bytes.len() > limit {
        return Err((
            "invalid_target",
            "the selected run evidence exceeds its read limit",
        ));
    }
    Ok(bytes)
}

fn control_events(folder: &Path, plan: &str) -> Result<Vec<Value>, EvidenceError> {
    let bytes = bounded_file(&folder.join("events.jsonl"), MAX_LOG_BYTES, true)?;
    let mut events = Vec::new();
    // A torn final line is excluded exactly as in observation, without repairing it.
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        if line.len() > MAX_EVENT_BYTES {
            return Err((
                "invalid_target",
                "a run event exceeds the bounded read limit",
            ));
        }
        let event: Value = serde_json::from_slice(line)
            .map_err(|_| ("invalid_target", "the selected run has unreadable events"))?;
        if event.get("kind").and_then(Value::as_str) != Some("fx-run-events-v1") {
            return Err((
                "unsupported_version",
                "the run event version is unsupported",
            ));
        }
        if event.get("plan").and_then(Value::as_str) != Some(plan)
            || !event
                .get("invocation_id")
                .and_then(Value::as_str)
                .is_some_and(|id| hex(id, 16))
            || event.get("offset_ms").and_then(Value::as_u64).is_none()
            || event
                .get("event")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err((
                "invalid_target",
                "the selected run event envelope is invalid",
            ));
        }
        if matches!(
            event.get("event").and_then(Value::as_str),
            Some("run_started" | "cancel_requested" | "run_finished" | "run_cancelled")
        ) {
            if events.len() == MAX_CONTROL_EVENTS {
                return Err((
                    "invalid_target",
                    "the selected run exceeds the bounded invocation history limit",
                ));
            }
            events.push(event);
        }
    }
    Ok(events)
}

/// The public CLI/SDK outcome. Private discovery fields never enter this document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlResult {
    pub kind: String,
    pub operation: String,
    pub invocation_id: Option<String>,
    pub outcome: String,
    pub request_status: String,
    pub recorded_state: String,
    pub cleanup: String,
    pub external_completion: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_cancel: Option<bool>,
}

impl ControlResult {
    pub fn error(operation: &str, code: &str, message: &str) -> Self {
        let mut result = Self::new(operation);
        result.fail(code, message);
        result
    }

    fn new(operation: &str) -> Self {
        Self {
            kind: "fx-run-control-v1".into(),
            operation: operation.into(),
            invocation_id: None,
            outcome: "inspected".into(),
            request_status: "not_accepted".into(),
            recorded_state: "unknown".into(),
            cleanup: "unknown".into(),
            external_completion: "not_verified".into(),
            code: None,
            message: None,
            availability: (operation == "inspect").then(|| "unknown".into()),
            can_cancel: (operation == "inspect").then_some(false),
        }
    }

    fn fail(&mut self, code: &str, message: &str) {
        self.outcome = "error".into();
        self.code = Some(code.into());
        self.message = Some(message.into());
    }

    pub fn exit_status(&self) -> u8 {
        if self.outcome != "error" {
            return if self.code.is_none()
                && matches!(
                    (self.operation.as_str(), self.outcome.as_str()),
                    ("inspect", "inspected")
                        | (
                            "cancel",
                            "accepted"
                                | "already_requested"
                                | "already_terminal"
                                | "finishing"
                                | "completed"
                        )
                ) {
                0
            } else {
                2
            };
        }
        match self.code.as_deref() {
            Some(
                "unavailable"
                | "wait_timeout"
                | "owner_lost"
                | "acknowledgment_unknown"
                | "completion_unverified",
            ) => 1,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone)]
struct Record {
    folder: PathBuf,
    folder_digest: String,
    plan: String,
    invocation: Option<String>,
    state: String,
    accepted: bool,
    terminal: Option<Value>,
}

impl Record {
    fn read(folder: &Path) -> Result<Self, EvidenceError> {
        let folder = folder
            .canonicalize()
            .map_err(|_| ("invalid_target", "the selected run folder cannot be read"))?;
        let plan: Value = serde_json::from_slice(&bounded_file(
            &folder.join("plan.json"),
            MAX_PLAN_BYTES,
            false,
        )?)
        .map_err(|_| ("invalid_target", "the selected run has no readable plan"))?;
        if plan.get("kind").and_then(Value::as_str) != Some("fx-graph-v1") {
            return Err((
                "unsupported_version",
                "the selected folder does not hold a supported FX run plan",
            ));
        }
        let plan = plan
            .get("plan")
            .and_then(Value::as_str)
            .filter(|plan| is_digest(plan))
            .ok_or((
                "invalid_target",
                "the selected run has no valid plan identity",
            ))?
            .to_string();
        let events = control_events(&folder, &plan)?;
        let latest = events
            .iter()
            .rfind(|event| event.get("event").and_then(Value::as_str) == Some("run_started"));
        let invocation = latest
            .and_then(|e| e.get("invocation_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut record = Self {
            folder_digest: folder_key(&folder),
            folder,
            plan,
            invocation,
            state: if latest.is_some() {
                "unfinished"
            } else {
                "planned"
            }
            .into(),
            accepted: false,
            terminal: None,
        };
        record.apply_events(&events);
        Ok(record)
    }

    fn apply_events(&mut self, events: &[Value]) {
        for event in events {
            if event.get("invocation_id").and_then(Value::as_str) != self.invocation.as_deref() {
                continue;
            }
            match event.get("event").and_then(Value::as_str) {
                Some("cancel_requested") => self.accepted = true,
                Some("run_cancelled") => {
                    self.state = "cancelled".into();
                    self.terminal = Some(event.clone());
                }
                Some("run_finished") => {
                    self.state = if event.get("ok").and_then(Value::as_bool) == Some(true) {
                        "succeeded"
                    } else {
                        "failed"
                    }
                    .into();
                    self.terminal = Some(event.clone());
                }
                _ => {}
            }
        }
    }

    fn refresh_bound(&mut self) -> bool {
        match control_events(&self.folder, &self.plan) {
            Ok(events) => {
                self.apply_events(&events);
                true
            }
            Err(_) => false,
        }
    }

    fn result(&self, operation: &str) -> ControlResult {
        let mut result = ControlResult::new(operation);
        result.invocation_id.clone_from(&self.invocation);
        result.recorded_state.clone_from(&self.state);
        if self.accepted {
            result.request_status = "accepted".into();
        }
        result
    }
}

fn folder_key(folder: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        sha256_hex(folder.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        sha256_hex(folder.to_string_lossy().as_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    version: u8,
    folder_digest: String,
    canonical_folder: PathBuf,
    plan: String,
    invocation_id: String,
    owner_nonce: String,
    socket: String,
    token: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    operation: String,
    folder_digest: String,
    plan: String,
    invocation_id: String,
    owner_nonce: String,
    token: String,
    source: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u8,
    folder_digest: String,
    plan: String,
    invocation_id: String,
    owner_nonce: String,
    outcome: String,
    request_status: String,
    phase: String,
    code: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u8,
    folder_digest: String,
    plan: String,
    invocation_id: String,
    owner_nonce: String,
    terminal_digest: String,
    request_status: String,
}

#[cfg(unix)]
mod private {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };

    pub fn uid() -> Result<u32, &'static str> {
        // FX does not change process credentials. Cache the effective UID so polling does
        // not repeatedly start a subprocess; caller environment never selects the identity.
        static UID: std::sync::OnceLock<Result<u32, &'static str>> = std::sync::OnceLock::new();
        *UID.get_or_init(|| {
            let output = std::process::Command::new("/usr/bin/id")
                .arg("-u")
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|_| "unavailable")?;
            if !output.status.success() {
                return Err("unavailable");
            }
            std::str::from_utf8(&output.stdout)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .ok_or("unavailable")
        })
    }

    pub fn root(create: bool) -> Result<PathBuf, &'static str> {
        let tmp = Path::new("/tmp")
            .canonicalize()
            .map_err(|_| "unavailable")?;
        let uid = uid()?;
        let parent = tmp.join(format!("grida-fx-{uid}"));
        let root = parent.join("control-v1");
        for path in [&parent, &root] {
            if create {
                match std::fs::DirBuilder::new().mode(0o700).create(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err("unavailable"),
                }
            }
            let metadata = std::fs::symlink_metadata(path).map_err(|_| "unavailable")?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != uid
                || metadata.mode() & 0o777 != 0o700
            {
                return Err("unauthorized");
            }
        }
        Ok(root)
    }

    pub fn validate(path: &Path, socket: bool) -> Result<(), &'static str> {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| "unavailable")?;
        let kind = if socket {
            metadata.file_type().is_socket()
        } else {
            metadata.is_file()
        };
        if !kind
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid()?
            || metadata.mode() & 0o777 != 0o600
        {
            return Err("unauthorized");
        }
        Ok(())
    }

    pub fn read<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, &'static str> {
        validate(path, false)?;
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| "unavailable")?
            .take((MAX_PAYLOAD + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "unavailable")?;
        if bytes.len() > MAX_PAYLOAD {
            return Err("unauthorized");
        }
        serde_json::from_slice(&bytes).map_err(|_| "unsupported_version")
    }

    pub fn random() -> Result<String, &'static str> {
        let mut bytes = [0; 16];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(|_| "unavailable")?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), &'static str> {
        let bytes = serde_json::to_vec(value).map_err(|_| "record_error")?;
        if bytes.len() > MAX_PAYLOAD {
            return Err("record_error");
        }
        if std::fs::symlink_metadata(path).is_ok() {
            validate(path, false)?;
        }
        let temporary = path.with_file_name(format!("t-{}", random()?));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|_| "record_error")?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| "record_error")?;
            std::fs::rename(&temporary, path).map_err(|_| "record_error")?;
            Ok(())
        })();
        let _ = std::fs::remove_file(temporary);
        result
    }

    pub fn socket_mode(path: &Path) -> Result<(), &'static str> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "unavailable")
    }
}

fn pointer_path(root: &Path, key: &str) -> PathBuf {
    root.join(format!("p-{key}.json"))
}
fn receipt_path(root: &Path, owner: &Owner) -> PathBuf {
    root.join(format!(
        "r-{}-{}-{}.json",
        owner.folder_digest, owner.invocation_id, owner.owner_nonce
    ))
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn validate_owner(owner: &Owner, record: &Record) -> Result<(), &'static str> {
    if owner.version != 1 {
        return Err("unsupported_version");
    }
    if owner.folder_digest != record.folder_digest
        || owner.canonical_folder != record.folder
        || owner.plan != record.plan
    {
        return Err("unauthorized");
    }
    if Some(owner.invocation_id.as_str()) != record.invocation.as_deref() {
        return Err("invocation_mismatch");
    }
    if !hex(&owner.invocation_id, 16)
        || !hex(&owner.owner_nonce, 32)
        || !hex(&owner.token, 32)
        || owner.socket != format!("s-{}", owner.owner_nonce)
        || !is_digest(&owner.plan)
    {
        return Err("unauthorized");
    }
    Ok(())
}

/// A runner-owned listener. Construction is permitted only while holding the canonical run lock,
/// after the run_started record has been flushed. Errors leave ordinary execution available.
pub struct ControlServer {
    #[cfg(unix)]
    root: PathBuf,
    #[cfg(unix)]
    owner: Owner,
    #[cfg(unix)]
    stopped: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(unix)]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(unix)]
    control: Arc<RunControl>,
}

impl ControlServer {
    pub fn start(
        folder: &Path,
        plan: &str,
        invocation: &str,
        control: Arc<RunControl>,
        _handle: &tokio::runtime::Handle,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        {
            let attempt = || -> Result<Self, &'static str> {
                let folder = folder.canonicalize().map_err(|_| "unavailable")?;
                let root = private::root(true)?;
                let owner_nonce = private::random()?;
                let owner = Owner {
                    version: 1,
                    folder_digest: folder_key(&folder),
                    canonical_folder: folder,
                    plan: plan.into(),
                    invocation_id: invocation.into(),
                    socket: format!("s-{owner_nonce}"),
                    owner_nonce,
                    token: private::random()?,
                };
                let socket = root.join(&owner.socket);
                let listener =
                    std::os::unix::net::UnixListener::bind(&socket).map_err(|_| "unavailable")?;
                private::socket_mode(&socket)?;
                listener.set_nonblocking(true).map_err(|_| "unavailable")?;
                if let Err(error) =
                    private::write(&pointer_path(&root, &owner.folder_digest), &owner)
                {
                    let _ = std::fs::remove_file(&socket);
                    return Err(error);
                }
                let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let stop = Arc::clone(&stopped);
                let identity = owner.clone();
                let state = Arc::clone(&control);
                let thread = std::thread::Builder::new()
                    .name("fx-run-control".into())
                    .spawn(move || {
                        while !stop.load(std::sync::atomic::Ordering::Acquire) {
                            match listener.accept() {
                                Ok((stream, _)) => serve(stream, &identity, &state),
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(Duration::from_millis(10))
                                }
                                Err(_) => break,
                            }
                        }
                    })
                    .map_err(|_| "unavailable")?;
                Ok(Self {
                    root,
                    owner,
                    stopped,
                    thread: Some(thread),
                    control,
                })
            };
            attempt().map_err(str::to_string)
        }
        #[cfg(not(unix))]
        {
            let _ = (folder, plan, invocation, control);
            Err("unsupported_version".into())
        }
    }

    /// Called only after local cleanup and release of run writer ownership.
    pub fn complete(&self, terminal: &Value) -> Result<(), String> {
        #[cfg(unix)]
        {
            if terminal.get("invocation_id").and_then(Value::as_str)
                != Some(&self.owner.invocation_id)
                || terminal.get("plan").and_then(Value::as_str) != Some(&self.owner.plan)
                || !matches!(
                    terminal.get("event").and_then(Value::as_str),
                    Some("run_finished" | "run_cancelled")
                )
            {
                return Err("record_error".into());
            }
            let receipt = Receipt {
                version: 1,
                folder_digest: self.owner.folder_digest.clone(),
                plan: self.owner.plan.clone(),
                invocation_id: self.owner.invocation_id.clone(),
                owner_nonce: self.owner.owner_nonce.clone(),
                terminal_digest: digest(terminal),
                request_status: if self.control.cancellation_requested() {
                    "accepted"
                } else {
                    "not_accepted"
                }
                .into(),
            };
            private::root(false)
                .and_then(|_| private::write(&receipt_path(&self.root, &self.owner), &receipt))
                .map_err(str::to_string)
        }
        #[cfg(not(unix))]
        {
            let _ = terminal;
            Err("unsupported_version".into())
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.stopped
                .store(true, std::sync::atomic::Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            // The immutable socket name belongs to this nonce, never to a successor. Keep the
            // pointer as temporary completion discovery until a lock-owning runner replaces it.
            if private::root(false).is_ok()
                && private::validate(&self.root.join(&self.owner.socket), true).is_ok()
            {
                let _ = std::fs::remove_file(self.root.join(&self.owner.socket));
            }
        }
    }
}

#[cfg(unix)]
fn serve(mut stream: std::os::unix::net::UnixStream, owner: &Owner, control: &RunControl) {
    use std::io::{Read, Write};
    let started = Instant::now();
    let mut bytes = Vec::new();
    loop {
        let remaining = TRANSPORT_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return;
        }
        let mut chunk = [0; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.len() > MAX_PAYLOAD {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
    let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
        return;
    };
    let mut reply = Reply {
        version: 1,
        folder_digest: owner.folder_digest.clone(),
        plan: owner.plan.clone(),
        invocation_id: owner.invocation_id.clone(),
        owner_nonce: owner.owner_nonce.clone(),
        outcome: "inspected".into(),
        request_status: "not_accepted".into(),
        phase: "running".into(),
        code: None,
    };
    if request.version != 1 {
        reply.code = Some("unsupported_version".into());
    } else if request.folder_digest != owner.folder_digest
        || request.plan != owner.plan
        || request.owner_nonce != owner.owner_nonce
        || request.token != owner.token
    {
        reply.code = Some("unauthorized".into());
    } else if request.invocation_id != owner.invocation_id {
        reply.code = Some("invocation_mismatch".into());
    } else if !matches!(request.operation.as_str(), "inspect" | "cancel")
        || !matches!(request.source.as_str(), "cli" | "sdk")
    {
        reply.code = Some("unsupported_version".into());
    } else {
        if request.operation == "cancel" {
            let source = if request.source == "sdk" {
                CancelSource::Sdk
            } else {
                CancelSource::Cli
            };
            reply.outcome = match control.request_cancel(source) {
                CancelAcceptance::Accepted => "accepted",
                CancelAcceptance::AlreadyRequested => "already_requested",
                CancelAcceptance::Finishing => "finishing",
                CancelAcceptance::AlreadyTerminal => "already_terminal",
                CancelAcceptance::RecordError => {
                    reply.code = Some("record_error".into());
                    "error"
                }
            }
            .into();
        }
        reply.phase = match control.phase() {
            ControlPhase::Running => "running",
            ControlPhase::CancelRequested => "cancel_requested",
            ControlPhase::RecordError => "record_error",
            ControlPhase::Finishing => "finishing",
            ControlPhase::Terminal => "terminal",
        }
        .into();
        if control.cancellation_requested() {
            reply.request_status = "accepted".into();
        }
        if reply.phase == "record_error" {
            reply.request_status = "unknown".into();
        }
    }
    if reply.code.is_some() {
        reply.outcome = "error".into();
    }
    if let Ok(mut bytes) = serde_json::to_vec(&reply) {
        bytes.push(b'\n');
        let mut pending = bytes.as_slice();
        while !pending.is_empty() {
            let remaining = TRANSPORT_TIMEOUT.saturating_sub(started.elapsed());
            if remaining.is_zero() || stream.set_write_timeout(Some(remaining)).is_err() {
                return;
            }
            match stream.write(pending) {
                Ok(0) => return,
                Ok(count) => pending = &pending[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct ExchangeError {
    code: &'static str,
    may_have_sent: bool,
    definite_owner_loss: bool,
}

#[cfg(unix)]
fn exchange(
    root: &Path,
    owner: &Owner,
    operation: &str,
    source: &str,
    remaining: Duration,
) -> Result<Reply, ExchangeError> {
    let before_send = |code| ExchangeError {
        code,
        may_have_sent: false,
        definite_owner_loss: false,
    };
    private::root(false).map_err(before_send)?;
    if std::fs::symlink_metadata(root.join(&owner.socket))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Err(ExchangeError {
            code: "unavailable",
            may_have_sent: false,
            definite_owner_loss: true,
        });
    }
    private::validate(&root.join(&owner.socket), true).map_err(before_send)?;
    let request = Request {
        version: 1,
        operation: operation.into(),
        folder_digest: owner.folder_digest.clone(),
        plan: owner.plan.clone(),
        invocation_id: owner.invocation_id.clone(),
        owner_nonce: owner.owner_nonce.clone(),
        token: owner.token.clone(),
        source: source.into(),
    };
    let bytes = serde_json::to_vec(&request).map_err(|_| before_send("record_error"))?;
    if bytes.len() > MAX_PAYLOAD {
        return Err(before_send("record_error"));
    }
    let sent = std::sync::atomic::AtomicBool::new(false);
    let owner_lost = std::sync::atomic::AtomicBool::new(false);
    let fault = |code| ExchangeError {
        code,
        may_have_sent: sent.load(std::sync::atomic::Ordering::Acquire),
        definite_owner_loss: owner_lost.load(std::sync::atomic::Ordering::Acquire),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| before_send("unavailable"))?;
    let result = runtime.block_on(async {
        tokio::time::timeout(remaining.min(TRANSPORT_TIMEOUT), async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut stream = tokio::net::UnixStream::connect(root.join(&owner.socket))
                .await
                .map_err(|error| {
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) {
                        owner_lost.store(true, std::sync::atomic::Ordering::Release);
                    }
                    "unavailable"
                })?;
            sent.store(true, std::sync::atomic::Ordering::Release);
            stream
                .write_all(&bytes)
                .await
                .map_err(|_| "acknowledgment_unknown")?;
            stream
                .shutdown()
                .await
                .map_err(|_| "acknowledgment_unknown")?;
            let mut response = Vec::new();
            stream
                .take((MAX_PAYLOAD + 1) as u64)
                .read_to_end(&mut response)
                .await
                .map_err(|_| "acknowledgment_unknown")?;
            if response.len() > MAX_PAYLOAD {
                return Err("unsupported_version");
            }
            serde_json::from_slice::<Reply>(&response).map_err(|_| "unsupported_version")
        })
        .await
    });
    let reply = match result {
        Ok(result) => result.map_err(fault)?,
        Err(_) => {
            return Err(fault(
                if sent.load(std::sync::atomic::Ordering::Acquire) && operation == "cancel" {
                    "acknowledgment_unknown"
                } else {
                    "unavailable"
                },
            ));
        }
    };
    if reply.version != 1
        || reply.folder_digest != owner.folder_digest
        || reply.plan != owner.plan
        || reply.invocation_id != owner.invocation_id
        || reply.owner_nonce != owner.owner_nonce
    {
        return Err(fault("unauthorized"));
    }
    if !matches!(
        reply.outcome.as_str(),
        "inspected" | "accepted" | "already_requested" | "finishing" | "already_terminal" | "error"
    ) || !matches!(
        reply.request_status.as_str(),
        "accepted" | "not_accepted" | "unknown"
    ) || !matches!(
        reply.phase.as_str(),
        "running" | "cancel_requested" | "record_error" | "finishing" | "terminal"
    ) || reply.code.as_deref().is_some_and(|code| {
        !matches!(
            code,
            "unsupported_version" | "unauthorized" | "invocation_mismatch" | "record_error"
        )
    }) {
        return Err(fault("unsupported_version"));
    }
    if (reply.outcome == "error") != reply.code.is_some()
        || (operation == "inspect" && !matches!(reply.outcome.as_str(), "inspected" | "error"))
        || (operation == "cancel" && reply.outcome == "inspected")
        || (matches!(reply.outcome.as_str(), "accepted" | "already_requested")
            && reply.request_status != "accepted")
    {
        return Err(fault("unsupported_version"));
    }
    Ok(reply)
}

#[cfg(unix)]
fn completed(root: &Path, owner: &Owner, record: &Record) -> bool {
    let Some(terminal) = &record.terminal else {
        return false;
    };
    private::root(false).is_ok()
        && private::read::<Receipt>(&receipt_path(root, owner)).is_ok_and(|receipt| {
            receipt.version == 1
                && receipt.folder_digest == owner.folder_digest
                && receipt.plan == owner.plan
                && receipt.invocation_id == owner.invocation_id
                && receipt.owner_nonce == owner.owner_nonce
                && receipt.terminal_digest == digest(terminal)
                && matches!(receipt.request_status.as_str(), "accepted" | "not_accepted")
        })
}

fn explanation(code: &str) -> &'static str {
    match code {
        "invocation_mismatch" => {
            "the selected run invocation changed; inspect the exact target again"
        }
        "unsupported_version" => "compatible local run control is not supported",
        "unauthorized" => {
            "private local control ownership, permissions or identity could not be verified"
        }
        "record_error" => "cancellation could not be durably recorded; cleanup remains unverified",
        "wait_timeout" => "the wait deadline expired; any accepted cancellation remains in effect",
        "owner_lost" => "the selected owner was lost before local completion could be verified",
        "acknowledgment_unknown" => {
            "the cancellation acknowledgment was lost; acceptance is unknown"
        }
        "completion_unverified" => "the terminal result has no compatible local cleanup receipt",
        _ => "the selected invocation has no verified available control owner",
    }
}

/// Observes saved evidence and verifies the sampled owner with one bounded local handshake.
pub fn inspect(folder: &Path) -> ControlResult {
    operate(
        folder,
        None,
        false,
        Duration::from_secs(30),
        "cli",
        "inspect",
        Instant::now(),
    )
}

/// Cancels exactly one sampled invocation. A wait never switches to a successor invocation.
pub fn cancel(
    folder: &Path,
    invocation: Option<&str>,
    wait: bool,
    timeout: Duration,
    source: &str,
    started: Instant,
) -> ControlResult {
    operate(folder, invocation, wait, timeout, source, "cancel", started)
}

fn operate(
    folder: &Path,
    expected: Option<&str>,
    wait: bool,
    timeout: Duration,
    source: &str,
    operation: &str,
    started: Instant,
) -> ControlResult {
    let mut record = match Record::read(folder) {
        Ok(record) => record,
        Err((code, message)) => return ControlResult::error(operation, code, message),
    };
    let mut result = record.result(operation);
    if expected.is_some() && expected != record.invocation.as_deref() {
        result.fail("invocation_mismatch", explanation("invocation_mismatch"));
        return result;
    }
    #[cfg(not(unix))]
    {
        let _ = (wait, timeout, source, started);
        if operation == "inspect" {
            result.availability = Some("unsupported".into());
            return result;
        }
        result.fail("unsupported_version", explanation("unsupported_version"));
        result
    }
    #[cfg(unix)]
    {
        let remaining = || timeout.saturating_sub(started.elapsed());
        if wait && remaining().is_zero() {
            result.fail("wait_timeout", explanation("wait_timeout"));
            return result;
        }
        let discovery = private::root(false).and_then(|root| {
            let owner: Owner = private::read(&pointer_path(&root, &record.folder_digest))?;
            validate_owner(&owner, &record)?;
            Ok((root, owner))
        });
        let (root, owner) = match discovery {
            Ok(found) => found,
            Err(code) => {
                if operation == "inspect" && code == "unsupported_version" {
                    result.availability = Some("unsupported".into());
                    return result;
                }
                if operation == "inspect" && matches!(code, "unavailable" | "invocation_mismatch") {
                    result.availability = Some("unavailable".into());
                    return result;
                }
                if record.terminal.is_some() && code == "unavailable" {
                    if wait {
                        result.fail(
                            "completion_unverified",
                            explanation("completion_unverified"),
                        );
                    } else {
                        result.outcome = "already_terminal".into();
                    }
                    return result;
                }
                result.fail(code, explanation(code));
                return result;
            }
        };
        let receipt_verified = completed(&root, &owner, &record);
        if receipt_verified {
            result.cleanup = "complete".into();
        }
        if record.terminal.is_some() && operation == "cancel" {
            result.outcome = "already_terminal".into();
            if !wait {
                return result;
            }
            if receipt_verified {
                result.outcome = "completed".into();
                return result;
            }
        }
        let reply = exchange(
            &root,
            &owner,
            operation,
            source,
            if wait { remaining() } else { TRANSPORT_TIMEOUT },
        );
        let mut last_handshake = None;
        match reply {
            Ok(reply) => {
                last_handshake = Some(Instant::now());
                result.request_status = reply.request_status;
                result.cleanup = "pending".into();
                if let Some(code) = reply.code {
                    result.fail(&code, explanation(&code));
                    return result;
                }
                if record.refresh_bound() {
                    result.recorded_state = record.state.clone();
                }
                if completed(&root, &owner, &record) {
                    result.cleanup = "complete".into();
                }
                if operation == "inspect" {
                    result.availability = Some("available".into());
                    result.can_cancel = Some(matches!(
                        reply.phase.as_str(),
                        "running" | "cancel_requested"
                    ));
                    return result;
                }
                result.outcome = reply.outcome;
            }
            Err(error) => {
                let code = error.code;
                if operation == "inspect" && code == "unsupported_version" {
                    result.availability = Some("unsupported".into());
                    return result;
                }
                if operation == "inspect" && code == "unavailable" {
                    result.availability = Some("unavailable".into());
                    return result;
                }
                record.refresh_bound();
                result.recorded_state = record.state.clone();
                result.cleanup = "unknown".into();
                if record.accepted {
                    result.request_status = "accepted".into();
                } else if error.may_have_sent {
                    result.request_status = "unknown".into();
                }
                if completed(&root, &owner, &record) {
                    result.cleanup = "complete".into();
                    if wait {
                        result.outcome = "completed".into();
                        return result;
                    }
                }
                if !wait || (!error.may_have_sent && !record.accepted && record.terminal.is_none())
                {
                    result.fail(code, explanation(code));
                    return result;
                }
            }
        }
        if !wait {
            return result;
        }
        loop {
            record.refresh_bound();
            result.recorded_state = record.state.clone();
            if record.accepted {
                result.request_status = "accepted".into();
            }
            if completed(&root, &owner, &record) {
                result.outcome = "completed".into();
                result.cleanup = "complete".into();
                return result;
            }
            if remaining().is_zero() {
                result.fail("wait_timeout", explanation("wait_timeout"));
                return result;
            }
            if last_handshake.is_none_or(|at: Instant| at.elapsed() >= Duration::from_millis(250)) {
                // A socket inode can survive a crash. Probe only the immutable sampled owner,
                // never the folder's replacement pointer or a newer invocation.
                match exchange(&root, &owner, "inspect", "cli", remaining()) {
                    Ok(reply) if reply.code.is_none() => {
                        result.cleanup = "pending".into();
                        if reply.request_status == "accepted" {
                            result.request_status = "accepted".into();
                        }
                    }
                    Ok(_) => result.cleanup = "unknown".into(),
                    Err(error) => {
                        result.cleanup = "unknown".into();
                        if error.definite_owner_loss {
                            record.refresh_bound();
                            result.recorded_state = record.state.clone();
                            if record.accepted {
                                result.request_status = "accepted".into();
                            }
                            if completed(&root, &owner, &record) {
                                result.outcome = "completed".into();
                                result.cleanup = "complete".into();
                                return result;
                            }
                            let code = if record.terminal.is_some() {
                                "completion_unverified"
                            } else {
                                "owner_lost"
                            };
                            result.fail(code, explanation(code));
                            return result;
                        }
                    }
                }
                last_handshake = Some(Instant::now());
            }
            std::thread::sleep(remaining().min(Duration::from_millis(25)));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::Cancel;
    use crate::events::{Event, EventLog};
    use grida_fx_core::money::Usd;
    use serde_json::json;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};

    fn start(folder: &Path, invocation: &str) -> (Arc<RunControl>, ControlServer) {
        let plan = "0".repeat(64);
        std::fs::write(
            folder.join("plan.json"),
            json!({"kind":"fx-graph-v1", "plan":plan, "workflow":{"id":"case"}}).to_string(),
        )
        .unwrap();
        let events =
            Arc::new(EventLog::open(&folder.join("events.jsonl"), invocation, &plan).unwrap());
        events
            .emit(&Event::RunStarted {
                workflow: "case".into(),
                name: None,
                created_at: "2026-10-08T00:00:00.000Z".into(),
                resumed: false,
                ceiling_usd: None,
                charged_usd: Usd::ZERO,
                estimate_low: Usd::ZERO,
                estimate_high: Usd::ZERO,
                stand_in: false,
            })
            .unwrap();
        let control = RunControl::new(events, Cancel::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let server =
            ControlServer::start(folder, &plan, invocation, control.clone(), runtime.handle())
                .unwrap();
        (control, server)
    }

    fn terminal(control: &RunControl) -> Value {
        control
            .emit_terminal(&Event::RunCancelled {
                reason: "interrupted".into(),
                charged_usd: Usd::ZERO,
            })
            .unwrap();
        control.terminal().unwrap()
    }

    #[test]
    fn authenticated_cancellation_is_idempotent_and_keeps_private_identity_private() {
        let folder = tempfile::tempdir().unwrap();
        let (control, server) = start(folder.path(), "0123456789abcdef");
        let inspected = inspect(folder.path());
        assert_eq!(inspected.availability.as_deref(), Some("available"));
        assert_eq!(inspected.can_cancel, Some(true));
        let accepted = cancel(
            folder.path(),
            None,
            false,
            Duration::from_secs(30),
            "cli",
            Instant::now(),
        );
        assert_eq!(accepted.outcome, "accepted", "{accepted:?}");
        assert_eq!(accepted.request_status, "accepted");
        assert_eq!(
            cancel(
                folder.path(),
                None,
                false,
                Duration::from_secs(30),
                "sdk",
                Instant::now()
            )
            .outcome,
            "already_requested"
        );
        let public = serde_json::to_string(&accepted).unwrap();
        assert!(!public.contains(&server.owner.token));
        assert!(!public.contains(&server.owner.owner_nonce));
        assert!(!public.contains(folder.path().to_str().unwrap()));
        assert_eq!(
            crate::events::read_events(&folder.path().join("events.jsonl"))
                .unwrap()
                .iter()
                .filter(|event| event["event"] == "cancel_requested")
                .count(),
            1
        );
        assert!(control.admit().is_none());
        let _ = std::fs::remove_file(pointer_path(&server.root, &server.owner.folder_digest));
    }

    #[test]
    fn bad_token_cannot_mutate_and_stale_pointer_does_not_select_a_successor() {
        let folder = tempfile::tempdir().unwrap();
        let (control, server) = start(folder.path(), "0123456789abcdef");
        let mut wrong = server.owner.clone();
        wrong.token = "f".repeat(32);
        assert_eq!(
            exchange(&server.root, &wrong, "cancel", "cli", TRANSPORT_TIMEOUT)
                .unwrap()
                .code
                .as_deref(),
            Some("unauthorized")
        );
        assert!(!control.cancellation_requested());
        let mut stale = server.owner.clone();
        stale.invocation_id = "fedcba9876543210".into();
        private::write(
            &pointer_path(&server.root, &server.owner.folder_digest),
            &stale,
        )
        .unwrap();
        let result = cancel(
            folder.path(),
            None,
            false,
            Duration::from_secs(30),
            "cli",
            Instant::now(),
        );
        assert_eq!(result.code.as_deref(), Some("invocation_mismatch"));
        assert!(!control.cancellation_requested());
        let _ = std::fs::remove_file(pointer_path(&server.root, &server.owner.folder_digest));
    }

    #[test]
    fn completion_receipt_survives_resume_and_matches_the_exact_terminal_event() {
        let folder = tempfile::tempdir().unwrap();
        let (first, a) = start(folder.path(), "0123456789abcdef");
        first.request_cancel(CancelSource::Cli);
        let terminal_a = terminal(&first);
        a.complete(&terminal_a).unwrap();
        let mut record_a = Record::read(folder.path()).unwrap();
        assert!(completed(&a.root, &a.owner, &record_a));
        let (second, b) = start(folder.path(), "fedcba9876543210");
        record_a.refresh_bound();
        assert_eq!(record_a.state, "cancelled");
        assert!(completed(&a.root, &a.owner, &record_a));
        let guarded = cancel(
            folder.path(),
            Some("0123456789abcdef"),
            false,
            Duration::from_secs(30),
            "cli",
            Instant::now(),
        );
        assert_eq!(guarded.code.as_deref(), Some("invocation_mismatch"));
        assert!(!second.cancellation_requested());
        let root = a.root.clone();
        let owner_a = a.owner.clone();
        let owner_b = b.owner.clone();
        drop(a);
        let current: Owner = private::read(&pointer_path(&root, &owner_b.folder_digest)).unwrap();
        assert_eq!(current.owner_nonce, owner_b.owner_nonce);
        record_a.terminal.as_mut().unwrap()["reason"] = json!("changed");
        assert!(!completed(&root, &owner_a, &record_a));
        let _ = std::fs::remove_file(receipt_path(&root, &owner_a));
        let _ = std::fs::remove_file(pointer_path(&root, &owner_b.folder_digest));
    }

    #[test]
    fn terminal_history_without_receipt_never_claims_verified_cleanup() {
        let folder = tempfile::tempdir().unwrap();
        let (control, server) = start(folder.path(), "0123456789abcdef");
        control.request_cancel(CancelSource::Cli);
        terminal(&control);
        let root = server.root.clone();
        let owner = server.owner.clone();
        drop(server);
        let result = cancel(
            folder.path(),
            None,
            true,
            Duration::from_secs(1),
            "cli",
            Instant::now(),
        );
        assert_eq!(result.code.as_deref(), Some("completion_unverified"));
        assert_eq!(result.recorded_state, "cancelled");
        assert_ne!(result.cleanup, "complete");
        let _ = std::fs::remove_file(pointer_path(&root, &owner.folder_digest));
    }

    #[test]
    fn waiting_deadline_leaves_cancellation_accepted() {
        let folder = tempfile::tempdir().unwrap();
        let (control, server) = start(folder.path(), "0123456789abcdef");
        let result = cancel(
            folder.path(),
            None,
            true,
            Duration::from_millis(80),
            "cli",
            Instant::now(),
        );
        assert_eq!(result.code.as_deref(), Some("wait_timeout"));
        assert_eq!(result.request_status, "accepted");
        assert!(control.cancellation_requested());
        let _ = std::fs::remove_file(pointer_path(&server.root, &server.owner.folder_digest));
    }

    #[test]
    fn private_documents_refuse_permissions_symlinks_extra_fields_and_oversize() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("record.json");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        std::fs::write(&path, "{} ").unwrap();
        assert!(private::read::<Receipt>(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(private::read::<Receipt>(&path).err(), Some("unauthorized"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = folder.path().join("linked.json");
        symlink(&path, &link).unwrap();
        assert_eq!(private::read::<Receipt>(&link).err(), Some("unauthorized"));
        std::fs::write(&path, vec![b' '; MAX_PAYLOAD + 1]).unwrap();
        assert_eq!(private::read::<Receipt>(&path).err(), Some("unauthorized"));
        let record: Value = json!({"version":1,"folder_digest":"0".repeat(64),"plan":"0".repeat(64),"invocation_id":"0123456789abcdef","owner_nonce":"0".repeat(32),"terminal_digest":"0".repeat(64),"request_status":"not_accepted","extra":true});
        std::fs::write(&path, record.to_string()).unwrap();
        assert_eq!(
            private::read::<Receipt>(&path).err(),
            Some("unsupported_version")
        );
    }

    #[test]
    fn lost_or_truncated_acceptance_reply_never_claims_not_accepted() {
        use std::io::{Read, Write};
        for (reply, erase_evidence) in [(Vec::new(), false), (b"{\"version\":1".to_vec(), true)] {
            let folder = tempfile::tempdir().unwrap();
            let (control, server) = start(folder.path(), "0123456789abcdef");
            let root = server.root.clone();
            let owner = server.owner.clone();
            drop(server);
            let listener =
                std::os::unix::net::UnixListener::bind(root.join(&owner.socket)).unwrap();
            private::socket_mode(&root.join(&owner.socket)).unwrap();
            let state = control.clone();
            let events_path = folder.path().join("events.jsonl");
            let handler = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(TRANSPORT_TIMEOUT)).unwrap();
                let mut bytes = Vec::new();
                (&mut stream)
                    .take((MAX_PAYLOAD + 1) as u64)
                    .read_to_end(&mut bytes)
                    .unwrap();
                let request: Request = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(request.operation, "cancel");
                assert_eq!(
                    state.request_cancel(CancelSource::Cli),
                    CancelAcceptance::Accepted
                );
                if erase_evidence {
                    std::fs::remove_file(events_path).unwrap();
                }
                stream.write_all(&reply).unwrap();
            });
            let result = cancel(
                folder.path(),
                None,
                false,
                Duration::from_secs(2),
                "cli",
                Instant::now(),
            );
            handler.join().unwrap();
            assert_eq!(result.outcome, "error");
            assert_eq!(
                result.request_status,
                if erase_evidence {
                    "unknown"
                } else {
                    "accepted"
                },
                "{result:?}"
            );
            assert_eq!(result.cleanup, "unknown");
            assert!(control.cancellation_requested());
            let _ = std::fs::remove_file(root.join(&owner.socket));
            let _ = std::fs::remove_file(pointer_path(&root, &owner.folder_digest));
        }
    }

    #[test]
    fn waiter_detects_a_crashed_owner_even_when_its_socket_inode_remains() {
        let folder = tempfile::tempdir().unwrap();
        let (control, mut server) = start(folder.path(), "0123456789abcdef");
        let selected = folder.path().to_path_buf();
        let waiter = std::thread::spawn(move || {
            cancel(
                &selected,
                None,
                true,
                Duration::from_secs(3),
                "cli",
                Instant::now(),
            )
        });
        let began = Instant::now();
        while !control.cancellation_requested() {
            assert!(began.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(5));
        }
        server
            .stopped
            .store(true, std::sync::atomic::Ordering::Release);
        server.thread.take().unwrap().join().unwrap();
        assert!(private::validate(&server.root.join(&server.owner.socket), true).is_ok());
        let result = waiter.join().unwrap();
        assert_eq!(result.code.as_deref(), Some("owner_lost"), "{result:?}");
        assert_eq!(result.request_status, "accepted");
        assert_eq!(result.cleanup, "unknown");
        assert!(began.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_file(pointer_path(&server.root, &server.owner.folder_digest));
    }
}
