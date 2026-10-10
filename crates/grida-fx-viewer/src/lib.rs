//! Read-only browser views for FX runs and materialized plans, hosted independently or by a
//! project service with a private local catalog.
//!
//! The caller owns argument parsing, browser opening and process interruption. This crate binds
//! loopback only and starts no workflow, provider adapter or node host. Service discovery is
//! confined to the explicitly configured run directory.

mod layout;
mod read;
mod scopes;
pub mod service;

pub use read::{Artifact, Node, RunDocument};

use axum::body::Body;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path as RoutePath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use grida_fx_core::docs::{Schema, validate};
use grida_fx_core::value::is_digest;
use grida_fx_runtime::observation;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::Value;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

#[derive(RustEmbed)]
#[folder = "../../web/viewer/dist/"]
struct Assets;

#[derive(Clone)]
enum Source {
    Run(PathBuf),
    Plan(Arc<Value>),
}

#[derive(Clone)]
struct AppState {
    source: Source,
    authority: String,
    artifact_prefix: String,
}

/// A bound server. The caller can print its actual address before opening a browser.
pub struct Server {
    listener: tokio::net::TcpListener,
    app: Router,
    url: String,
}

impl Server {
    /// The loopback HTTP URL, including the OS-assigned port.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Runs until the caller interrupts its process or the listener fails.
    pub async fn serve(self) -> io::Result<()> {
        axum::serve(self.listener, self.app).await
    }

    /// Stops accepting requests when the caller ends this invocation.
    pub async fn serve_until(
        self,
        stop: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> io::Result<()> {
        axum::serve(self.listener, self.app)
            .with_graceful_shutdown(stop)
            .await
    }
}

/// Validates one existing run folder and binds loopback. Port zero lets the OS choose safely.
pub async fn bind(run: &Path, port: u16) -> io::Result<Server> {
    let root = run.canonicalize()?;
    let checked = root.clone();
    // Initial hosting checks records only. Artifact hashing belongs to observers,
    // so large files cannot hold up a workflow before its first step.
    tokio::task::spawn_blocking(move || read::read_inventory(&checked))
        .await
        .map_err(io::Error::other)??;
    bind_source(Source::Run(root), port).await
}

/// Validates a materialized expanded graph and serves it unchanged from memory. Planning, if
/// requested, belongs to the caller and ends before this read-only host is started.
pub async fn bind_plan(graph: Value, port: u16) -> io::Result<Server> {
    let source = plan_source(graph)?;
    bind_source(source, port).await
}

fn plan_source(graph: Value) -> io::Result<Source> {
    validate(Schema::Graph, &graph, "plan")
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.message))?;
    scopes::validate_plan(&graph)
        .map_err(|reason| io::Error::new(io::ErrorKind::InvalidData, reason))?;
    Ok(Source::Plan(Arc::new(graph)))
}

async fn bind_source(source: Source, port: u16) -> io::Result<Server> {
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    let authority = listener.local_addr()?.to_string();
    let url = format!("http://{authority}/");
    let app = router(AppState {
        source,
        authority,
        artifact_prefix: String::new(),
    });
    Ok(Server { listener, app, url })
}

fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route("/api/view", get(view_document))
        .route("/api/run", get(run_document))
        .route("/api/snapshot", get(observation_snapshot))
        .route("/api/layout", get(layout_report))
        .route("/api/events", get(observation_events))
        .route("/api/artifacts/{digest}", get(artifact))
        .route("/api/{*path}", get(api_missing))
        .fallback(get(asset))
        .layer(middleware::from_fn_with_state(state.clone(), local_request))
        .with_state(state)
}

async fn local_request(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let expected_origin = format!("http://{}", state.authority);
    let fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    if host != Some(state.authority.as_str())
        || origin.is_some_and(|value| value != expected_origin)
        || matches!(fetch_site, Some("cross-site" | "same-site"))
    {
        return failure(
            StatusCode::FORBIDDEN,
            "This viewer accepts same-origin loopback requests only.",
        );
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}

async fn view_document(State(state): State<Arc<AppState>>) -> Response {
    match &state.source {
        Source::Run(_) => run_document(State(state)).await,
        Source::Plan(graph) => {
            let mut response = Json(graph.as_ref()).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
    }
}

async fn run_document(State(state): State<Arc<AppState>>) -> Response {
    let Source::Run(root) = &state.source else {
        return failure(StatusCode::NOT_FOUND, "A static plan has no recorded run.");
    };
    let root = root.clone();
    match tokio::task::spawn_blocking(move || read::read_run(&root)).await {
        Ok(Ok(mut snapshot)) => {
            prefix_artifacts(&mut snapshot.document, &state.artifact_prefix);
            let mut response = Json(snapshot.document).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        _ => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The selected run cannot be read. Check its plan and record files.",
        ),
    }
}

fn prefix_artifacts(document: &mut RunDocument, prefix: &str) {
    for artifact in &mut document.artifacts {
        if let Some(url) = &mut artifact.url {
            *url = format!("{prefix}{url}");
        }
    }
}

/// A display projection, its layout report and its cursor use the same captured event prefix.
async fn observation_snapshot(State(state): State<Arc<AppState>>) -> Response {
    let Source::Run(root) = &state.source else {
        return observation_failure("unavailable", "A static plan has no recorded run.");
    };
    let root = root.clone();
    match tokio::task::spawn_blocking(move || {
        let captured = observation::snapshot(&root)?;
        let projected = read::read_observed(&root, &captured).ok().map(|snapshot| {
            let report = layout::run_report(&captured.plan, &snapshot, captured.cursor.clone());
            (snapshot.document, report)
        });
        Ok::<_, observation::ObservationError>((captured, projected))
    })
    .await
    {
        Ok(Ok((captured, Some((mut view, report))))) => {
            prefix_artifacts(&mut view, &state.artifact_prefix);
            let mut value = serde_json::to_value(captured).expect("serializable snapshot");
            value["view"] = serde_json::to_value(view).expect("serializable view");
            value["layout"] = serde_json::to_value(report).expect("serializable report");
            observed_json(value)
        }
        Ok(Err(error)) => observed_error(error),
        _ => observation_failure("unavailable", "The selected run cannot be projected."),
    }
}

/// Cells and member order for the view (spec/layout.md §6.11). A run's report reflects one
/// captured record prefix and carries its cursor; no `ETag`, since the body changes as the run
/// records more.
async fn layout_report(State(state): State<Arc<AppState>>) -> Response {
    let root = match &state.source {
        Source::Plan(graph) => {
            let report = serde_json::to_value(layout::plan_report(graph)).expect("serializable");
            return observed_json(report);
        }
        Source::Run(root) => root.clone(),
    };
    match tokio::task::spawn_blocking(move || {
        let captured = observation::snapshot(&root)?;
        let report = read::project_records(&root, &captured)
            .ok()
            .map(|snapshot| layout::run_report(&captured.plan, &snapshot, captured.cursor));
        Ok::<_, observation::ObservationError>(report)
    })
    .await
    {
        Ok(Ok(Some(report))) => {
            observed_json(serde_json::to_value(report).expect("serializable report"))
        }
        Ok(Err(error)) => observed_error(error),
        _ => observation_failure("unavailable", "The selected run cannot be projected."),
    }
}

#[derive(Deserialize)]
struct EventQuery {
    after: Option<String>,
    limit: Option<String>,
}

async fn observation_events(
    State(state): State<Arc<AppState>>,
    query: Result<Query<EventQuery>, QueryRejection>,
) -> Response {
    let Source::Run(root) = &state.source else {
        return observation_failure("unavailable", "A static plan has no recorded run.");
    };
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return observation_failure(
                "invalid_request",
                "Observation query parameters are malformed.",
            );
        }
    };
    let limit = match query.limit.as_deref() {
        None => observation::DEFAULT_LIMIT,
        Some(value) => match value.parse::<usize>() {
            Ok(limit) => limit,
            Err(_) => {
                return observation_failure(
                    "invalid_limit",
                    "Limit must be an integer from 1 to 1024.",
                );
            }
        },
    };
    let root = root.clone();
    match tokio::task::spawn_blocking(move || {
        observation::batch(&root, query.after.as_deref(), limit)
    })
    .await
    {
        Ok(Ok(batch)) => observed_json(serde_json::to_value(batch).expect("serializable batch")),
        Ok(Err(error)) => observed_error(error),
        _ => observation_failure("unavailable", "The selected run cannot be read."),
    }
}

fn observed_json(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn observation_failure(code: &str, message: &str) -> Response {
    let status = match code {
        "invalid_cursor" | "run_changed" => StatusCode::CONFLICT,
        "invalid_limit" | "unsupported_version" | "invalid_request" => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    let mut response = observed_json(serde_json::json!({
        "kind": "fx-run-observation-error-v1", "code": code, "message": message
    }));
    *response.status_mut() = status;
    response
}

fn observed_error(error: observation::ObservationError) -> Response {
    let value = serde_json::to_value(error).expect("serializable observation error");
    observation_failure(
        value["code"].as_str().unwrap_or("unavailable"),
        value["message"]
            .as_str()
            .unwrap_or("The selected run cannot be read."),
    )
}

async fn api_missing() -> Response {
    failure(StatusCode::NOT_FOUND, "Unknown viewer API route.")
}

async fn artifact(
    State(state): State<Arc<AppState>>,
    RoutePath(digest): RoutePath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let Source::Run(root) = &state.source else {
        return failure(
            StatusCode::NOT_FOUND,
            "A static plan does not serve artifact bytes.",
        );
    };
    if !is_digest(&digest) {
        return failure(
            StatusCode::NOT_FOUND,
            "Artifact is not recorded in this run.",
        );
    }
    let root = root.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let snapshot = read::read_inventory(&root)?;
        snapshot.open_artifact(&root, &digest)
    })
    .await;
    let (file, artifact) = match opened {
        Ok(Ok(opened)) => opened,
        _ => {
            return failure(
                StatusCode::NOT_FOUND,
                "Recorded artifact bytes are unavailable.",
            );
        }
    };
    let etag = format!("\"{}\"", artifact.digest);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag.as_str())
    {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, etag)
            .body(Body::empty())
            .expect("static response headers");
    }
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let range = match range_header {
        Some(value) => match byte_range(value, artifact.size) {
            Some(range) => Some(range),
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{}", artifact.size))
                    .body(Body::empty())
                    .expect("numeric response header");
            }
        },
        None => None,
    };
    let (offset, length) =
        range.map_or((0, artifact.size), |(start, end)| (start, end - start + 1));
    let (mime, inline) = artifact_mime(&artifact.kind);
    let mut response = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, length)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, etag)
        .header(
            header::CACHE_CONTROL,
            "private, max-age=31536000, immutable",
        )
        .header(
            header::CONTENT_SECURITY_POLICY,
            "sandbox; default-src 'none'",
        )
        .header(
            header::CONTENT_DISPOSITION,
            if inline { "inline" } else { "attachment" },
        );
    if let Some((start, end)) = range {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{}", artifact.size),
        );
    }
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        let mut file = tokio::fs::File::from_std(file);
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
            return failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Artifact could not be read.",
            );
        }
        Body::from_stream(ReaderStream::new(file.take(length)))
    };
    response
        .body(body)
        .expect("validated media and numeric response headers")
}

/// Only inert formats receive an inline media type; recorded HTML and SVG are downloads.
fn artifact_mime(kind: &str) -> (&'static str, bool) {
    let mime = match kind {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/webp" => "image/webp",
        "image/gif" => "image/gif",
        "image/avif" => "image/avif",
        "audio/wav" => "audio/wav",
        "audio/mpeg" => "audio/mpeg",
        "audio/ogg" => "audio/ogg",
        "audio/flac" => "audio/flac",
        "video/mp4" => "video/mp4",
        "video/webm" => "video/webm",
        "json" | "annotations" | "application/json" => "application/json",
        "text/plain" | "text/yaml" | "text/toml" => "text/plain; charset=utf-8",
        _ => return ("application/octet-stream", false),
    };
    (mime, true)
}

fn byte_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if size == 0 || end.contains(',') {
        return None;
    }
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        (suffix > 0).then(|| (size.saturating_sub(suffix), size - 1))
    } else {
        let start: u64 = start.parse().ok()?;
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>().ok()?.min(size - 1)
        };
        (start <= end && start < size).then_some((start, end))
    }
}

async fn asset(request: Request) -> Response {
    let path = request.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(asset) = Assets::get(path) else {
        return failure(StatusCode::NOT_FOUND, "Viewer asset not found.");
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(asset.data.into_owned()))
        .expect("static viewer response headers")
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

#[cfg(test)]
mod tests;
