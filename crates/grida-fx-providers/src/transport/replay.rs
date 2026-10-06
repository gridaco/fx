//! Transports for tests (feature `testing`): [`ReplayTransport`] plays synthetic exchanges and
//! asserts every request it is given; [`NoNetwork`] panics on any send. Neither touches the
//! network. Fixtures are synthetic: made-up keys (`test-openai-key`), made-up hosts where the
//! adapter allows it, and media built by the test or by a committed script named next to it.
//!
//! **Fixture format** (`fx-exchanges-v1`, JSON; [`ReplayTransport::from_fixture`]):
//!
//! ```json
//! {"kind": "fx-exchanges-v1",
//!  "exchanges": [
//!   {"request": {"method": "POST", "url": "https://api.openai.com/v1/images/generations",
//!                "lane": "provider",
//!                "credential": "authorization: Bearer test-openai-key",
//!                "headers": {"content-type": "application/json"},
//!                "body": {"json": {"model": "img-a", "prompt": "a kite"}}},
//!    "response": {"status": 200, "headers": {"x-request-id": "r1"},
//!                 "body": {"json": {"data": [{"b64_json": "…"}]}}}},
//!   {"request": {"method": "GET", "url": "https://v3.fal.media/files/x.png", "lane": "download"},
//!    "error": {"phase": "after_send", "kind": "other", "reason": "connection reset"}}
//!  ]}
//! ```
//!
//! - `request` is what the adapter must send, in order. `method`, `url` and `lane` must match
//!   exactly. `credential` is the header the transport would attach, as `"<name>: <value>"`; when
//!   it is absent or `null` the request must carry none. `headers` lists headers that must be
//!   present with these values (names compared case-insensitively); others are allowed. `body`
//!   is `null` or absent for no body, `"any"` to skip the check, `{"json": v}` (compared as parsed
//!   JSON, member order ignored), `{"json_text": "…"}` (the exact compact encoding), `{"text":
//!   "…"}`, `{"base64": "…"}`, or `{"multipart": [{"name", "filename"?, "content_type"?,
//!   "text"? | "base64"?}]}` (compared part by part, in order).
//! - Exactly one of `response` and `error`. A response `body` is `{"json": v}`, `{"text": "…"}`,
//!   `{"base64": "…"}`, `{"zeros": n}` (n zero bytes, never allocated when n exceeds the
//!   request's cap) or `{"file": "<path relative to the fixture>"}`; absent means empty. A body
//!   longer than the request's `max_response_bytes` is answered with
//!   [`super::TransportError::too_large`], as a real transport would.
//! - `error` is a [`super::TransportError`]: `phase` `not_sent` | `after_send`, `kind` `connect` |
//!   `timeout` | `too_large` | `refused` | `other`, and a `reason`.
//!
//! A request that breaks the transport's own rules ([`super::check_request`]) is answered with that
//! refusal and consumes no exchange. A request with no exchange left, or one that differs from
//! the next expected request, **panics** with the difference: a test never passes on a request it
//! did not script. [`ReplayTransport::assert_done`] checks every exchange was used.

use super::{
    Body, HttpRequest, HttpResponse, Lane, Method, Part, Phase, Transport, TransportError,
    TransportErrorKind,
};
use crate::BoxFuture;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// The body a request must carry.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpectBody {
    /// Not checked.
    Any,
    Empty,
    /// JSON, compared as parsed values (member order ignored).
    Json(Value),
    /// JSON, compared as its exact compact encoding.
    JsonText(String),
    Text(String),
    Bytes(Vec<u8>),
    Multipart(Vec<Part>),
}

/// The request an exchange expects.
#[derive(Debug, Clone, PartialEq)]
pub struct Expect {
    pub method: Method,
    pub url: String,
    pub lane: Lane,
    /// `"<header>: <value>"`, or `None` for no credential.
    pub credential: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: ExpectBody,
}

impl Expect {
    /// A request with no credential, no required header and no body.
    pub fn new(method: Method, url: impl Into<String>, lane: Lane) -> Expect {
        Expect {
            method,
            url: url.into(),
            lane,
            credential: None,
            headers: Vec::new(),
            body: ExpectBody::Empty,
        }
    }

    /// `POST url` on the provider lane with a JSON body.
    pub fn post_json(url: impl Into<String>, body: Value) -> Expect {
        Expect::new(Method::Post, url, Lane::Provider).body(ExpectBody::Json(body))
    }

    /// `GET url` on the provider lane.
    pub fn get(url: impl Into<String>) -> Expect {
        Expect::new(Method::Get, url, Lane::Provider)
    }

    /// `GET url` on the download lane (never a credential).
    pub fn download(url: impl Into<String>) -> Expect {
        Expect::new(Method::Get, url, Lane::Download)
    }

    /// The credential header, e.g. `credential("authorization", "Bearer test-key")`.
    pub fn credential(mut self, header: &str, value: &str) -> Expect {
        self.credential = Some(format!("{}: {value}", header.to_ascii_lowercase()));
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Expect {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn body(mut self, body: ExpectBody) -> Expect {
        self.body = body;
        self
    }

    /// Answers with `response`.
    pub fn reply(self, response: HttpResponse) -> Exchange {
        Exchange {
            expect: self,
            reply: Reply::Response(response),
        }
    }

    /// Answers with a transport failure.
    pub fn fail(self, error: TransportError) -> Exchange {
        Exchange {
            expect: self,
            reply: Reply::Error(error),
        }
    }
}

/// What an exchange answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Response(HttpResponse),
    /// A response whose body is `len` zero bytes, allocated only when within the cap.
    Zeros {
        status: u16,
        headers: Vec<(String, String)>,
        len: u64,
    },
    Error(TransportError),
}

/// One scripted exchange.
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    pub expect: Expect,
    pub reply: Reply,
}

/// Plays exchanges in order, asserting each request (module doc).
#[derive(Debug, Default)]
pub struct ReplayTransport {
    script: Mutex<VecDeque<Exchange>>,
    log: Mutex<Vec<HttpRequest>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl ReplayTransport {
    pub fn new(exchanges: Vec<Exchange>) -> ReplayTransport {
        ReplayTransport {
            script: Mutex::new(exchanges.into()),
            log: Mutex::new(Vec::new()),
        }
    }

    /// Reads an `fx-exchanges-v1` fixture file. Panics when it is not one (a test bug).
    pub fn from_fixture(path: &Path) -> ReplayTransport {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));
        let base = path.parent().unwrap_or(Path::new("."));
        ReplayTransport::new(
            parse_fixture(&text, base)
                .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display())),
        )
    }

    /// Every request sent so far, in order (credentials included, for assertions).
    pub fn requests(&self) -> Vec<HttpRequest> {
        lock(&self.log).clone()
    }

    /// How many exchanges are left.
    pub fn remaining(&self) -> usize {
        lock(&self.script).len()
    }

    /// Panics unless every exchange was used.
    pub fn assert_done(&self) {
        let left = self.remaining();
        assert!(
            left == 0,
            "ReplayTransport: {left} scripted exchange(s) were never requested"
        );
    }

    fn answer(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        super::check_request(&request)?;
        let number = {
            let mut log = lock(&self.log);
            log.push(request.clone());
            log.len()
        };
        let Some(exchange) = lock(&self.script).pop_front() else {
            panic!(
                "ReplayTransport: unexpected request #{number}: {} {} ({} lane); nothing more was scripted",
                request.method.as_str(),
                request.url,
                request.lane.as_str()
            );
        };
        if let Some(difference) = difference(&exchange.expect, &request) {
            panic!(
                "ReplayTransport: request #{number} ({} {}) differs from the script: {difference}",
                request.method.as_str(),
                request.url
            );
        }
        let cap = request.max_response_bytes;
        match exchange.reply {
            Reply::Error(error) => Err(error),
            Reply::Response(response) => {
                if response.body.len() as u64 > cap {
                    Err(TransportError::too_large(cap))
                } else {
                    Ok(response)
                }
            }
            Reply::Zeros {
                status,
                headers,
                len,
            } => {
                if len > cap {
                    return Err(TransportError::too_large(cap));
                }
                let len = usize::try_from(len).map_err(|_| TransportError::too_large(cap))?;
                Ok(HttpResponse {
                    status,
                    headers,
                    body: vec![0; len],
                })
            }
        }
    }
}

impl Transport for ReplayTransport {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, TransportError>> {
        Box::pin(async move { self.answer(request) })
    }
}

/// The first way `request` differs from `expect`, if any.
fn difference(expect: &Expect, request: &HttpRequest) -> Option<String> {
    if expect.method != request.method {
        return Some(format!(
            "method {} expected, {} sent",
            expect.method.as_str(),
            request.method.as_str()
        ));
    }
    if expect.url != request.url {
        return Some(format!("URL {} expected, {} sent", expect.url, request.url));
    }
    if expect.lane != request.lane {
        return Some(format!(
            "lane {} expected, {} sent",
            expect.lane.as_str(),
            request.lane.as_str()
        ));
    }
    let credential = request
        .credential
        .as_ref()
        .map(|c| format!("{}: {}", c.header, c.header_value()));
    if expect.credential != credential {
        return Some(format!(
            "credential {:?} expected, {:?} sent",
            expect.credential, credential
        ));
    }
    for (name, value) in &expect.headers {
        let sent = request
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str());
        if sent != Some(value.as_str()) {
            return Some(format!("header {name}: {value:?} expected, {sent:?} sent"));
        }
    }
    let body = match (&expect.body, &request.body) {
        (ExpectBody::Any, _) | (ExpectBody::Empty, Body::Empty) => return None,
        (ExpectBody::Json(want), Body::Json(sent)) => {
            (want == sent).then_some(()).ok_or_else(|| {
                format!(
                    "JSON body\n  expected {}\n  sent     {}",
                    compact(want),
                    compact(sent)
                )
            })
        }
        (ExpectBody::JsonText(want), Body::Json(sent)) => {
            let sent = compact(sent);
            (*want == sent)
                .then_some(())
                .ok_or_else(|| format!("JSON text\n  expected {want}\n  sent     {sent}"))
        }
        (ExpectBody::Text(want), Body::Bytes { bytes, .. }) => (want.as_bytes()
            == bytes.as_slice())
        .then_some(())
        .ok_or_else(|| "text body differs".to_string()),
        (ExpectBody::Bytes(want), Body::Bytes { bytes, .. }) => (want == bytes)
            .then_some(())
            .ok_or_else(|| format!("{} body bytes expected, {} sent", want.len(), bytes.len())),
        (ExpectBody::Multipart(want), Body::Multipart(sent)) => multipart_difference(want, sent),
        (want, sent) => Err(format!(
            "body {} expected, {} sent",
            body_word(want),
            sent_word(sent)
        )),
    };
    body.err()
}

fn multipart_difference(want: &[Part], sent: &[Part]) -> Result<(), String> {
    if want.len() != sent.len() {
        return Err(format!(
            "{} multipart parts expected, {} sent ({:?})",
            want.len(),
            sent.len(),
            sent.iter().map(|p| p.name.as_str()).collect::<Vec<_>>()
        ));
    }
    for (i, (w, s)) in want.iter().zip(sent).enumerate() {
        if w.name != s.name || w.filename != s.filename || w.content_type != s.content_type {
            return Err(format!(
                "part {i}: name={:?} filename={:?} type={:?} expected, name={:?} filename={:?} type={:?} sent",
                w.name, w.filename, w.content_type, s.name, s.filename, s.content_type
            ));
        }
        if w.data != s.data {
            return Err(format!(
                "part {i} ({}): {} bytes expected, {} different bytes sent",
                w.name,
                w.data.len(),
                s.data.len()
            ));
        }
    }
    Ok(())
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn body_word(body: &ExpectBody) -> &'static str {
    match body {
        ExpectBody::Any => "any",
        ExpectBody::Empty => "none",
        ExpectBody::Json(_) | ExpectBody::JsonText(_) => "JSON",
        ExpectBody::Text(_) | ExpectBody::Bytes(_) => "bytes",
        ExpectBody::Multipart(_) => "multipart",
    }
}

fn sent_word(body: &Body) -> &'static str {
    match body {
        Body::Empty => "none",
        Body::Json(_) => "JSON",
        Body::Bytes { .. } => "bytes",
        Body::Multipart(_) => "multipart",
    }
}

/// Parses an `fx-exchanges-v1` fixture; file bodies are read relative to `base`.
pub fn parse_fixture(text: &str, base: &Path) -> Result<Vec<Exchange>, String> {
    let document: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    if document.get("kind").and_then(Value::as_str) != Some("fx-exchanges-v1") {
        return Err("kind is not fx-exchanges-v1".into());
    }
    let exchanges = document
        .get("exchanges")
        .and_then(Value::as_array)
        .ok_or("exchanges is not a list")?;
    exchanges
        .iter()
        .enumerate()
        .map(|(i, exchange)| {
            parse_exchange(exchange, base).map_err(|e| format!("exchanges.{i}: {e}"))
        })
        .collect()
}

fn parse_exchange(exchange: &Value, base: &Path) -> Result<Exchange, String> {
    let request = exchange.get("request").ok_or("no request")?;
    let text = |value: &Value, key: &str| -> Result<String, String> {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("request.{key} is not text"))
    };
    let method = match text(request, "method")?.as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        other => return Err(format!("method {other} is not GET or POST")),
    };
    let lane = match request
        .get("lane")
        .and_then(Value::as_str)
        .unwrap_or("provider")
    {
        "provider" => Lane::Provider,
        "upload" => Lane::Upload,
        "download" => Lane::Download,
        other => return Err(format!("lane {other} is not provider, upload or download")),
    };
    let mut expect = Expect::new(method, text(request, "url")?, lane);
    expect.credential = match request.get("credential") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("request.credential is not text".into()),
    };
    if let Some(headers) = request.get("headers") {
        expect.headers = header_pairs(headers)?;
    }
    expect.body = match request.get("body") {
        None | Some(Value::Null) => ExpectBody::Empty,
        Some(Value::String(s)) if s == "any" => ExpectBody::Any,
        Some(body) => expect_body(body)?,
    };
    let reply = match (exchange.get("response"), exchange.get("error")) {
        (Some(response), None) => response_reply(response, base)?,
        (None, Some(error)) => Reply::Error(transport_error(error)?),
        _ => return Err("an exchange has exactly one of response and error".into()),
    };
    Ok(Exchange { expect, reply })
}

fn header_pairs(headers: &Value) -> Result<Vec<(String, String)>, String> {
    headers
        .as_object()
        .ok_or("headers is not an object")?
        .iter()
        .map(|(k, v)| {
            v.as_str()
                .map(|v| (k.to_ascii_lowercase(), v.to_string()))
                .ok_or_else(|| format!("header {k} is not text"))
        })
        .collect()
}

fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(text)
        .map_err(|_| "base64 does not decode".to_string())
}

fn expect_body(body: &Value) -> Result<ExpectBody, String> {
    let object = body.as_object().ok_or("body is not an object")?;
    if let Some(value) = object.get("json") {
        return Ok(ExpectBody::Json(value.clone()));
    }
    if let Some(Value::String(s)) = object.get("json_text") {
        return Ok(ExpectBody::JsonText(s.clone()));
    }
    if let Some(Value::String(s)) = object.get("text") {
        return Ok(ExpectBody::Text(s.clone()));
    }
    if let Some(Value::String(s)) = object.get("base64") {
        return Ok(ExpectBody::Bytes(decode_base64(s)?));
    }
    if let Some(Value::Array(parts)) = object.get("multipart") {
        return parts
            .iter()
            .map(|part| {
                let name = part
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("a part has no name")?;
                let optional =
                    |key: &str| part.get(key).and_then(Value::as_str).map(str::to_string);
                let data = match (part.get("text"), part.get("base64")) {
                    (Some(Value::String(t)), None) => t.as_bytes().to_vec(),
                    (None, Some(Value::String(b))) => decode_base64(b)?,
                    _ => return Err("a part has exactly one of text and base64".to_string()),
                };
                Ok(Part {
                    name: name.to_string(),
                    filename: optional("filename"),
                    content_type: optional("content_type"),
                    data,
                })
            })
            .collect::<Result<_, _>>()
            .map(ExpectBody::Multipart);
    }
    Err("body is none of json, json_text, text, base64, multipart".into())
}

fn response_reply(response: &Value, base: &Path) -> Result<Reply, String> {
    let status = response
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok())
        .ok_or("response.status is not a status code")?;
    let headers = match response.get("headers") {
        None => Vec::new(),
        Some(headers) => header_pairs(headers)?,
    };
    let body = match response.get("body") {
        None | Some(Value::Null) => Vec::new(),
        Some(body) => {
            let object = body.as_object().ok_or("response.body is not an object")?;
            if let Some(value) = object.get("json") {
                serde_json::to_vec(value).map_err(|e| e.to_string())?
            } else if let Some(Value::String(s)) = object.get("text") {
                s.as_bytes().to_vec()
            } else if let Some(Value::String(s)) = object.get("base64") {
                decode_base64(s)?
            } else if let Some(len) = object.get("zeros").and_then(Value::as_u64) {
                return Ok(Reply::Zeros {
                    status,
                    headers,
                    len,
                });
            } else if let Some(Value::String(file)) = object.get("file") {
                std::fs::read(base.join(file)).map_err(|e| format!("body file {file}: {e}"))?
            } else {
                return Err("response.body is none of json, text, base64, zeros, file".into());
            }
        }
    };
    Ok(Reply::Response(HttpResponse {
        status,
        headers,
        body,
    }))
}

fn transport_error(error: &Value) -> Result<TransportError, String> {
    let phase = match error.get("phase").and_then(Value::as_str) {
        Some("not_sent") => Phase::NotSent,
        Some("after_send") => Phase::AfterSend,
        _ => return Err("error.phase is not not_sent or after_send".into()),
    };
    let kind = match error.get("kind").and_then(Value::as_str).unwrap_or("other") {
        "connect" => TransportErrorKind::Connect,
        "timeout" => TransportErrorKind::Timeout,
        "too_large" => TransportErrorKind::TooLarge,
        "refused" => TransportErrorKind::Refused,
        "other" => TransportErrorKind::Other,
        other => return Err(format!("error.kind {other} is unknown")),
    };
    let reason = error
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("a scripted transport failure")
        .to_string();
    Ok(TransportError {
        phase,
        kind,
        reason,
    })
}

/// A transport that must never be used: any send panics, naming only the request's origin.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoNetwork;

impl Transport for NoNetwork {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, TransportError>> {
        panic!(
            "NoNetwork: a test tried to reach {} {} ({} lane)",
            request.method.as_str(),
            request.origin(),
            request.lane.as_str()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Secret;
    use crate::transport::Credential;
    use serde_json::json;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn post(url: &str, body: Value) -> HttpRequest {
        HttpRequest::new(Method::Post, url, Lane::Provider)
            .credential(Credential {
                header: "authorization",
                prefix: "Bearer ",
                secret: Secret::new("test-key"),
            })
            .header("content-type", "application/json")
            .body(Body::Json(body))
    }

    #[test]
    fn scripted_exchanges_play_in_order_and_are_logged() {
        let transport = ReplayTransport::new(vec![
            Expect::post_json("https://api.example.test/v1/a", json!({"x": 1, "y": [2]}))
                .credential("authorization", "Bearer test-key")
                .header("content-type", "application/json")
                .reply(HttpResponse::json(200, &json!({"ok": true}))),
            Expect::post_json("https://api.example.test/v1/a", json!({"x": 1, "y": [2]}))
                .credential("authorization", "Bearer test-key")
                .fail(TransportError::not_sent(
                    TransportErrorKind::Connect,
                    "refused",
                )),
        ]);
        let response = block_on(transport.send(post(
            "https://api.example.test/v1/a",
            json!({"y": [2], "x": 1}),
        )))
        .unwrap();
        assert_eq!(response.status, 200);
        let error = block_on(transport.send(post(
            "https://api.example.test/v1/a",
            json!({"x": 1, "y": [2]}),
        )))
        .unwrap_err();
        assert_eq!(error.phase, Phase::NotSent);
        assert_eq!(transport.requests().len(), 2);
        transport.assert_done();
    }

    #[test]
    #[should_panic(expected = "differs from the script")]
    fn a_different_body_panics() {
        let transport = ReplayTransport::new(vec![
            Expect::post_json("https://api.example.test/v1/a", json!({"x": 1}))
                .credential("authorization", "Bearer test-key")
                .reply(HttpResponse::new(200, Vec::new())),
        ]);
        let _ = block_on(transport.send(post("https://api.example.test/v1/a", json!({"x": 2}))));
    }

    #[test]
    #[should_panic(expected = "nothing more was scripted")]
    fn an_unscripted_request_panics() {
        let transport = ReplayTransport::new(Vec::new());
        let _ = block_on(transport.send(post("https://api.example.test/v1/a", json!({}))));
    }

    #[test]
    #[should_panic(expected = "NoNetwork")]
    fn no_network_panics() {
        let _ = block_on(NoNetwork.send(post("https://api.example.test/v1/a", json!({}))));
    }

    #[test]
    fn bodies_over_the_cap_are_too_large_without_allocating() {
        let transport = ReplayTransport::new(vec![Exchange {
            expect: Expect::download("https://files.example.test/m.glb"),
            reply: Reply::Zeros {
                status: 200,
                headers: Vec::new(),
                len: 150_000_001,
            },
        }]);
        let request = HttpRequest::new(
            Method::Get,
            "https://files.example.test/m.glb",
            Lane::Download,
        )
        .max_response_bytes(150_000_000);
        let error = block_on(transport.send(request)).unwrap_err();
        assert_eq!(error.kind, TransportErrorKind::TooLarge);
        assert_eq!(error.phase, Phase::AfterSend);
    }

    #[test]
    fn fixtures_parse() {
        let text = r#"{"kind": "fx-exchanges-v1", "exchanges": [
          {"request": {"method": "POST", "url": "https://api.example.test/files", "lane": "upload",
                       "credential": "authorization: Bearer k",
                       "body": {"multipart": [{"name": "file", "filename": "front.png",
                                               "content_type": "image/png", "base64": "iVBORw=="}]}},
           "response": {"status": 200, "body": {"json": {"code": 0, "data": {"file_token": "t1"}}}}},
          {"request": {"method": "GET", "url": "https://files.example.test/x", "lane": "download"},
           "error": {"phase": "after_send", "kind": "other", "reason": "reset"}}]}"#;
        let exchanges = parse_fixture(text, Path::new(".")).unwrap();
        assert_eq!(exchanges.len(), 2);
        assert_eq!(exchanges[0].expect.lane, Lane::Upload);
        assert!(
            matches!(&exchanges[0].expect.body, ExpectBody::Multipart(parts) if parts[0].filename.as_deref() == Some("front.png"))
        );
        assert!(matches!(&exchanges[1].reply, Reply::Error(e) if e.phase == Phase::AfterSend));
        assert!(parse_fixture(r#"{"kind": "other", "exchanges": []}"#, Path::new(".")).is_err());
    }

    #[test]
    fn a_credential_on_the_download_lane_consumes_nothing() {
        let transport = ReplayTransport::new(vec![
            Expect::download("https://files.example.test/x")
                .reply(HttpResponse::new(200, b"x".to_vec())),
        ]);
        let request = HttpRequest::new(Method::Get, "https://files.example.test/x", Lane::Download)
            .credential(Credential {
                header: "authorization",
                prefix: "",
                secret: Secret::new("k"),
            });
        assert!(block_on(transport.send(request)).is_err());
        assert_eq!(transport.remaining(), 1);
    }
}
