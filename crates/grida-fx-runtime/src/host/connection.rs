//! An asynchronous JSON-RPC connection to a node host (spec/protocol.md §1, §2): requests in both
//! directions, interleaved.
//!
//! - A **reader task** reads frames (LSP `Content-Length` framing, with the limits and errors of
//!   `grida_fx_protocol::framing`) and parses each body with the strict I-JSON reader
//!   (`grida_fx_core::value::parse_json`):
//!   - a response resolves the pending request with its id (one-shot channel); a response to an id
//!     that is not pending, a second response, or an error with a `null` id breaks the connection;
//!   - a request goes to the [`Incoming`] handler on a task of its own, so a slow `capability` call
//!     never stops the reader; its answer is written when it resolves;
//!   - a notification (`progress`) goes to [`Incoming::notify`];
//!   - a message outside I-JSON is answered `-32700` when it was a request, else breaks the
//!     connection; a message that is not JSON-RPC 2.0 is answered `-32600` when it carries a method
//!     and an id, else breaks it. Whether a message the strict reader refused was a request, and
//!     its id, is found by a scan of its top-level members that reads no other value, so a request
//!     holding `NaN` or nesting deeper than any reader goes is still answered.
//! - Writes go through one writer (a mutex around the input half), one whole frame at a time.
//!   Each write runs on a task of its own, so a caller that stops waiting never leaves half a
//!   frame on the wire.
//! - The engine numbers its requests from 1.
//! - When the host's output ends, or the connection breaks, every pending request fails with
//!   [`ConnectionError::Closed`] or [`ConnectionError::Protocol`], and so does every later call;
//!   nothing more is written, not even the answer to a host request that resolves afterwards.
//! - [`Connection::request_cancellable`] sends `$/cancel {id}` when its token fires and keeps
//!   waiting for the answer (the host answers `cancelled`); the caller decides how long to wait.
//!   A response that arrives for a request whose caller stopped waiting is discarded.

use crate::engine::Cancel;
use grida_fx_core::value::parse_json;
use grida_fx_protocol::framing::write_message;
use grida_fx_protocol::{ErrorCode, Id, Message, RpcError, method};
use grida_fx_providers::BoxFuture;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::{oneshot, watch};

/// Why a request got no result.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionError {
    /// The host's output ended (it exited) before the response arrived.
    Closed,
    /// A message broke the protocol, or writing failed: the sentence says which.
    Protocol(String),
    /// The host answered with an error.
    Rpc(RpcError),
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionError::Closed => f.write_str("the node host closed the session"),
            ConnectionError::Protocol(message) => f.write_str(message),
            ConnectionError::Rpc(error) => f.write_str(&error.message),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// Serves what a host sends: requests (answered with a result or an error) and notifications.
pub trait Incoming: Send + Sync {
    /// A host request. The future runs on a task of its own.
    fn request(
        &self,
        method: String,
        params: Option<Value>,
    ) -> BoxFuture<'static, Result<Value, RpcError>>;

    /// A host notification. Called on the reader task: it must not block.
    fn notify(&self, method: String, params: Option<Value>);
}

/// Answers every host request with `-32601`: what `describe` and `build` expect.
#[derive(Debug, Default)]
pub struct NoIncoming;

impl Incoming for NoIncoming {
    fn request(
        &self,
        method: String,
        _params: Option<Value>,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        Box::pin(async move {
            Err(RpcError::new(
                grida_fx_protocol::ErrorCode::MethodNotFound,
                format!("the engine serves no {method} here"),
            ))
        })
    }

    fn notify(&self, _method: String, _params: Option<Value>) {}
}

/// One connection (module doc). Cheap to clone; clones share the connection.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Answer = Result<Value, ConnectionError>;

struct Inner {
    /// The host's input; `None` once the engine closed it.
    writer: tokio::sync::Mutex<Option<Writer>>,
    state: Mutex<State>,
    /// `true` once the connection has ended.
    ended: watch::Sender<bool>,
    incoming: Arc<dyn Incoming>,
}

struct State {
    next_id: i64,
    /// The engine's requests waiting for a response. An entry stays until its response arrives
    /// (or the connection ends), even when its caller stopped waiting.
    pending: HashMap<i64, oneshot::Sender<Answer>>,
    ended: Option<Ended>,
}

/// How a connection ended. Kept so every later call fails the same way.
#[derive(Debug, Clone)]
enum Ended {
    Closed,
    Broken(String),
}

impl Ended {
    fn error(&self) -> ConnectionError {
        match self {
            Ended::Closed => ConnectionError::Closed,
            Ended::Broken(message) => ConnectionError::Protocol(message.clone()),
        }
    }
}

impl Connection {
    /// Starts the reader task on the current runtime (it must be called inside one).
    pub fn start<R, W>(reader: R, writer: W, incoming: Arc<dyn Incoming>) -> Connection
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let connection = Connection {
            inner: Arc::new(Inner {
                writer: tokio::sync::Mutex::new(Some(Box::new(writer))),
                state: Mutex::new(State {
                    next_id: 1,
                    pending: HashMap::new(),
                    ended: None,
                }),
                ended: watch::channel(false).0,
                incoming,
            }),
        };
        tokio::spawn(read_loop(connection.clone(), reader));
        connection
    }

    /// Sends a request and waits for its result.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, ConnectionError> {
        self.call(method, params, None).await
    }

    /// As [`Connection::request`], sending `$/cancel` for it when `cancel` fires (module doc).
    pub async fn request_cancellable(
        &self,
        method: &str,
        params: Option<Value>,
        cancel: &Cancel,
    ) -> Result<Value, ConnectionError> {
        self.call(method, params, Some(cancel)).await
    }

    /// Sends a notification (`exit`, `$/cancel`).
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), ConnectionError> {
        self.send(&Message::Notification {
            method: method.to_string(),
            params,
        })
        .await
    }

    /// Whether the connection still carries messages.
    pub fn is_open(&self) -> bool {
        self.state().ended.is_none()
    }

    /// Resolves when the host's output has ended or the connection broke.
    pub async fn closed(&self) {
        let mut ended = self.inner.ended.subscribe();
        let _ = ended.wait_for(|ended| *ended).await;
    }

    /// Ends the connection as if the host's output had ended: every pending request fails with
    /// [`ConnectionError::Closed`] and the host's output is no longer read. For a host that has
    /// exited while a process it left holds its output open.
    pub(crate) fn abandon(&self) {
        self.end(Ended::Closed);
    }

    /// Closes the host's input (end of file for the host). Later writes fail with
    /// [`ConnectionError::Closed`]; responses are still read until the host's output ends. Waits
    /// for a write in progress to finish.
    pub(crate) async fn close_input(&self) {
        let mut writer = self.inner.writer.lock().await;
        if let Some(mut writer) = writer.take() {
            let _ = writer.shutdown().await;
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends one request and waits for its answer, sending `$/cancel` once when `cancel` fires.
    async fn call(
        &self,
        method: &str,
        params: Option<Value>,
        cancel: Option<&Cancel>,
    ) -> Result<Value, ConnectionError> {
        let (id, mut answer) = self.register()?;
        let request = Message::Request {
            id: Id::Number(id),
            method: method.to_string(),
            params,
        };
        if let Err(error) = self.send(&request).await {
            self.state().pending.remove(&id);
            return Err(error);
        }
        let mut asked = false;
        loop {
            tokio::select! {
                received = &mut answer => {
                    return received.unwrap_or_else(|_| Err(self.ended_error()));
                }
                _ = cancelled(cancel), if !asked => {
                    asked = true;
                    let _ = self
                        .notify(method::CANCEL, Some(json!({"id": id})))
                        .await;
                }
            }
        }
    }

    /// A new id with its pending entry; refused once the connection has ended.
    fn register(&self) -> Result<(i64, oneshot::Receiver<Answer>), ConnectionError> {
        let mut state = self.state();
        if let Some(ended) = &state.ended {
            return Err(ended.error());
        }
        let id = state.next_id;
        state.next_id += 1;
        let (sender, receiver) = oneshot::channel();
        state.pending.insert(id, sender);
        Ok((id, receiver))
    }

    /// The error every call gets once the connection has ended.
    fn ended_error(&self) -> ConnectionError {
        self.state()
            .ended
            .as_ref()
            .map_or(ConnectionError::Closed, Ended::error)
    }

    fn check_open(&self) -> Result<(), ConnectionError> {
        match &self.state().ended {
            Some(ended) => Err(ended.error()),
            None => Ok(()),
        }
    }

    /// Writes one message as one frame, on a task of its own (module doc).
    async fn send(&self, message: &Message) -> Result<(), ConnectionError> {
        self.check_open()?;
        let body = serde_json::to_vec(&message.to_value()).map_err(|e| {
            ConnectionError::Protocol(format!(
                "a message cannot be written: {}",
                super::read_reason(&e)
            ))
        })?;
        let mut frame = Vec::with_capacity(body.len() + 32);
        write_message(&mut frame, &body).map_err(|e| {
            ConnectionError::Protocol(format!(
                "a message cannot be written: {}",
                grida_fx_core::error::io_reason(&e)
            ))
        })?;
        let connection = self.clone();
        match tokio::spawn(async move { connection.write_frame(frame).await }).await {
            Ok(written) => written,
            Err(_) => Err(ConnectionError::Protocol(
                "writing to the node host stopped before the message was written".into(),
            )),
        }
    }

    async fn write_frame(&self, frame: Vec<u8>) -> Result<(), ConnectionError> {
        let mut writer = self.inner.writer.lock().await;
        // The connection may have ended while this write waited for the writer.
        self.check_open()?;
        let Some(writer) = writer.as_mut() else {
            return Err(ConnectionError::Closed);
        };
        let written = async {
            writer.write_all(&frame).await?;
            writer.flush().await
        }
        .await;
        match written {
            Ok(()) => Ok(()),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                Err(self.end(Ended::Closed))
            }
            Err(e) => Err(self.end(Ended::Broken(format!(
                "cannot write to the node host: {}",
                grida_fx_core::error::io_reason(&e)
            )))),
        }
    }

    /// Ends the connection (the first reason wins), fails every pending request, and returns the
    /// error for it.
    fn end(&self, ended: Ended) -> ConnectionError {
        let (error, waiting) = {
            let mut state = self.state();
            let waiting = if state.ended.is_none() {
                state.ended = Some(ended);
                std::mem::take(&mut state.pending)
            } else {
                HashMap::new()
            };
            let error = state
                .ended
                .as_ref()
                .map_or(ConnectionError::Closed, Ended::error);
            (error, waiting)
        };
        for (_, sender) in waiting {
            let _ = sender.send(Err(error.clone()));
        }
        self.inner.ended.send_replace(true);
        error
    }

    /// Acts on one frame's body (module doc).
    fn receive(&self, body: &[u8]) {
        let parsed = match std::str::from_utf8(body) {
            Ok(text) => parse_json(text).map_err(|refused| refused.message),
            Err(_) => Err("it is not UTF-8".to_string()),
        };
        let value = match parsed {
            Ok(value) => value,
            Err(reason) => {
                match lenient_request_id(body) {
                    Some(id) => self.reply(Message::Error {
                        id: Some(id),
                        error: RpcError::new(
                            ErrorCode::ParseError,
                            format!("the message is not I-JSON: {reason}"),
                        ),
                    }),
                    None => {
                        self.end(Ended::Broken(format!(
                            "the node host sent a message that is not I-JSON: {reason}"
                        )));
                    }
                }
                return;
            }
        };
        let request_id = request_id_of(&value);
        let message = match Message::from_value(value) {
            Ok(message) => message,
            Err(error) => {
                match request_id {
                    Some(id) => self.reply(Message::Error {
                        id: Some(id),
                        error,
                    }),
                    None => {
                        self.end(Ended::Broken(format!(
                            "the node host sent a message that is not JSON-RPC 2.0: {}",
                            error.message
                        )));
                    }
                }
                return;
            }
        };
        match message {
            Message::Request { id, method, params } => {
                let answer = self.inner.incoming.request(method, params);
                let connection = self.clone();
                tokio::spawn(async move {
                    let reply = match answer.await {
                        Ok(result) => Message::Response { id, result },
                        Err(error) => Message::Error {
                            id: Some(id),
                            error,
                        },
                    };
                    let _ = connection.send(&reply).await;
                });
            }
            Message::Notification { method, params } => {
                self.inner.incoming.notify(method, params);
            }
            Message::Response { id, result } => self.deliver(id, Ok(result)),
            Message::Error {
                id: Some(id),
                error,
            } => self.deliver(id, Err(ConnectionError::Rpc(error))),
            Message::Error { id: None, error } => {
                self.end(Ended::Broken(format!(
                    "the node host could not read a message: {}",
                    error.message
                )));
            }
        }
    }

    /// Writes an answer of the engine's without holding up the reader.
    fn reply(&self, message: Message) {
        let connection = self.clone();
        tokio::spawn(async move {
            let _ = connection.send(&message).await;
        });
    }

    /// Resolves one of the engine's pending requests.
    fn deliver(&self, id: Id, answer: Answer) {
        let problem = {
            let mut state = self.state();
            if state.ended.is_some() {
                return;
            }
            let number = match &id {
                Id::Number(n) => Some(*n),
                Id::Text(_) => None,
            };
            if let Some(sender) = number.and_then(|n| state.pending.remove(&n)) {
                // The caller may have stopped waiting: the answer is then discarded.
                let _ = sender.send(answer);
                return;
            }
            match number {
                Some(n) if n >= 1 && n < state.next_id => {
                    format!("the node host answered request {id} twice")
                }
                _ => format!("the node host answered request {id}, which is not pending"),
            }
        };
        self.end(Ended::Broken(problem));
    }
}

/// Resolves when `cancel` fires; never without one.
async fn cancelled(cancel: Option<&Cancel>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
    }
}

/// The reader task: reads frames until the host's output ends or the connection ends.
async fn read_loop<R: AsyncRead + Unpin>(connection: Connection, reader: R) {
    let mut reader = BufReader::new(reader);
    let mut ended = connection.inner.ended.subscribe();
    loop {
        let frame = tokio::select! {
            biased;
            _ = ended.wait_for(|ended| *ended) => return,
            frame = read_frame(&mut reader) => frame,
        };
        match frame {
            Ok(Some(body)) => connection.receive(&body),
            Ok(None) => {
                connection.end(Ended::Closed);
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                connection.end(Ended::Closed);
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                connection.end(Ended::Broken(format!(
                    "the node host sent a malformed frame: {e}"
                )));
                return;
            }
            Err(e) => {
                connection.end(Ended::Broken(format!(
                    "cannot read the node host's output: {}",
                    grida_fx_core::error::io_reason(&e)
                )));
                return;
            }
        }
    }
}

/// The longest header line a reader accepts, in bytes, `\r\n` included (as
/// `grida_fx_protocol::framing`).
const MAX_HEADER_LINE: u64 = 8 * 1024;

/// Reads one frame's body: `grida_fx_protocol::framing::read_message` over an asynchronous
/// reader, with the same limits and errors. `Ok(None)` at a clean end of stream.
pub(crate) async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    let mut length: Option<usize> = None;
    let mut first = true;
    loop {
        let mut line = Vec::new();
        let read = (&mut *reader)
            .take(MAX_HEADER_LINE)
            .read_until(b'\n', &mut line)
            .await?;
        if read == 0 {
            if first {
                return Ok(None);
            }
            return Err(eof("the stream ended inside a frame header"));
        }
        first = false;
        if line.last() != Some(&b'\n') {
            if read as u64 >= MAX_HEADER_LINE {
                return Err(invalid("a frame header line is longer than 8 KiB"));
            }
            return Err(eof("the stream ended inside a frame header"));
        }
        if line.len() < 2 || line[line.len() - 2] != b'\r' {
            return Err(invalid("a frame header line does not end with \\r\\n"));
        }
        let line = &line[..line.len() - 2];
        if line.is_empty() {
            break;
        }
        let line = std::str::from_utf8(line)
            .ok()
            .filter(|line| line.is_ascii())
            .ok_or_else(|| invalid("a frame header line is not ASCII text"))?;
        let Some((name, value)) = line.split_once(':') else {
            return Err(invalid(format!(
                "a frame header line has no colon: {line:?}"
            )));
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        if length.is_some() {
            return Err(invalid("a frame has more than one Content-Length"));
        }
        let value = value.trim_matches(|c| c == ' ' || c == '\t');
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid(format!(
                "a frame's Content-Length is not a number: {value:?}"
            )));
        }
        let n = value
            .parse::<usize>()
            .map_err(|_| invalid(format!("a frame's Content-Length is too large: {value}")))?;
        length = Some(n);
    }
    let Some(length) = length else {
        return Err(invalid("a frame has no Content-Length"));
    };
    // Read incrementally: a huge length must not allocate before the bytes arrive.
    let mut body = Vec::with_capacity(length.min(64 * 1024));
    (&mut *reader)
        .take(length as u64)
        .read_to_end(&mut body)
        .await?;
    if body.len() != length {
        return Err(eof(format!(
            "the stream ended after {} of a frame's {length} bytes",
            body.len()
        )));
    }
    Ok(Some(body))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn eof(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, message.into())
}

/// The id of a parsed value that carries a method and an integer or string id: a request,
/// even when it is not a valid one.
fn request_id_of(value: &Value) -> Option<Id> {
    let object = value.as_object()?;
    object.get("method")?;
    match object.get("id")? {
        id @ (Value::Number(_) | Value::String(_)) => Id::from_value(id).ok(),
        _ => None,
    }
}

/// The request id of a body the strict reader refused; `None` unless the body is a JSON object
/// whose top-level members include a `method` and an integer or string `id` (once each). Only
/// the top level is read: the scan steps over every other value without reading it, so it
/// accepts what the strict reader refuses (`NaN`, any nesting), and it never recurses.
fn lenient_request_id(body: &[u8]) -> Option<Id> {
    let mut scan = Scan { body, at: 0 };
    scan.space();
    scan.byte(b'{')?;
    let (mut id, mut ids, mut methods) = (None, 0, 0);
    scan.space();
    if scan.peek() == Some(b'}') {
        return None;
    }
    loop {
        scan.space();
        let key = scan.string()?;
        scan.space();
        scan.byte(b':')?;
        scan.space();
        let start = scan.at;
        scan.skip_value()?;
        let value = &body[start..scan.at];
        match serde_json::from_slice::<String>(key).ok()?.as_str() {
            "id" => {
                ids += 1;
                id = serde_json::from_slice::<Value>(value).ok();
            }
            "method" => methods += 1,
            _ => {}
        }
        scan.space();
        match scan.next()? {
            b',' => continue,
            b'}' => break,
            _ => return None,
        }
    }
    if ids != 1 || methods != 1 {
        return None;
    }
    match id? {
        id @ (Value::Number(_) | Value::String(_)) => Id::from_value(&id).ok(),
        _ => None,
    }
}

/// A forward scan over a JSON text (see [`lenient_request_id`]).
struct Scan<'a> {
    body: &'a [u8],
    at: usize,
}

impl<'a> Scan<'a> {
    fn peek(&self) -> Option<u8> {
        self.body.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn byte(&mut self, expected: u8) -> Option<()> {
        (self.next()? == expected).then_some(())
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// A string, quotes included, left encoded.
    fn string(&mut self) -> Option<&'a [u8]> {
        let start = self.at;
        self.byte(b'"')?;
        loop {
            match self.next()? {
                b'"' => return Some(&self.body[start..self.at]),
                b'\\' => {
                    self.next()?;
                }
                _ => {}
            }
        }
    }

    /// Steps over one value of any kind and nesting, reading only strings and brackets; a
    /// scalar is whatever runs up to the next separator (`NaN` included).
    fn skip_value(&mut self) -> Option<()> {
        match self.peek()? {
            b'"' => self.string().map(|_| ()),
            b'[' | b'{' => {
                let mut open: Vec<u8> = Vec::new();
                loop {
                    match self.peek()? {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'[' => open.push(b']'),
                        b'{' => open.push(b'}'),
                        byte @ (b']' | b'}') => {
                            if open.pop()? != byte {
                                return None;
                            }
                            if open.is_empty() {
                                self.at += 1;
                                return Some(());
                            }
                        }
                        _ => {}
                    }
                    self.at += 1;
                }
            }
            _ => {
                let start = self.at;
                while let Some(byte) = self.peek() {
                    match byte {
                        b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r' => break,
                        b'"' | b'[' | b'{' => return None,
                        _ => self.at += 1,
                    }
                }
                (self.at > start).then_some(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grida_fx_protocol::framing::read_message;
    use std::pin::Pin;
    use std::sync::OnceLock;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{DuplexStream, duplex};
    use tokio::sync::mpsc;

    /// The host's end of an in-memory connection.
    struct Host {
        /// What the engine wrote.
        input: BufReader<DuplexStream>,
        /// What the engine reads.
        output: DuplexStream,
    }

    impl Host {
        /// The next message the engine wrote; `None` once the engine closed the host's input.
        async fn read(&mut self) -> Option<Value> {
            let body = read_frame(&mut self.input).await.unwrap()?;
            Some(serde_json::from_slice(&body).unwrap())
        }

        /// Sends raw bytes.
        async fn raw(&mut self, bytes: &[u8]) {
            self.output.write_all(bytes).await.unwrap();
        }

        /// Sends one frame with this body.
        async fn send(&mut self, body: &str) {
            let mut bytes = Vec::new();
            write_message(&mut bytes, body.as_bytes()).unwrap();
            self.raw(&bytes).await;
        }

        async fn send_json(&mut self, value: Value) {
            self.send(&serde_json::to_string(&value).unwrap()).await;
        }

        /// Ends the host's output.
        async fn close(&mut self) {
            self.output.shutdown().await.unwrap();
        }
    }

    fn connect(incoming: Arc<dyn Incoming>) -> (Connection, Host) {
        let (engine_reads, host_writes) = duplex(1 << 16);
        let (host_reads, engine_writes) = duplex(1 << 16);
        let connection = Connection::start(engine_reads, engine_writes, incoming);
        let host = Host {
            input: BufReader::new(host_reads),
            output: host_writes,
        };
        (connection, host)
    }

    fn spawn_request(
        connection: &Connection,
        method: &'static str,
        params: Option<Value>,
    ) -> tokio::task::JoinHandle<Result<Value, ConnectionError>> {
        let connection = connection.clone();
        tokio::spawn(async move { connection.request(method, params).await })
    }

    /// Sorts replies by id (numbers first), for answers written by tasks of their own.
    fn by_id(mut replies: Vec<Value>) -> Vec<Value> {
        replies.sort_by_key(|reply| (reply["id"].is_string(), reply["id"].to_string()));
        replies
    }

    #[tokio::test]
    async fn ids_count_from_one() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let engine = tokio::spawn({
            let connection = connection.clone();
            async move {
                let first = connection
                    .request("describe", Some(json!({"targets": []})))
                    .await
                    .unwrap();
                connection.notify("progress", None).await.unwrap();
                let second = connection.request("shutdown", None).await.unwrap();
                (first, second)
            }
        });
        // The bytes on the wire are compact JSON with jsonrpc first.
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"describe","params":{"targets":[]}}"#;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut wire = vec![0; header.len() + body.len()];
        host.input.read_exact(&mut wire).await.unwrap();
        assert_eq!(wire, [header.as_bytes(), body.as_bytes()].concat());
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": "a"}))
            .await;
        assert_eq!(
            host.read().await.unwrap(),
            json!({"jsonrpc": "2.0", "method": "progress"})
        );
        assert_eq!(
            host.read().await.unwrap(),
            json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"})
        );
        host.send_json(json!({"jsonrpc": "2.0", "id": 2, "result": {"b": [1]}}))
            .await;
        assert_eq!(engine.await.unwrap(), (json!("a"), json!({"b": [1]})));
        assert!(connection.is_open());
    }

    #[tokio::test]
    async fn errors_reach_the_caller() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let build = spawn_request(&connection, "build", None);
        host.read().await.unwrap();
        host.send_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32004, "message": "nodes/x.py has no function build"}
        }))
        .await;
        match build.await.unwrap() {
            Err(ConnectionError::Rpc(error)) => {
                assert_eq!(error.kind(), Some(ErrorCode::LoadFailed));
                assert_eq!(error.message, "nodes/x.py has no function build");
            }
            other => panic!("{other:?}"),
        }
        // The connection goes on after an error response.
        assert!(connection.is_open());
    }

    /// Records host notifications; answers `fact` with `{}` and refuses other requests.
    #[derive(Default)]
    struct Recorder {
        notes: Mutex<Vec<(String, Option<Value>)>>,
        requests: Mutex<Vec<String>>,
    }

    impl Incoming for Recorder {
        fn request(
            &self,
            method: String,
            _params: Option<Value>,
        ) -> BoxFuture<'static, Result<Value, RpcError>> {
            self.requests.lock().unwrap().push(method.clone());
            Box::pin(async move {
                if method == "fact" {
                    return Ok(json!({}));
                }
                Err(RpcError::new(ErrorCode::InvalidParams, "no"))
            })
        }

        fn notify(&self, method: String, params: Option<Value>) {
            self.notes.lock().unwrap().push((method, params));
        }
    }

    #[tokio::test]
    async fn host_messages_are_served_while_waiting() {
        let recorder = Arc::new(Recorder::default());
        let (connection, mut host) = connect(recorder.clone());
        let run = spawn_request(&connection, "run", Some(json!({})));
        assert_eq!(host.read().await.unwrap()["method"], json!("run"));
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "method": "fact", "params": {"run_id": "r1", "name": "n", "value": 1}})).await;
        host.send_json(json!({"jsonrpc": "2.0", "method": "progress", "params": {"run_id": "r1", "text": "half"}})).await;
        host.send_json(json!({"jsonrpc": "2.0", "id": "h2", "method": "annotate", "params": {}}))
            .await;
        let replies = by_id(vec![host.read().await.unwrap(), host.read().await.unwrap()]);
        assert_eq!(
            replies,
            [
                json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
                json!({"jsonrpc": "2.0", "id": "h2", "error": {"code": -32602, "message": "no"}}),
            ]
        );
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": {"outputs": {}}}))
            .await;
        assert_eq!(run.await.unwrap().unwrap(), json!({"outputs": {}}));
        assert_eq!(*recorder.requests.lock().unwrap(), ["fact", "annotate"]);
        assert_eq!(
            *recorder.notes.lock().unwrap(),
            vec![(
                "progress".to_string(),
                Some(json!({"run_id": "r1", "text": "half"}))
            )]
        );
    }

    #[tokio::test]
    async fn no_incoming_answers_method_not_found() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let describe = spawn_request(&connection, "describe", None);
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "method": "capability", "params": {}}))
            .await;
        let reply = host.read().await.unwrap();
        assert_eq!(reply["id"], json!(1));
        assert_eq!(reply["error"]["code"], json!(-32601));
        assert_eq!(
            reply["error"]["message"],
            json!("the engine serves no capability here")
        );
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": null}))
            .await;
        assert_eq!(describe.await.unwrap().unwrap(), Value::Null);
    }

    /// Sends `tool.invoke` back through the connection when the host asks for `agent.run`.
    struct Nesting {
        connection: OnceLock<Connection>,
        inner: mpsc::UnboundedSender<Result<Value, ConnectionError>>,
    }

    impl Incoming for Nesting {
        fn request(
            &self,
            method: String,
            _params: Option<Value>,
        ) -> BoxFuture<'static, Result<Value, RpcError>> {
            assert_eq!(method, "agent.run");
            let connection = self.connection.get().cloned().expect("connected");
            let inner = self.inner.clone();
            Box::pin(async move {
                let answer = connection
                    .request("tool.invoke", Some(json!({"name": "look"})))
                    .await;
                let _ = inner.send(answer.clone());
                answer.map_err(|e| RpcError::new(ErrorCode::NodeFailure, e.to_string()))
            })
        }

        fn notify(&self, _method: String, _params: Option<Value>) {}
    }

    fn nesting() -> (
        Connection,
        Host,
        mpsc::UnboundedReceiver<Result<Value, ConnectionError>>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let nesting = Arc::new(Nesting {
            connection: OnceLock::new(),
            inner: sender,
        });
        let (connection, host) = connect(nesting.clone());
        let _ = nesting.connection.set(connection.clone());
        (connection, host, receiver)
    }

    #[tokio::test]
    async fn requests_nest_in_both_directions() {
        let (connection, mut host, mut inner) = nesting();
        let run = spawn_request(&connection, "run", None);
        let sent = host.read().await.unwrap();
        assert_eq!((&sent["id"], &sent["method"]), (&json!(1), &json!("run")));
        // The host asks for an agent while the engine's run (1) is pending.
        host.send_json(json!({"jsonrpc": "2.0", "id": 7, "method": "agent.run", "params": {}}))
            .await;
        let invoke = host.read().await.unwrap();
        assert_eq!(
            (&invoke["id"], &invoke["method"]),
            (&json!(2), &json!("tool.invoke"))
        );
        // It answers the outer run first, then the inner tool.invoke (2).
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": "outer"}))
            .await;
        host.send_json(json!({"jsonrpc": "2.0", "id": 2, "result": {"content": "a cat"}}))
            .await;
        assert_eq!(run.await.unwrap().unwrap(), json!("outer"));
        assert_eq!(
            inner.recv().await.unwrap().unwrap(),
            json!({"content": "a cat"})
        );
        assert_eq!(
            host.read().await.unwrap(),
            json!({"jsonrpc": "2.0", "id": 7, "result": {"content": "a cat"}})
        );
        assert!(connection.state().pending.is_empty());
    }

    #[tokio::test]
    async fn an_end_inside_a_nested_request_ends_the_outer_one() {
        let (connection, mut host, mut inner) = nesting();
        let run = spawn_request(&connection, "run", None);
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 7, "method": "agent.run", "params": {}}))
            .await;
        assert_eq!(host.read().await.unwrap()["method"], json!("tool.invoke"));
        host.close().await;
        assert_eq!(run.await.unwrap(), Err(ConnectionError::Closed));
        assert_eq!(inner.recv().await.unwrap(), Err(ConnectionError::Closed));
        // Nothing was answered to the host after its output ended.
        connection.close_input().await;
        assert_eq!(host.read().await, None);
    }

    #[tokio::test]
    async fn end_of_output_is_closed() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        host.close().await;
        assert_eq!(
            connection.request("describe", None).await,
            Err(ConnectionError::Closed)
        );
        connection.closed().await;
        assert!(!connection.is_open());
        assert_eq!(
            connection.notify("exit", None).await,
            Err(ConnectionError::Closed)
        );

        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let describe = spawn_request(&connection, "describe", None);
        host.read().await.unwrap();
        host.raw(b"Content-Length: 40\r\n\r\n{\"jsonrpc\"").await;
        host.close().await;
        assert_eq!(describe.await.unwrap(), Err(ConnectionError::Closed));
    }

    /// The sentence a connection broke with, after the host sent `bytes` unprompted.
    async fn broken_by(bytes: &[u8]) -> String {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        host.raw(bytes).await;
        connection.closed().await;
        match connection.request("describe", None).await {
            Err(ConnectionError::Protocol(message)) => message,
            other => panic!("{other:?}"),
        }
    }

    fn frame(body: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_message(&mut bytes, body.as_bytes()).unwrap();
        bytes
    }

    #[tokio::test]
    async fn unknown_and_repeated_response_ids_break_the_connection() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let first = spawn_request(&connection, "describe", None);
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 5, "result": null}))
            .await;
        let broken = Err(ConnectionError::Protocol(
            "the node host answered request 5, which is not pending".into(),
        ));
        assert_eq!(first.await.unwrap(), broken);
        // Ended: the next call fails the same way and writes nothing.
        assert_eq!(connection.request("describe", None).await, broken);
        connection.close_input().await;
        assert_eq!(host.read().await, None);

        assert_eq!(
            broken_by(&frame(r#"{"jsonrpc":"2.0","id":"x","result":1}"#)).await,
            "the node host answered request \"x\", which is not pending"
        );

        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let first = spawn_request(&connection, "a", None);
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": 1}))
            .await;
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": 2}))
            .await;
        assert_eq!(first.await.unwrap(), Ok(json!(1)));
        connection.closed().await;
        assert_eq!(
            connection.request("b", None).await,
            Err(ConnectionError::Protocol(
                "the node host answered request 1 twice".into()
            ))
        );
    }

    #[tokio::test]
    async fn a_request_outside_ijson_is_answered_parse_error() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let run = spawn_request(&connection, "run", None);
        host.read().await.unwrap();
        host.send(r#"{"jsonrpc":"2.0","id":3,"method":"fact","params":{"a":1,"a":2}}"#)
            .await;
        host.send(r#"{"jsonrpc":"2.0","id":4,"method":"fact","params":{"n":9007199254740993}}"#)
            .await;
        let replies = by_id(vec![host.read().await.unwrap(), host.read().await.unwrap()]);
        for (reply, id) in replies.iter().zip([3, 4]) {
            assert_eq!(reply["id"], json!(id));
            assert_eq!(reply["error"]["code"], json!(-32700));
            assert!(
                reply["error"]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("the message is not I-JSON: ")
            );
        }
        host.send(r#"{"jsonrpc":"2.0","id":1,"result":true}"#).await;
        assert_eq!(run.await.unwrap(), Ok(json!(true)));
    }

    #[tokio::test]
    async fn requests_no_reader_takes_are_answered_parse_error_with_their_id() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let run = spawn_request(&connection, "run", None);
        host.read().await.unwrap();
        // NaN, which no JSON reader takes, and nesting deeper than any reader goes.
        host.send(r#"{"jsonrpc":"2.0","id":9001,"method":"fact","params":{"value":NaN}}"#)
            .await;
        let deep = format!(
            r#"{{"jsonrpc":"2.0","method":"fact","params":{{"value":{}{}}},"id":"deep"}}"#,
            "[".repeat(600),
            "]".repeat(600)
        );
        host.send(&deep).await;
        let replies = by_id(vec![host.read().await.unwrap(), host.read().await.unwrap()]);
        assert_eq!(replies[0]["id"], json!(9001));
        assert_eq!(replies[1]["id"], json!("deep"));
        for reply in &replies {
            assert_eq!(reply["error"]["code"], json!(-32700), "{reply}");
        }
        assert!(
            replies[1]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("nested too deeply")
        );
        // The connection goes on.
        assert!(connection.is_open());
        host.send(r#"{"jsonrpc":"2.0","id":1,"result":true}"#).await;
        assert_eq!(run.await.unwrap(), Ok(json!(true)));
    }

    #[test]
    fn request_ids_are_found_without_reading_other_values() {
        let id = |body: &str| lenient_request_id(body.as_bytes());
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":7,"method":"fact","params":{"v":NaN}}"#),
            Some(Id::Number(7))
        );
        assert_eq!(
            id(
                r#" { "params" : [Infinity, -Infinity, {"id": 1}] , "method":"x", "id" : "a\"b" } "#
            ),
            Some(Id::Text("a\"b".into()))
        );
        assert_eq!(
            id(r#"{"method":"x","params":{"s":"} ] , \" {"},"id":2}"#),
            Some(Id::Number(2))
        );
        let deep = format!(
            r#"{{"id":3,"method":"x","params":{}{}}}"#,
            "{\"a\":".repeat(100_000),
            "1".to_string() + &"}".repeat(100_000)
        );
        assert_eq!(id(&deep), Some(Id::Number(3)));
        // Not a request, or no id it could be answered by.
        for body in [
            r#"{"id":1,"result":NaN}"#,
            r#"{"method":"progress","params":NaN}"#,
            r#"{"id":null,"method":"x","v":NaN}"#,
            r#"{"id":1.5,"method":"x","v":NaN}"#,
            r#"{"id":[1],"method":"x","v":NaN}"#,
            r#"{"id":1,"id":2,"method":"x","v":NaN}"#,
            r#"{"id":1,"method":"x","v":[NaN}"#,
            r#"{"id":1,"method":"x" "v":1}"#,
            r#"[{"id":1,"method":"x"}]"#,
            r#"{"id":1,"method":"x","#,
            "{}",
            "",
            "NaN",
        ] {
            assert_eq!(id(body), None, "{body}");
        }
    }

    #[tokio::test]
    async fn a_response_outside_ijson_ends_the_connection() {
        assert!(
            broken_by(&frame(r#"{"jsonrpc":"2.0","id":1,"result":{"a":1,"a":2}}"#))
                .await
                .starts_with("the node host sent a message that is not I-JSON")
        );
        assert!(
            broken_by(&frame("not json"))
                .await
                .starts_with("the node host sent a message that is not I-JSON")
        );
        let mut bytes = Vec::new();
        write_message(
            &mut bytes,
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"\xff\"}",
        )
        .unwrap();
        assert!(broken_by(&bytes).await.contains("not UTF-8"));
    }

    #[tokio::test]
    async fn an_invalid_request_is_answered_invalid_request() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let run = spawn_request(&connection, "run", None);
        host.read().await.unwrap();
        host.send(r#"{"id":4,"method":"fact"}"#).await;
        host.send(r#"{"jsonrpc":"2.0","id":"x","method":"fact","params":7}"#)
            .await;
        let replies = by_id(vec![host.read().await.unwrap(), host.read().await.unwrap()]);
        assert_eq!(replies[0]["id"], json!(4));
        assert_eq!(replies[0]["error"]["code"], json!(-32600));
        assert_eq!(replies[1]["id"], json!("x"));
        assert_eq!(replies[1]["error"]["code"], json!(-32600));
        host.send(r#"{"jsonrpc":"2.0","id":1,"result":null}"#).await;
        assert_eq!(run.await.unwrap(), Ok(Value::Null));
    }

    #[tokio::test]
    async fn other_broken_messages_end_the_connection() {
        assert!(
            broken_by(&frame(r#"{"jsonrpc":"2.0","result":1}"#))
                .await
                .starts_with("the node host sent a message that is not JSON-RPC 2.0")
        );
        assert!(
            broken_by(&frame(r#"[{"jsonrpc":"2.0","id":1,"result":1}]"#))
                .await
                .contains("batches are not used")
        );
        assert_eq!(
            broken_by(&frame(
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"bad frame"}}"#
            ))
            .await,
            "the node host could not read a message: bad frame"
        );
        assert!(
            broken_by(b"Content-Length: x\r\n\r\n{}")
                .await
                .starts_with("the node host sent a malformed frame")
        );
    }

    /// A host input that is already closed.
    struct BrokenPipe;

    impl AsyncWrite for BrokenPipe {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_closed_input_is_closed() {
        let (engine_reads, _host_writes) = duplex(64);
        let connection = Connection::start(engine_reads, BrokenPipe, Arc::new(NoIncoming));
        assert_eq!(
            connection.request("describe", None).await,
            Err(ConnectionError::Closed)
        );
        assert!(!connection.is_open());
        // The id was spent: ids never repeat.
        assert_eq!(connection.state().next_id, 2);
    }

    #[tokio::test]
    async fn closing_the_input_ends_the_hosts_input_only() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let pending = spawn_request(&connection, "run", None);
        host.read().await.unwrap();
        connection.close_input().await;
        assert_eq!(host.read().await, None);
        assert_eq!(
            connection.notify("exit", None).await,
            Err(ConnectionError::Closed)
        );
        // Responses are still read.
        assert!(connection.is_open());
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": 3}))
            .await;
        assert_eq!(pending.await.unwrap(), Ok(json!(3)));
    }

    #[tokio::test]
    async fn a_cancellable_request_sends_cancel_once_and_keeps_waiting() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let cancel = Cancel::new();
        let run = tokio::spawn({
            let connection = connection.clone();
            let cancel = cancel.clone();
            async move {
                connection
                    .request_cancellable("run", Some(json!({"run_id": "r1"})), &cancel)
                    .await
            }
        });
        assert_eq!(host.read().await.unwrap()["id"], json!(1));
        cancel.cancel();
        assert_eq!(
            host.read().await.unwrap(),
            json!({"jsonrpc": "2.0", "method": "$/cancel", "params": {"id": 1}})
        );
        cancel.cancel();
        host.send_json(json!({"jsonrpc": "2.0", "id": 1,
            "error": {"code": -32002, "message": "the run was cancelled"}}))
            .await;
        match run.await.unwrap() {
            Err(ConnectionError::Rpc(error)) => {
                assert_eq!(error.kind(), Some(ErrorCode::Cancelled));
            }
            other => panic!("{other:?}"),
        }
        // `$/cancel` went out once.
        connection.close_input().await;
        assert_eq!(host.read().await, None);
    }

    #[tokio::test]
    async fn an_uncancelled_request_sends_no_cancel() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let cancel = Cancel::new();
        let run = tokio::spawn({
            let connection = connection.clone();
            async move { connection.request_cancellable("run", None, &cancel).await }
        });
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": 1}))
            .await;
        assert_eq!(run.await.unwrap(), Ok(json!(1)));
        connection.close_input().await;
        assert_eq!(host.read().await, None);
    }

    /// Answers `slow` once the test opens the gate, and `quick` at once.
    struct Gated {
        gate: Mutex<Option<oneshot::Receiver<()>>>,
    }

    impl Incoming for Gated {
        fn request(
            &self,
            method: String,
            _params: Option<Value>,
        ) -> BoxFuture<'static, Result<Value, RpcError>> {
            let gate = if method == "slow" {
                self.gate.lock().unwrap().take()
            } else {
                None
            };
            Box::pin(async move {
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                Ok(json!(method))
            })
        }

        fn notify(&self, _method: String, _params: Option<Value>) {}
    }

    #[tokio::test]
    async fn a_slow_host_request_does_not_stop_the_reader() {
        let (open, gate) = oneshot::channel();
        let (connection, mut host) = connect(Arc::new(Gated {
            gate: Mutex::new(Some(gate)),
        }));
        let run = spawn_request(&connection, "run", None);
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "method": "slow", "params": {}}))
            .await;
        host.send_json(json!({"jsonrpc": "2.0", "id": 2, "method": "quick", "params": {}}))
            .await;
        assert_eq!(
            host.read().await.unwrap(),
            json!({"jsonrpc": "2.0", "id": 2, "result": "quick"})
        );
        // The engine's own request is answered while the slow one is pending.
        let other = spawn_request(&connection, "tool.invoke", None);
        assert_eq!(host.read().await.unwrap()["id"], json!(2));
        host.send_json(json!({"jsonrpc": "2.0", "id": 2, "result": "tool"}))
            .await;
        assert_eq!(other.await.unwrap(), Ok(json!("tool")));
        open.send(()).unwrap();
        assert_eq!(
            host.read().await.unwrap(),
            json!({"jsonrpc": "2.0", "id": 1, "result": "slow"})
        );
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": "done"}))
            .await;
        assert_eq!(run.await.unwrap(), Ok(json!("done")));
    }

    #[tokio::test]
    async fn responses_arrive_in_any_order() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let first = spawn_request(&connection, "a", None);
        host.read().await.unwrap();
        let second = spawn_request(&connection, "b", None);
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 2, "result": "b"}))
            .await;
        assert_eq!(second.await.unwrap(), Ok(json!("b")));
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": "a"}))
            .await;
        assert_eq!(first.await.unwrap(), Ok(json!("a")));
    }

    #[tokio::test]
    async fn a_late_answer_to_an_abandoned_request_is_discarded() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let waited =
            tokio::time::timeout(Duration::from_millis(20), connection.request("run", None)).await;
        assert!(waited.is_err());
        host.read().await.unwrap();
        host.send_json(json!({"jsonrpc": "2.0", "id": 1, "result": "late"}))
            .await;
        let next = spawn_request(&connection, "describe", None);
        assert_eq!(host.read().await.unwrap()["id"], json!(2));
        host.send_json(json!({"jsonrpc": "2.0", "id": 2, "result": "next"}))
            .await;
        assert_eq!(next.await.unwrap(), Ok(json!("next")));
        assert!(connection.is_open());
    }

    #[tokio::test]
    async fn pending_requests_fail_when_the_output_ends() {
        let (connection, mut host) = connect(Arc::new(NoIncoming));
        let first = spawn_request(&connection, "a", None);
        let second = spawn_request(&connection, "b", None);
        host.read().await.unwrap();
        host.read().await.unwrap();
        assert!(connection.is_open());
        host.close().await;
        connection.closed().await;
        assert!(!connection.is_open());
        assert_eq!(first.await.unwrap(), Err(ConnectionError::Closed));
        assert_eq!(second.await.unwrap(), Err(ConnectionError::Closed));
    }

    #[test]
    fn requests_are_sendable() {
        fn send<T: Send>(_: &T) {}
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (connection, _host) = connect(Arc::new(NoIncoming));
        let cancel = Cancel::new();
        send(&connection.request("a", None));
        send(&connection.request_cancellable("a", None, &cancel));
        send(&connection.notify("a", None));
        send(&connection.closed());
        send(&connection.close_input());
    }

    /// The asynchronous frame reader agrees with `grida_fx_protocol::framing::read_message`.
    #[tokio::test]
    async fn frames_read_as_the_protocol_crate_reads_them() {
        fn sync_all(bytes: &[u8]) -> Result<Vec<Vec<u8>>, (io::ErrorKind, String)> {
            let mut reader = std::io::Cursor::new(bytes.to_vec());
            let mut bodies = Vec::new();
            loop {
                match read_message(&mut reader) {
                    Ok(Some(body)) => bodies.push(body),
                    Ok(None) => return Ok(bodies),
                    Err(e) => return Err((e.kind(), e.to_string())),
                }
            }
        }
        async fn async_all(bytes: &[u8]) -> Result<Vec<Vec<u8>>, (io::ErrorKind, String)> {
            let mut reader = bytes;
            let mut bodies = Vec::new();
            loop {
                match read_frame(&mut reader).await {
                    Ok(Some(body)) => bodies.push(body),
                    Ok(None) => return Ok(bodies),
                    Err(e) => return Err((e.kind(), e.to_string())),
                }
            }
        }
        let long = format!("X: {}\r\n", "a".repeat(9000));
        let mut many = Vec::new();
        write_message(&mut many, br#"{"a":1}"#).unwrap();
        write_message(&mut many, "{\"t\":\"caf\u{e9}\"}".as_bytes()).unwrap();
        write_message(&mut many, b"").unwrap();
        let cases: Vec<&[u8]> = vec![
            b"",
            &many,
            b"content-type: application/vscode-jsonrpc; charset=utf-8\r\nCONTENT-LENGTH:  2 \r\nX-Other: 99\r\n\r\n{}",
            b"\r\n{}",
            b"X-Other: 1\r\n\r\n{}",
            b"Content-Length: two\r\n\r\n{}",
            b"Content-Length: -2\r\n\r\n{}",
            b"Content-Length: +2\r\n\r\n{}",
            b"Content-Length: \r\n\r\n{}",
            b"Content-Length: 99999999999999999999999999\r\n\r\n",
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            b"Content-Length: 2\n\n{}",
            b"Content-Length 2\r\n\r\n{}",
            b"Content-Length: 2\r\n\xff\r\n\r\n{}",
            long.as_bytes(),
            b"Content-Len",
            b"Content-Length: 2\r\n",
            b"Content-Length: 5\r\n\r\n{}",
            b"Content-Length: 2\r\n\r\n{}Content",
        ];
        for bytes in cases {
            assert_eq!(
                async_all(bytes).await,
                sync_all(bytes),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
        assert_eq!(async_all(&many).await.unwrap().len(), 3);
    }
}
