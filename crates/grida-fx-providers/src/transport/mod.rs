//! The injected transport: one HTTP exchange per [`Transport::send`] (spec/providers.md §2).
//!
//! Adapters are thin clients over this trait (docs/wg/overview.md decision 9): they build an
//! [`HttpRequest`], hand it to the transport once, and classify what comes back. The transport
//! owns nothing else:
//!
//! - it **never retries** and **never follows a redirect** (a 3xx is returned as a status);
//! - it enforces the request's `timeout` (the whole exchange, connect to last body byte) and
//!   `max_response_bytes` (counted on the bytes it reads, aborting as soon as the cap is crossed);
//! - it attaches the request's [`Credential`] as one header, and refuses a credential on the
//!   [`Lane::Download`] lane before anything leaves (a result URL is another host);
//! - it reports a failure with its [`Phase`]: [`Phase::NotSent`] only when the request provably
//!   never left (no connection was established: DNS, TCP connect, TLS handshake, connect or pool
//!   timeout, or a refusal by the transport itself); every other failure is [`Phase::AfterSend`].
//!   A transport that cannot tell reports `AfterSend`.
//!
//! Implementations: [`http::HttpTransport`] (reqwest over rustls; constructed only for a live run,
//! never in a test), and for tests [`replay::ReplayTransport`] (synthetic exchange fixtures, each
//! request asserted) and [`replay::NoNetwork`] (panics on any send), behind feature `testing`.
//!
//! Credentials never appear in an error's `reason`, in `Debug` output or in a recorded fixture:
//! [`Secret`] prints as `[redacted]`.

pub mod http;
pub mod multipart;
#[cfg(any(test, feature = "testing"))]
pub mod replay;

use crate::BoxFuture;
use crate::keys::Secret;
use serde_json::Value;
use std::fmt;
use std::time::Duration;

/// One HTTP exchange's transport. Dyn-compatible; shared as `Arc<dyn Transport>`.
pub trait Transport: Send + Sync {
    /// Makes the exchange once. Never retries, never follows a redirect.
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, TransportError>>;
}

/// The method of a request. Adapters use only these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// What an exchange is for (spec/providers.md §2). It decides whether a credential may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// A request to the provider's API: may carry the provider's credential.
    Provider,
    /// An upload to the provider's API (multipart files): may carry the credential.
    Upload,
    /// A result file at a URL the provider returned: never a credential, no client defaults.
    Download,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Provider => "provider",
            Lane::Upload => "upload",
            Lane::Download => "download",
        }
    }
}

/// A credential the transport attaches as the header `header: <prefix><secret>`.
#[derive(Clone, PartialEq)]
pub struct Credential {
    /// The header name, lowercase (`authorization`, `xi-api-key`).
    pub header: &'static str,
    /// What precedes the secret (`Bearer `, `Key `, or empty).
    pub prefix: &'static str,
    pub secret: Secret,
}

impl Credential {
    /// The header value. Only a transport calls this, to put it on the wire.
    pub fn header_value(&self) -> String {
        format!("{}{}", self.prefix, self.secret.expose())
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential({}: {}[redacted])", self.header, self.prefix)
    }
}

/// A request body.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Empty,
    /// Sent as compact UTF-8 JSON (`serde_json::to_vec`, keys in order) with
    /// `content-type: application/json`.
    Json(Value),
    /// Raw bytes with their content type.
    Bytes {
        content_type: String,
        bytes: Vec<u8>,
    },
    /// `multipart/form-data`, encoded by [`multipart::encode`].
    Multipart(Vec<Part>),
}

/// One part of a multipart body.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// The form field name (`image[]`, `mask`, `file`, `model`).
    pub name: String,
    /// A file part's file name; `None` for a text field.
    pub filename: Option<String>,
    /// A file part's content type; `None` for a text field.
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

impl Part {
    /// A text field.
    pub fn text(name: &str, value: &str) -> Part {
        Part {
            name: name.to_string(),
            filename: None,
            content_type: None,
            data: value.as_bytes().to_vec(),
        }
    }

    /// A file part.
    pub fn file(name: &str, filename: &str, content_type: &str, data: Vec<u8>) -> Part {
        Part {
            name: name.to_string(),
            filename: Some(filename.to_string()),
            content_type: Some(content_type.to_string()),
            data,
        }
    }
}

/// One request (module doc).
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: Method,
    /// The absolute URL. May be a signed download URL: never put it in a reason or a record.
    pub url: String,
    /// Headers other than the credential, names lowercase. The transport adds only
    /// `content-type` (from the body), `content-length`, `host`, `accept-encoding: identity`,
    /// `accept: */*` unless the request sets `accept`, and `user-agent` off the download lane.
    pub headers: Vec<(String, String)>,
    pub credential: Option<Credential>,
    pub body: Body,
    /// The whole exchange's deadline.
    pub timeout: Duration,
    /// The most response body bytes read; more is [`TransportErrorKind::TooLarge`].
    pub max_response_bytes: u64,
    pub lane: Lane,
}

impl HttpRequest {
    /// A request with no headers, no credential and no body, a 300 s timeout and a 64 MiB cap.
    pub fn new(method: Method, url: impl Into<String>, lane: Lane) -> HttpRequest {
        HttpRequest {
            method,
            url: url.into(),
            headers: Vec::new(),
            credential: None,
            body: Body::Empty,
            timeout: Duration::from_secs(300),
            max_response_bytes: 64 * 1024 * 1024,
            lane,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> HttpRequest {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn credential(mut self, credential: Credential) -> HttpRequest {
        self.credential = Some(credential);
        self
    }

    pub fn body(mut self, body: Body) -> HttpRequest {
        self.body = body;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> HttpRequest {
        self.timeout = timeout;
        self
    }

    pub fn max_response_bytes(mut self, cap: u64) -> HttpRequest {
        self.max_response_bytes = cap;
        self
    }

    /// The URL's scheme and host, for messages that must not show a path or a query.
    pub fn origin(&self) -> String {
        match url::Url::parse(&self.url) {
            Ok(url) => format!(
                "{}://{}",
                url.scheme(),
                url.host_str().unwrap_or("(no host)")
            ),
            Err(_) => "(an unparseable URL)".to_string(),
        }
    }
}

/// A response: status, headers (names lowercase, in order), and the whole body as read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// A JSON response with `content-type: application/json`.
    pub fn json(status: u16, value: &Value) -> HttpResponse {
        HttpResponse::new(status, serde_json::to_vec(value).unwrap_or_default())
            .with_header("content-type", "application/json")
    }

    pub fn with_header(mut self, name: &str, value: &str) -> HttpResponse {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    /// The first header of this name, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 200 to 299.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// When a failure happened (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The request provably never left: the provider cannot have received it.
    NotSent,
    /// The request may have been received.
    AfterSend,
}

/// What kind of failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportErrorKind {
    /// No connection: DNS, TCP connect, TLS handshake (always `NotSent`).
    Connect,
    /// The request's deadline passed (`NotSent` only while connecting).
    Timeout,
    /// The response body passed `max_response_bytes` (always `AfterSend`).
    TooLarge,
    /// The transport refused the request before sending it, such as a credential on the download
    /// lane or a URL it cannot parse (always `NotSent`).
    Refused,
    /// Anything else: a reset, a truncated body, a protocol error.
    Other,
}

/// A failed exchange. `reason` is a sentence that never holds a credential, a URL's path or query,
/// or a response body.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportError {
    pub phase: Phase,
    pub kind: TransportErrorKind,
    pub reason: String,
}

impl TransportError {
    pub fn not_sent(kind: TransportErrorKind, reason: impl Into<String>) -> TransportError {
        TransportError {
            phase: Phase::NotSent,
            kind,
            reason: reason.into(),
        }
    }

    pub fn after_send(kind: TransportErrorKind, reason: impl Into<String>) -> TransportError {
        TransportError {
            phase: Phase::AfterSend,
            kind,
            reason: reason.into(),
        }
    }

    pub fn too_large(cap: u64) -> TransportError {
        TransportError::after_send(
            TransportErrorKind::TooLarge,
            format!("the response is larger than {cap} bytes"),
        )
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

/// The variable that turns the network off for a live run (spec/providers.md §2):
/// `GRIDA_FX_NETWORK=off` makes `--live` build its adapters over [`Offline`].
pub const NETWORK_VARIABLE: &str = "GRIDA_FX_NETWORK";

/// A transport with the network off: every exchange is refused before it leaves
/// ([`Phase::NotSent`], [`TransportErrorKind::Refused`]), so adapters answer `Refused` at $0.
#[derive(Debug, Default, Clone, Copy)]
pub struct Offline;

impl Transport for Offline {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, TransportError>> {
        Box::pin(async move {
            check_request(&request)?;
            Err(TransportError::not_sent(
                TransportErrorKind::Refused,
                format!("the network is off ({NETWORK_VARIABLE}=off)"),
            ))
        })
    }
}

/// The checks every transport makes before anything leaves (spec/providers.md §2): an absolute
/// `http`/`https` URL with a host and no userinfo, and no credential on the download lane.
pub fn check_request(request: &HttpRequest) -> Result<(), TransportError> {
    let refused = |reason: &str| TransportError::not_sent(TransportErrorKind::Refused, reason);
    let url =
        url::Url::parse(&request.url).map_err(|_| refused("the request's URL does not parse"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(refused("a request goes to an http(s) URL with a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refused("a request's URL carries no credentials"));
    }
    if request.lane == Lane::Download && request.credential.is_some() {
        return Err(refused("a download never carries a credential"));
    }
    if request.headers.iter().any(|(name, _)| {
        request
            .credential
            .as_ref()
            .is_some_and(|c| c.header == name)
    }) {
        return Err(refused(
            "a credential header is set only through the credential",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> Secret {
        Secret::new("sk-test-1")
    }

    #[test]
    fn credentials_never_print() {
        let credential = Credential {
            header: "authorization",
            prefix: "Bearer ",
            secret: secret(),
        };
        let shown = format!("{credential:?}");
        assert!(!shown.contains("sk-test-1"), "{shown}");
        assert_eq!(credential.header_value(), "Bearer sk-test-1");
        let request = HttpRequest::new(
            Method::Post,
            "https://api.example.test/v1/x",
            Lane::Provider,
        )
        .credential(credential);
        assert!(!format!("{request:?}").contains("sk-test-1"));
    }

    #[test]
    fn a_download_never_carries_a_credential() {
        let request = HttpRequest::new(
            Method::Get,
            "https://files.example.test/a.png",
            Lane::Download,
        )
        .credential(Credential {
            header: "authorization",
            prefix: "Key ",
            secret: secret(),
        });
        let error = check_request(&request).unwrap_err();
        assert_eq!(error.phase, Phase::NotSent);
        assert_eq!(error.kind, TransportErrorKind::Refused);
        assert!(
            check_request(&HttpRequest::new(
                Method::Get,
                "https://files.example.test/a.png",
                Lane::Download
            ))
            .is_ok()
        );
    }

    #[test]
    fn urls_are_absolute_http_without_userinfo() {
        for url in [
            "not a url",
            "ftp://x.test/a",
            "https://user:pw@x.test/a",
            "file:///etc/x",
        ] {
            let request = HttpRequest::new(Method::Get, url, Lane::Provider);
            assert!(check_request(&request).is_err(), "{url}");
        }
    }

    #[test]
    fn the_origin_hides_path_and_query() {
        let request = HttpRequest::new(
            Method::Get,
            "https://cdn.example.test/out/a.png?sig=secret",
            Lane::Download,
        );
        assert_eq!(request.origin(), "https://cdn.example.test");
    }

    #[test]
    fn response_headers_are_case_insensitive() {
        let response = HttpResponse::new(200, b"x".to_vec()).with_header("X-Request-Id", "r1");
        assert_eq!(response.header("x-request-id"), Some("r1"));
        assert!(response.is_success());
        assert!(!HttpResponse::new(302, Vec::new()).is_success());
    }
}
