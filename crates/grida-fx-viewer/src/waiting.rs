//! Browser adapter for the runtime's exact-run response operation.
use super::{AppState, Source, failure, json_submission, local_submission, observed_json};
use axum::body::Bytes;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use grida_fx_core::value::parse_json;
use grida_fx_runtime::waiting::{self, ResponseChannel, WaitingError};
use serde_json::json;
use std::sync::Arc;

pub(super) async fn list(State(host): State<Arc<AppState>>) -> Response {
    let Source::Run(root) = &host.source else {
        return failure(StatusCode::NOT_FOUND, "A plan has no run requests.");
    };
    let root = root.clone();
    match tokio::task::spawn_blocking(move || waiting::list(&root)).await {
        Ok(Ok(requests)) => observed_json(json!({ "requests": requests })),
        Ok(Err(error)) => waiting_error(error),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not read run requests.",
        ),
    }
}

pub(super) async fn respond(
    State(host): State<Arc<AppState>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let Source::Run(root) = &host.source else {
        return failure(StatusCode::NOT_FOUND, "A plan has no run requests.");
    };
    if !local_submission(request.headers(), &host.authority, "respond") {
        return failure(
            StatusCode::FORBIDDEN,
            "Submit the answer from this local viewer.",
        );
    }
    if !json_submission(request.headers()) {
        return failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Submit the answer as application/json.",
        );
    }
    let bytes = match Bytes::from_request(request, &()).await {
        Ok(body) => body,
        Err(rejection) => {
            return failure(rejection.status(), "Could not read the submitted answer.");
        }
    };
    let answer = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| parse_json(text).ok())
        .and_then(|mut body| {
            let object = body.as_object_mut()?;
            (object.len() == 1)
                .then(|| object.remove("answer"))
                .flatten()
        });
    let Some(answer) = answer else {
        return failure(
            StatusCode::BAD_REQUEST,
            "Expected a JSON object containing only an answer.",
        );
    };
    let root = root.clone();
    match tokio::task::spawn_blocking(move || {
        waiting::respond(&root, &id, answer, ResponseChannel::Viewer)
    })
    .await
    {
        Ok(Ok(request)) => observed_json(json!(request)),
        Ok(Err(error)) => waiting_error(error),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not record the answer.",
        ),
    }
}

fn waiting_error(error: WaitingError) -> Response {
    let status = match error.code() {
        "not_found" | "invalid_run" => StatusCode::NOT_FOUND,
        "answer_conflict" | "expired" => StatusCode::CONFLICT,
        "invalid_answer" | "invalid_request" => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        observed_json(json!({
            "code": error.code(), "error": error.to_string(), "request": error.request(),
        })),
    )
        .into_response()
}
