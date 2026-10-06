//! The default transport: reqwest over rustls (spec/providers.md §2). It is constructed only when a
//! run is live (`grida-fx run --live`, through [`crate::live::live_setup`]); no test ever
//! constructs it, and nothing else in the engine reaches the network.
//!
//! Contract (the module doc of [`super`] holds the rules every transport keeps):
//! - `reqwest::Client`s for two kinds of lane: the provider and upload lanes, whose default
//!   headers are `user-agent` ([`HttpConfig::user_agent`]) and reqwest's fixed `accept: */*`
//!   (which a request setting `accept` replaces), and the download lane with only that
//!   `accept: */*`; none has a cookie store (reqwest is built without one), none keeps an idle
//!   connection (each exchange opens its own, so nothing is ever resent on a stale one);
//! - **proxies** (spec/providers.md §2 item 9): each lane has a client that uses the proxies the
//!   environment names (`HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`, as reqwest reads them) and a
//!   direct one that uses none. A request goes direct when its URL is plain `http` or its host is
//!   a loopback host (the private `direct`): a proxy would read a plain request, credential
//!   included, in clear text, and through a proxy a loopback host would be the proxy's own
//!   machine, not this one. Every other request is `https` to a remote host: through a proxy it
//!   is tunnelled (`CONNECT`), so the proxy sees the host and port, and the request stays
//!   encrypted end to end;
//! - `redirect::Policy::none()`, no retries (reqwest's own retry of protocol NACKs is turned
//!   off), `accept-encoding: identity` on every request (the adapter reads the entity as sent), a
//!   connect timeout ([`HttpConfig::connect_timeout`], 30 s by default) inside the request's
//!   whole-exchange `timeout`;
//! - the headers: the request's own, in order, then `accept-encoding: identity`, then the
//!   credential's header (marked sensitive) from [`super::Credential::header_value`]; a
//!   `content-type` the body decides replaces one the request set, except that a
//!   [`Body::Json`] keeps a `content-type` the request set;
//! - the body: [`Body::Json`] as `serde_json::to_vec` with `content-type: application/json`;
//!   [`Body::Multipart`] through [`super::multipart::encode`] with a boundary from
//!   [`super::multipart::boundary`]; [`Body::Bytes`] as given with its content type;
//!   [`Body::Empty`] sends none;
//! - the response read chunk by chunk, failing with [`TransportError::too_large`] as soon as more
//!   than `max_response_bytes` arrived (a declared length above the cap fails before reading);
//!   header names lowercase, values that are not UTF-8 left out;
//! - classification: a request reqwest cannot build is `NotSent` and
//!   [`TransportErrorKind::Refused`]; an error before a connection was established
//!   (`is_connect()`: DNS, TCP connect, TLS handshake, a connect timeout) is `NotSent`;
//!   every other error, a timeout after the connection included, is `AfterSend`;
//! - reasons are fixed sentences naming the URL's origin at most ([`HttpRequest::origin`]), never
//!   its path or query, never a header, never reqwest's own message (which holds the full URL).

use super::multipart;
use super::{
    Body, HttpRequest, HttpResponse, Lane, Method, Transport, TransportError, TransportErrorKind,
};
use crate::BoxFuture;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;

/// How the default transport is built.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    /// `grida-fx/<version>`.
    pub user_agent: String,
}

impl Default for HttpConfig {
    fn default() -> HttpConfig {
        HttpConfig {
            connect_timeout: Duration::from_secs(30),
            user_agent: format!("grida-fx/{}", grida_fx_core::ENGINE_VERSION),
        }
    }
}

/// The default transport (module doc).
#[derive(Debug)]
pub struct HttpTransport {
    config: HttpConfig,
    clients: Clients,
}

impl HttpTransport {
    /// Builds the clients. Fails only when TLS cannot be set up.
    pub fn new(config: HttpConfig) -> Result<HttpTransport, String> {
        let clients = Clients::new(&config)?;
        Ok(HttpTransport { config, clients })
    }

    pub fn config(&self) -> &HttpConfig {
        &self.config
    }
}

impl Transport for HttpTransport {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, TransportError>> {
        Box::pin(exchange(&self.clients, request))
    }
}

/// The clients of the module doc: per kind of lane, one through the environment's proxies and
/// one direct.
#[derive(Debug)]
struct Clients {
    /// The provider and upload lanes.
    api: reqwest::Client,
    api_direct: reqwest::Client,
    /// The download lane.
    download: reqwest::Client,
    download_direct: reqwest::Client,
}

impl Clients {
    fn new(config: &HttpConfig) -> Result<Clients, String> {
        let user_agent = HeaderValue::from_str(&config.user_agent)
            .map_err(|_| "the transport's user-agent is not a valid header value".to_string())?;
        let build = |builder: reqwest::ClientBuilder| {
            builder
                .build()
                .map_err(|_| "the HTTP client could not be set up (TLS)".to_string())
        };
        Ok(Clients {
            api: build(client_builder(config).user_agent(user_agent.clone()))?,
            api_direct: build(client_builder(config).user_agent(user_agent).no_proxy())?,
            download: build(client_builder(config))?,
            download_direct: build(client_builder(config).no_proxy())?,
        })
    }

    /// The client a request to `url` on `lane` goes through (module doc).
    fn of(&self, lane: Lane, url: &reqwest::Url) -> &reqwest::Client {
        match (lane, direct(url)) {
            (Lane::Provider | Lane::Upload, false) => &self.api,
            (Lane::Provider | Lane::Upload, true) => &self.api_direct,
            (Lane::Download, false) => &self.download,
            (Lane::Download, true) => &self.download_direct,
        }
    }
}

/// Whether a request to `url` goes straight to its host, never through a proxy: a URL that is not
/// `https`, or a loopback host (`localhost`, a loopback IP) (module doc).
fn direct(url: &reqwest::Url) -> bool {
    url.scheme() != "https" || crate::wire::is_loopback(url)
}

/// The exchange (module doc).
async fn exchange(clients: &Clients, request: HttpRequest) -> Result<HttpResponse, TransportError> {
    super::check_request(&request)?;
    let prepared = prepare(&request)?;
    let origin = request.origin();
    let url = reqwest::Url::parse(&request.url).map_err(|_| {
        TransportError::not_sent(
            TransportErrorKind::Refused,
            "the request's URL does not parse",
        )
    })?;
    let method = match request.method {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
    };
    let client = clients.of(request.lane, &url);
    let mut builder = client
        .request(method, url)
        .headers(prepared.headers)
        .timeout(request.timeout);
    if let Some(body) = prepared.body {
        builder = builder.body(body);
    }
    let mut response = builder.send().await.map_err(|error| {
        send_error(
            Failure {
                builder: error.is_builder(),
                connect: error.is_connect(),
                timeout: error.is_timeout(),
            },
            &origin,
            request.timeout,
        )
    })?;
    check_declared_length(response.content_length(), request.max_response_bytes)?;
    let status = response.status().as_u16();
    let headers = response_headers(response.headers());
    let body = read_capped(
        &mut response,
        request.max_response_bytes,
        &origin,
        request.timeout,
    )
    .await?;
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// What every client shares (module doc).
fn client_builder(config: &HttpConfig) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .use_rustls_tls()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .connect_timeout(config.connect_timeout)
        .pool_max_idle_per_host(0)
}

/// A request's headers and body bytes, as they go on the wire.
#[derive(Debug)]
struct Prepared {
    headers: HeaderMap,
    body: Option<Vec<u8>>,
}

/// The final headers and body of `request` (module doc). A header that is not a valid HTTP
/// header is refused before anything leaves, naming the header but never its value.
fn prepare(request: &HttpRequest) -> Result<Prepared, TransportError> {
    let (content_type, body, keep_own_type) = match &request.body {
        Body::Empty => (None, None, true),
        Body::Json(value) => (
            Some("application/json".to_string()),
            Some(serde_json::to_vec(value).map_err(|_| {
                TransportError::not_sent(
                    TransportErrorKind::Refused,
                    "the request's JSON body cannot be written",
                )
            })?),
            true,
        ),
        Body::Bytes {
            content_type,
            bytes,
        } => (Some(content_type.clone()), Some(bytes.clone()), false),
        Body::Multipart(parts) => {
            let boundary = multipart::boundary(parts);
            (
                Some(multipart::content_type(&boundary)),
                Some(multipart::encode(parts, &boundary)),
                false,
            )
        }
    };
    let own_type = request
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    let mut headers = HeaderMap::new();
    for (name, value) in &request.headers {
        let is_type = name.eq_ignore_ascii_case("content-type");
        if name.eq_ignore_ascii_case("accept-encoding") || (is_type && !keep_own_type) {
            continue;
        }
        headers.append(header_name(name)?, header_value(name, value)?);
    }
    if let Some(content_type) = content_type
        && !(own_type && keep_own_type)
    {
        headers.append(CONTENT_TYPE, header_value("content-type", &content_type)?);
    }
    headers.append(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    if let Some(credential) = &request.credential {
        let mut value = HeaderValue::from_str(&credential.header_value()).map_err(|_| {
            TransportError::not_sent(
                TransportErrorKind::Refused,
                format!(
                    "the credential for the {} header is not a valid header value",
                    credential.header
                ),
            )
        })?;
        value.set_sensitive(true);
        headers.append(header_name(credential.header)?, value);
    }
    Ok(Prepared { headers, body })
}

fn header_name(name: &str) -> Result<HeaderName, TransportError> {
    HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
        TransportError::not_sent(
            TransportErrorKind::Refused,
            "a request header has a name that is not a valid header name",
        )
    })
}

fn header_value(name: &str, value: &str) -> Result<HeaderValue, TransportError> {
    HeaderValue::from_str(value).map_err(|_| {
        let reason = if name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            format!(
                "the {} header has a value that is not a valid header value",
                name.to_ascii_lowercase()
            )
        } else {
            "a request header has a value that is not a valid header value".to_string()
        };
        TransportError::not_sent(TransportErrorKind::Refused, reason)
    })
}

/// A declared body length above the cap fails before reading.
fn check_declared_length(declared: Option<u64>, cap: u64) -> Result<(), TransportError> {
    match declared {
        Some(length) if length > cap => Err(TransportError::too_large(cap)),
        _ => Ok(()),
    }
}

/// Response headers: names lowercase, in order; values that are not UTF-8 left out.
fn response_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let value = std::str::from_utf8(value.as_bytes()).ok()?;
            Some((name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

/// What a reqwest error says about itself, the only parts the classification reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Failure {
    /// The request could not be built.
    builder: bool,
    /// No connection was established.
    connect: bool,
    timeout: bool,
}

/// How a failed send is reported (module doc).
fn send_error(failure: Failure, origin: &str, timeout: Duration) -> TransportError {
    if failure.builder {
        return TransportError::not_sent(
            TransportErrorKind::Refused,
            format!("the request to {origin} could not be built"),
        );
    }
    if failure.connect {
        let kind = if failure.timeout {
            TransportErrorKind::Timeout
        } else {
            TransportErrorKind::Connect
        };
        return TransportError::not_sent(kind, format!("could not connect to {origin}"));
    }
    if failure.timeout {
        return timed_out(origin, timeout);
    }
    TransportError::after_send(
        TransportErrorKind::Other,
        format!("the connection to {origin} broke"),
    )
}

/// How a failed read of the response body is reported: always after the send.
fn read_error(failure: Failure, origin: &str, timeout: Duration) -> TransportError {
    if failure.timeout {
        return timed_out(origin, timeout);
    }
    TransportError::after_send(
        TransportErrorKind::Other,
        format!("the response from {origin} could not be read"),
    )
}

fn timed_out(origin: &str, timeout: Duration) -> TransportError {
    TransportError::after_send(
        TransportErrorKind::Timeout,
        format!(
            "the request to {origin} timed out after {} s",
            timeout.as_secs_f64()
        ),
    )
}

/// A response body read chunk by chunk.
trait Chunks {
    /// The next chunk; `None` at the end of the body.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, Failure>;
}

impl Chunks for reqwest::Response {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, Failure> {
        match self.chunk().await {
            Ok(chunk) => Ok(chunk.map(|bytes| bytes.to_vec())),
            Err(error) => Err(Failure {
                builder: false,
                connect: false,
                timeout: error.is_timeout(),
            }),
        }
    }
}

/// Reads every chunk, failing as soon as more than `cap` bytes arrived.
async fn read_capped(
    source: &mut impl Chunks,
    cap: u64,
    origin: &str,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    let mut body = Vec::new();
    loop {
        match source.next_chunk().await {
            Ok(Some(chunk)) => {
                if body.len() as u64 + chunk.len() as u64 > cap {
                    return Err(TransportError::too_large(cap));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(failure) => return Err(read_error(failure, origin, timeout)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Secret;
    use crate::transport::{Credential, Part, Phase};
    use serde_json::json;

    fn post(body: Body) -> HttpRequest {
        HttpRequest::new(
            Method::Post,
            "https://api.example.test/v1/x",
            Lane::Provider,
        )
        .body(body)
    }

    fn credential() -> Credential {
        Credential {
            header: "authorization",
            prefix: "Bearer ",
            secret: Secret::new("sk-test-transport-1"),
        }
    }

    /// The headers as text, in order (the credential's value included: tests only).
    fn listed(headers: &HeaderMap) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_string(),
                    v.to_str().expect("ascii").to_string(),
                )
            })
            .collect()
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_json_body_is_compact_in_member_order_with_its_content_type() {
        let request = post(Body::Json(json!({"z": 1, "a": "é", "m": [1.0, null]})))
            .header("X-Trace", "t1")
            .credential(credential());
        let prepared = prepare(&request).expect("prepared");
        assert_eq!(
            prepared.body.as_deref(),
            Some(r#"{"z":1,"a":"é","m":[1.0,null]}"#.as_bytes())
        );
        assert_eq!(
            listed(&prepared.headers),
            pairs(&[
                ("x-trace", "t1"),
                ("content-type", "application/json"),
                ("accept-encoding", "identity"),
                ("authorization", "Bearer sk-test-transport-1"),
            ])
        );
        let sensitive = prepared
            .headers
            .get("authorization")
            .expect("credential header");
        assert!(sensitive.is_sensitive());
        assert!(!format!("{:?}", prepared.headers).contains("sk-test-transport-1"));
    }

    #[test]
    fn a_json_body_keeps_a_content_type_the_request_set() {
        let request =
            post(Body::Json(json!({}))).header("content-type", "application/json; charset=utf-8");
        let prepared = prepare(&request).expect("prepared");
        assert_eq!(
            listed(&prepared.headers),
            pairs(&[
                ("content-type", "application/json; charset=utf-8"),
                ("accept-encoding", "identity"),
            ])
        );
        assert_eq!(prepared.body.as_deref(), Some(&b"{}"[..]));
    }

    #[test]
    fn a_bytes_body_goes_as_given_with_its_own_content_type() {
        let request = post(Body::Bytes {
            content_type: "audio/mpeg".into(),
            bytes: vec![0xff, 0xfb, 0x90],
        })
        .header("content-type", "text/plain")
        .header("accept-encoding", "gzip")
        .header("accept", "audio/mpeg");
        let prepared = prepare(&request).expect("prepared");
        assert_eq!(prepared.body, Some(vec![0xff, 0xfb, 0x90]));
        assert_eq!(
            listed(&prepared.headers),
            pairs(&[
                ("accept", "audio/mpeg"),
                ("content-type", "audio/mpeg"),
                ("accept-encoding", "identity"),
            ])
        );
    }

    #[test]
    fn a_multipart_body_is_encoded_with_its_boundary() {
        let parts = vec![
            Part::text("model", "img-a"),
            Part::file("image[]", "a.png", "image/png", vec![1, 2, 3]),
        ];
        let boundary = multipart::boundary(&parts);
        let request = post(Body::Multipart(parts.clone())).credential(credential());
        let prepared = prepare(&request).expect("prepared");
        assert_eq!(prepared.body, Some(multipart::encode(&parts, &boundary)));
        let content_type = format!("multipart/form-data; boundary={boundary}");
        assert_eq!(
            listed(&prepared.headers),
            pairs(&[
                ("content-type", content_type.as_str()),
                ("accept-encoding", "identity"),
                ("authorization", "Bearer sk-test-transport-1"),
            ])
        );
    }

    #[test]
    fn an_empty_body_sends_no_body_and_no_content_type() {
        let request = HttpRequest::new(
            Method::Get,
            "https://cdn.example.test/a.png?sig=1",
            Lane::Download,
        );
        let prepared = prepare(&request).expect("prepared");
        assert_eq!(prepared.body, None);
        assert_eq!(
            listed(&prepared.headers),
            pairs(&[("accept-encoding", "identity")])
        );
    }

    #[test]
    fn invalid_headers_are_refused_without_their_values() {
        let bad_value = post(Body::Empty).header("x-note", "line\nbreak secret-123");
        let error = prepare(&bad_value).unwrap_err();
        assert_eq!(error.phase, Phase::NotSent);
        assert_eq!(error.kind, TransportErrorKind::Refused);
        assert_eq!(
            error.reason,
            "the x-note header has a value that is not a valid header value"
        );
        let bad_name = post(Body::Empty).header("bad name", "v");
        let error = prepare(&bad_name).unwrap_err();
        assert_eq!(error.kind, TransportErrorKind::Refused);
        assert!(!error.reason.contains("bad name"), "{}", error.reason);
        let bad_key = post(Body::Empty).credential(Credential {
            header: "xi-api-key",
            prefix: "",
            secret: Secret::new("key\r\nx-evil: 1"),
        });
        let error = prepare(&bad_key).unwrap_err();
        assert_eq!(error.phase, Phase::NotSent);
        assert_eq!(
            error.reason,
            "the credential for the xi-api-key header is not a valid header value"
        );
        assert!(!error.reason.contains("evil"));
    }

    #[test]
    fn a_declared_length_above_the_cap_fails_before_reading() {
        assert!(check_declared_length(None, 10).is_ok());
        assert!(check_declared_length(Some(10), 10).is_ok());
        let error = check_declared_length(Some(11), 10).unwrap_err();
        assert_eq!(error, TransportError::too_large(10));
        assert_eq!(error.phase, Phase::AfterSend);
        assert_eq!(error.kind, TransportErrorKind::TooLarge);
    }

    /// Scripted chunks, counting how many were read.
    struct Script {
        chunks: std::collections::VecDeque<Result<Option<Vec<u8>>, Failure>>,
        read: usize,
    }

    impl Script {
        fn new(chunks: Vec<Result<Option<Vec<u8>>, Failure>>) -> Script {
            Script {
                chunks: chunks.into(),
                read: 0,
            }
        }
    }

    impl Chunks for Script {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, Failure> {
            self.read += 1;
            self.chunks.pop_front().unwrap_or(Ok(None))
        }
    }

    fn read(script: &mut Script, cap: u64) -> Result<Vec<u8>, TransportError> {
        crate::testing::block_on(read_capped(
            script,
            cap,
            "https://cdn.example.test",
            Duration::from_secs(120),
        ))
    }

    #[test]
    fn a_body_is_read_whole_up_to_the_cap() {
        let mut script = Script::new(vec![Ok(Some(vec![1, 2])), Ok(Some(vec![3])), Ok(None)]);
        assert_eq!(read(&mut script, 3), Ok(vec![1, 2, 3]));
        let mut empty = Script::new(vec![Ok(None)]);
        assert_eq!(read(&mut empty, 0), Ok(Vec::new()));
    }

    #[test]
    fn reading_stops_as_soon_as_the_cap_is_crossed() {
        let mut script = Script::new(vec![
            Ok(Some(vec![1, 2])),
            Ok(Some(vec![3, 4])),
            Ok(Some(vec![5])),
            Ok(None),
        ]);
        assert_eq!(read(&mut script, 3), Err(TransportError::too_large(3)));
        assert_eq!(script.read, 2, "no chunk is read after the cap is crossed");
    }

    #[test]
    fn a_failed_read_is_after_the_send() {
        let mut broken = Script::new(vec![Ok(Some(vec![1])), Err(Failure::default())]);
        let error = read(&mut broken, 10).unwrap_err();
        assert_eq!(error.phase, Phase::AfterSend);
        assert_eq!(error.kind, TransportErrorKind::Other);
        assert_eq!(
            error.reason,
            "the response from https://cdn.example.test could not be read"
        );
        let mut slow = Script::new(vec![Err(Failure {
            timeout: true,
            ..Failure::default()
        })]);
        let error = read(&mut slow, 10).unwrap_err();
        assert_eq!(error.phase, Phase::AfterSend);
        assert_eq!(error.kind, TransportErrorKind::Timeout);
        assert_eq!(
            error.reason,
            "the request to https://cdn.example.test timed out after 120 s"
        );
    }

    #[test]
    fn send_failures_are_classified_by_phase() {
        let origin = "https://api.example.test";
        let timeout = Duration::from_millis(1500);
        let cases = [
            (
                Failure {
                    builder: true,
                    ..Failure::default()
                },
                Phase::NotSent,
                TransportErrorKind::Refused,
                "the request to https://api.example.test could not be built",
            ),
            (
                Failure {
                    connect: true,
                    ..Failure::default()
                },
                Phase::NotSent,
                TransportErrorKind::Connect,
                "could not connect to https://api.example.test",
            ),
            (
                Failure {
                    connect: true,
                    timeout: true,
                    ..Failure::default()
                },
                Phase::NotSent,
                TransportErrorKind::Timeout,
                "could not connect to https://api.example.test",
            ),
            (
                Failure {
                    timeout: true,
                    ..Failure::default()
                },
                Phase::AfterSend,
                TransportErrorKind::Timeout,
                "the request to https://api.example.test timed out after 1.5 s",
            ),
            (
                Failure::default(),
                Phase::AfterSend,
                TransportErrorKind::Other,
                "the connection to https://api.example.test broke",
            ),
        ];
        for (failure, phase, kind, reason) in cases {
            let error = send_error(failure, origin, timeout);
            assert_eq!(
                (error.phase, error.kind, error.reason.as_str()),
                (phase, kind, reason),
                "{failure:?}"
            );
        }
    }

    #[test]
    fn response_headers_are_lowercase_and_text_only() {
        let mut headers = HeaderMap::new();
        headers.append("X-Request-Id", HeaderValue::from_static("r1"));
        headers.append("retry-after", HeaderValue::from_static("20"));
        headers.append(
            "x-binary",
            HeaderValue::from_bytes(b"\xff\xfe").expect("opaque bytes"),
        );
        headers.append(
            "x-latin",
            HeaderValue::from_bytes("é".as_bytes()).expect("utf-8 bytes"),
        );
        assert_eq!(
            response_headers(&headers),
            pairs(&[
                ("x-request-id", "r1"),
                ("retry-after", "20"),
                ("x-latin", "é")
            ])
        );
    }

    #[test]
    fn plain_and_loopback_urls_go_direct() {
        let direct = |url: &str| direct(&reqwest::Url::parse(url).expect("a URL"));
        for url in [
            "http://localhost:8080/v1",
            "http://127.0.0.1:9/v1",
            "http://[::1]:9/v1",
            "https://localhost/v1",
            "https://LOCALHOST./v1",
            "https://127.0.0.2/v1",
            "http://api.example.test/v1",
        ] {
            assert!(direct(url), "{url}");
        }
        for url in [
            "https://api.example.test/v1",
            "https://openrouter.ai/api/v1",
            "https://v3b.fal.media/files/a.mp4?sig=1",
            "https://localhost.example.test/v1",
        ] {
            assert!(!direct(url), "{url}");
        }
    }

    /// The environment variable that tells [`proxy_policy_child`] where to send.
    const CHILD_TARGET: &str = "GRIDA_FX_TEST_PROXY_TARGET";

    /// A listener on 127.0.0.1 that answers every request it reads with `204`, and keeps what it
    /// read, until `stop` is set.
    fn spy(
        listener: std::net::TcpListener,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::thread::JoinHandle<Vec<String>> {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        listener
            .set_nonblocking(true)
            .expect("a nonblocking listener");
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).expect("a blocking stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("a read timeout");
                let mut read = Vec::new();
                let mut buffer = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    read.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&read).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if read.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
                seen.push(String::from_utf8_lossy(&read).to_string());
            }
            seen
        })
    }

    /// spec/providers.md §2 item 9, end to end on loopback only: this test runs
    /// [`proxy_policy_child`] in a process of its own whose `HTTP_PROXY` and `ALL_PROXY` name a listener here (the
    /// "proxy"), and another listener here plays the provider at a plain `http` loopback URL, as
    /// a local gateway would. The child first sends one request through a proxied client, to show
    /// the proxy variables are in force; then FX's exchange sends a credentialed request to the
    /// same URL. Only the first may reach the proxy; the credential goes straight to the
    /// provider. Nothing leaves 127.0.0.1, and the default transport itself is never built: the
    /// child builds the clients and calls the exchange.
    #[test]
    fn a_proxy_in_the_environment_never_sees_a_plain_or_loopback_request() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let provider = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        let proxy_url = format!("http://{}", proxy.local_addr().expect("an address"));
        let provider_url = format!("http://{}", provider.local_addr().expect("an address"));
        let stop = Arc::new(AtomicBool::new(false));
        let proxy_seen = spy(proxy, Arc::clone(&stop));
        let provider_seen = spy(provider, Arc::clone(&stop));
        let child = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args([
                "transport::http::tests::proxy_policy_child",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD")
            .env("HTTP_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("ALL_PROXY", &proxy_url)
            .env("all_proxy", &proxy_url)
            .env(CHILD_TARGET, &provider_url)
            .output()
            .expect("the child ran");
        stop.store(true, Ordering::SeqCst);
        let proxy_seen = proxy_seen.join().expect("the proxy listener");
        let provider_seen = provider_seen.join().expect("the provider listener");
        let stdout = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success() && stdout.contains("1 passed"),
            "the child failed: {stdout}{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(proxy_seen.len(), 1, "only the control: {proxy_seen:?}");
        assert!(
            proxy_seen[0].starts_with("GET http://127.0.0.1:"),
            "{proxy_seen:?}"
        );
        assert!(proxy_seen[0].contains("/control"), "{proxy_seen:?}");
        assert!(
            !proxy_seen[0].to_ascii_lowercase().contains("authorization"),
            "{proxy_seen:?}"
        );
        assert_eq!(provider_seen.len(), 1, "{provider_seen:?}");
        assert!(
            provider_seen[0].starts_with("POST /v1/images/generations HTTP/1.1"),
            "{provider_seen:?}"
        );
        assert!(
            provider_seen[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer test-key-not-real-proxy"),
            "{provider_seen:?}"
        );
    }

    /// The child of [`a_proxy_in_the_environment_never_sees_a_plain_or_loopback_request`]; does
    /// nothing unless [`CHILD_TARGET`] names a loopback URL.
    #[test]
    #[ignore = "run by a_proxy_in_the_environment_never_sees_a_plain_or_loopback_request"]
    fn proxy_policy_child() {
        let Ok(target) = std::env::var(CHILD_TARGET) else {
            return;
        };
        let parsed = reqwest::Url::parse(&target).expect("a URL");
        assert!(crate::wire::is_loopback(&parsed), "loopback only");
        let clients = Clients::new(&HttpConfig::default()).expect("the clients");
        crate::testing::block_on(async {
            let control = clients
                .api
                .get(format!("{target}/control"))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .expect("the proxied control request");
            assert_eq!(control.status().as_u16(), 204);
            let request = HttpRequest::new(
                Method::Post,
                format!("{target}/v1/images/generations"),
                Lane::Provider,
            )
            .credential(Credential {
                header: "authorization",
                prefix: "Bearer ",
                secret: Secret::new("test-key-not-real-proxy"),
            })
            .body(Body::Json(json!({"model": "img-a"})))
            .timeout(Duration::from_secs(10));
            let response = exchange(&clients, request).await.expect("the exchange");
            assert_eq!(response.status, 204);
        });
    }

    #[test]
    fn the_default_config_names_the_engine() {
        let config = HttpConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(30));
        assert_eq!(
            config.user_agent,
            format!("grida-fx/{}", grida_fx_core::ENGINE_VERSION)
        );
    }
}
