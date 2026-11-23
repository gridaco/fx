//! Translate exact-attempt browser actions to local engine operations.

use super::{Catalog, ServiceState, run_details};
use crate::{failure, json_submission, local_submission, observed_json};
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use grida_fx_runtime::{control, run_index};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A viewer action that starts a runner through the owning project service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchOperation {
    Resume,
    Rerun,
}

/// Private launcher request. The service supplies both canonical paths, never the browser.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub operation: LaunchOperation,
    pub project_root: PathBuf,
    pub folder: PathBuf,
    pub expected_invocation: String,
    pub source_attempt: u64,
    pub run_id: String,
    pub request_id: String,
}

/// Receipt from a launcher once the selected runner has recorded its start.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchAccepted {
    pub folder: PathBuf,
    pub invocation_id: String,
}

/// A browser-safe refusal or uncertain start result from the engine launcher.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRefused {
    pub code: String,
    /// This message is shown to the browser. It must contain no private paths or credentials.
    pub message: String,
    pub unknown: bool,
}

/// The engine-owned launcher. It checks the expected invocation against the exact source
/// before starting work. Rerun also uses the request key to recover a previous acceptance.
pub type RunLauncher =
    Arc<dyn Fn(LaunchRequest) -> Result<LaunchAccepted, LaunchRefused> + Send + Sync>;

#[derive(Clone, Copy)]
enum Action {
    Pause,
    Cancel,
    Start(LaunchOperation),
}

impl Action {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Start(LaunchOperation::Resume)),
            "rerun" => Some(Self::Start(LaunchOperation::Rerun)),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionRequest {
    invocation_id: String,
    #[serde(default, deserialize_with = "present_request_id")]
    request_id: Option<String>,
}

fn present_request_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn identifier(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn action_result(outcome: &str, message: &str) -> Value {
    json!({"kind":"fx-viewer-run-action-v1", "outcome":outcome, "message":message})
}

fn refused(status: StatusCode, code: &str, message: &str) -> Response {
    let mut body = action_result("refused", message);
    body["code"] = json!(code);
    (status, observed_json(body)).into_response()
}

fn owned(catalog: &Catalog, folder: &Path) -> bool {
    run_index::recording_project(folder, &catalog.root).as_ref() == Some(&catalog.root)
}

fn unavailable() -> Response {
    refused(
        StatusCode::NOT_FOUND,
        "invalid_target",
        "FX couldn’t find this attempt.",
    )
}

fn eligibility(state: &ServiceState, id: &str) -> Response {
    let Ok((root, details)) = state.catalog.exact_run(id) else {
        return unavailable();
    };
    let control = control::inspect_for_viewer(&root);
    let invocation = control.invocation_id.filter(|id| identifier(id, 256));
    let can_start = state.run_launcher.is_some()
        && control.code.is_none()
        && invocation.is_some()
        && control.availability.as_deref() != Some("available")
        && owned(&state.catalog, &root);
    observed_json(json!({
        "kind":"fx-viewer-run-control-v1",
        "invocation_id":invocation,
        "attempt_number":details.attempt.attempt_number,
        "actions": {
            "pause":control.can_pause.unwrap_or(false) && control.recorded_state != "pausing",
            "resume":can_start && control.recorded_state == "paused",
            "rerun":can_start && matches!(control.recorded_state.as_str(),
                "unfinished" | "paused" | "awaiting" | "succeeded" | "failed" | "cancelled"),
            "cancel":control.can_cancel.unwrap_or(false) && control.request_status != "accepted",
        }
    }))
}

fn active_action(folder: &Path, will_pause: bool, invocation_id: &str) -> Response {
    let operation = if will_pause {
        control::pause
    } else {
        control::cancel
    };
    let result = operation(
        folder,
        Some(invocation_id),
        false,
        Duration::from_secs(30),
        "viewer",
        Instant::now(),
    );
    if result.invocation_id.as_deref() == Some(invocation_id) && result.request_status == "accepted"
    {
        let message = if will_pause {
            "Pause requested. Steps already running can finish."
        } else {
            "Cancellation requested."
        };
        return observed_json(action_result("accepted", message));
    }
    let code = match result.outcome.as_str() {
        "already_terminal" => "already_terminal",
        "finishing" => "finishing",
        _ => result.code.as_deref().unwrap_or("unavailable"),
    };
    if result.request_status == "unknown" {
        let mut body = action_result(
            "unknown",
            "FX couldn’t tell if the request went through. Check the run’s status before retrying.",
        );
        body["code"] = json!(code);
        return (StatusCode::SERVICE_UNAVAILABLE, observed_json(body)).into_response();
    }
    let message = match code {
        "already_terminal" => "This run has already finished.",
        "finishing" => "This run is already finishing.",
        "invocation_mismatch" => "The run changed. Check its status and try again.",
        "unsupported_version" => {
            "The viewer can’t control this version of FX. Check the run in the terminal."
        }
        "record_error" => "FX couldn’t save this request. Check the run before trying again.",
        _ => "FX couldn’t apply this action. Check the run’s status and try again.",
    };
    refused(StatusCode::CONFLICT, code, message)
}

fn start_action(state: &ServiceState, request: LaunchRequest) -> Response {
    let Some(launcher) = &state.run_launcher else {
        return refused(
            StatusCode::CONFLICT,
            "launcher_unavailable",
            "This viewer can’t start runs. Use the terminal to continue.",
        );
    };
    let operation = request.operation;
    let source_attempt = request.source_attempt;
    let source = request.folder.clone();
    let run_id = request.run_id.clone();
    let fallback = |refusal: LaunchRefused| {
        let (outcome, status, message, command) = match (refusal.unknown, operation) {
            (true, _) => (
                "unknown",
                StatusCode::SERVICE_UNAVAILABLE,
                refusal.message,
                format!("grida fx inspect {run_id} --history"),
            ),
            (false, LaunchOperation::Resume) => (
                "refused",
                StatusCode::CONFLICT,
                format!(
                    "{} Use the command below in your terminal to find the folder for attempt {source_attempt}, then pass that folder to grida fx resume.",
                    refusal.message
                ),
                format!("grida fx inspect {run_id} --history"),
            ),
            (false, LaunchOperation::Rerun) => (
                "refused",
                StatusCode::CONFLICT,
                refusal.message,
                format!("grida fx rerun {run_id} --attempt {source_attempt}"),
            ),
        };
        let mut body = action_result(outcome, &message);
        body["code"] = json!(refusal.code);
        body["cli_command"] = json!(command);
        (status, observed_json(body)).into_response()
    };
    if !owned(&state.catalog, &source) {
        return refused(
            StatusCode::CONFLICT,
            "wrong_project",
            "FX couldn’t find the project that started this run. Open that project to continue.",
        );
    }
    let accepted = match launcher(request) {
        Ok(accepted) => accepted,
        Err(error) => return fallback(error),
    };
    let registered = (|| {
        let folder = accepted.folder.canonicalize().ok()?;
        let details = run_details(&folder).ok()?;
        let attempt = details.attempt;
        let matches_operation = match operation {
            LaunchOperation::Resume => folder == source && attempt.attempt_number == source_attempt,
            LaunchOperation::Rerun => folder != source && attempt.attempt_number > source_attempt,
        };
        if !matches_operation
            || attempt.run_id != run_id
            || !owned(&state.catalog, &folder)
            || !identifier(&accepted.invocation_id, 256)
        {
            return None;
        }
        let entry = state
            .catalog
            .register_matching_run(&folder, Some(&details.identity))
            .ok()?;
        (entry.state != "unavailable").then_some(entry.url)
    })();
    let Some(url) = registered else {
        return fallback(LaunchRefused {
            code:"view_unavailable".into(),
            message:"The attempt started, but the viewer couldn’t open it. Check the run history before retrying.".into(),
            unknown:true,
        });
    };
    let message = match operation {
        LaunchOperation::Resume => "Run resumed.",
        LaunchOperation::Rerun => "New attempt started.",
    };
    let mut body = action_result("accepted", message);
    body["url"] = json!(url);
    observed_json(body)
}

fn submit(state: &ServiceState, id: &str, action: Action, request: ActionRequest) -> Response {
    let Ok((root, details)) = state.catalog.exact_run(id) else {
        return unavailable();
    };
    match action {
        Action::Pause | Action::Cancel => active_action(
            &root,
            matches!(action, Action::Pause),
            &request.invocation_id,
        ),
        Action::Start(operation) => start_action(
            state,
            LaunchRequest {
                operation,
                project_root: state.catalog.root.clone(),
                folder: root,
                expected_invocation: request.invocation_id,
                source_attempt: details.attempt.attempt_number,
                run_id: details.attempt.run_id,
                request_id: request.request_id.expect("validated start request"),
            },
        ),
    }
}

pub(super) async fn dispatch(
    state: Arc<ServiceState>,
    id: String,
    suffix: &str,
    request: Request,
) -> Response {
    if suffix == "api/control" {
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return failure(
                StatusCode::METHOD_NOT_ALLOWED,
                "Read run controls with GET.",
            );
        }
        return tokio::task::spawn_blocking(move || eligibility(&state, &id))
            .await
            .unwrap_or_else(|_| unavailable());
    }
    let verb = suffix.strip_prefix("api/control/").unwrap_or_default();
    let Some(action) = Action::parse(verb) else {
        return failure(StatusCode::NOT_FOUND, "Unknown run action.");
    };
    if *request.method() != Method::POST {
        return failure(
            StatusCode::METHOD_NOT_ALLOWED,
            "Submit a run action with POST.",
        );
    }
    if !local_submission(request.headers(), &state.authority, verb) {
        return refused(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Open this run in the local viewer to use its actions.",
        );
    }
    if !json_submission(request.headers()) {
        return refused(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "Submit the action as application/json.",
        );
    }
    let invalid = || {
        refused(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Supply the observed invocation ID and a request ID for a start action.",
        )
    };
    let Ok(bytes) = to_bytes(request.into_body(), 4096).await else {
        return invalid();
    };
    let Ok(request) = serde_json::from_slice::<ActionRequest>(&bytes) else {
        return invalid();
    };
    let key_valid = match action {
        Action::Start(_) => request
            .request_id
            .as_deref()
            .is_some_and(|key| identifier(key, 128)),
        _ => request.request_id.is_none(),
    };
    if !identifier(&request.invocation_id, 256) || !key_valid {
        return invalid();
    }
    tokio::task::spawn_blocking(move || submit(&state, &id, action, request))
        .await
        .unwrap_or_else(|_| {
            let mut body = action_result(
                "unknown",
                "FX couldn’t tell if the request went through. Check the run’s status before retrying.",
            );
            body["code"] = json!("unavailable");
            (StatusCode::SERVICE_UNAVAILABLE, observed_json(body)).into_response()
        })
}
