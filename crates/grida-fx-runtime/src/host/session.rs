//! A JSON-RPC session over framed streams (protocol.md §1, §2).
//!
//! The engine numbers its requests from 1. [`Session::request`] writes a request and reads
//! messages until its response arrives; requests and notifications the host sends meanwhile go to
//! the [`RequestHandler`], which may itself send requests through the session it is given (step
//! 3's `tool.invoke` while the host's `agent.run` is pending), so the loop is re-entrant. Every
//! body is parsed with the strict I-JSON reader (`grida_fx_core::value::parse_json`); a message
//! outside I-JSON is answered `-32700` when it was a request, and ends the session otherwise.
//! Responses may arrive in any order: a response for an id that is pending deeper in the stack
//! is kept until that frame returns.
//!
//! A message that is I-JSON but not JSON-RPC 2.0 is answered `-32600` when it carries a method
//! and an id, and ends the session otherwise. A response to an id the engine is not waiting for,
//! a second response to one, and an error response whose id is `null` end the session too. Once
//! the session has ended, by such a break or by the end of the host's output, every later call
//! fails the same way without touching the streams.

use grida_fx_core::value::parse_json;
use grida_fx_protocol::framing::{read_message, write_message};
use grida_fx_protocol::{ErrorCode, Id, Message, RpcError};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, Write};

/// Why a request got no result.
#[derive(Debug)]
pub enum SessionError {
    /// Reading or writing the streams failed.
    Io(io::Error),
    /// The stream ended (the host exited) before the response arrived.
    Closed,
    /// A message broke the protocol (not JSON-RPC, not I-JSON, an unknown response id).
    Protocol(String),
    /// The host answered with an error.
    Rpc(RpcError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "{e}"),
            SessionError::Closed => f.write_str("the node host closed the session"),
            SessionError::Protocol(m) => f.write_str(m),
            SessionError::Rpc(e) => f.write_str(&e.message),
        }
    }
}

impl std::error::Error for SessionError {}

/// Serves the requests and notifications a host sends while one of the engine's requests is
/// pending.
pub trait RequestHandler {
    /// Answers a host request. Step 2 answers every one with `-32601`.
    fn handle(
        &mut self,
        session: &mut Session,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, RpcError>;
    /// Takes a host notification (`progress`).
    fn notify(&mut self, _method: &str, _params: Option<Value>) {}
}

/// A handler that serves nothing: step 2's `describe` and `build` expect no host requests.
#[derive(Debug, Default)]
pub struct NoRequests;

impl RequestHandler for NoRequests {
    fn handle(
        &mut self,
        _session: &mut Session,
        method: &str,
        _params: Option<Value>,
    ) -> Result<Value, RpcError> {
        Err(RpcError::new(
            grida_fx_protocol::ErrorCode::MethodNotFound,
            format!("the engine serves no {method} here"),
        ))
    }
}

/// How a session ended. Kept so every later call fails the same way.
#[derive(Debug, Clone)]
enum Ended {
    Closed,
    Broken(String),
    Io(io::ErrorKind, String),
}

impl Ended {
    fn error(&self) -> SessionError {
        match self {
            Ended::Closed => SessionError::Closed,
            Ended::Broken(message) => SessionError::Protocol(message.clone()),
            Ended::Io(kind, message) => SessionError::Io(io::Error::new(*kind, message.clone())),
        }
    }
}

/// One session.
pub struct Session {
    reader: Box<dyn BufRead + Send>,
    writer: Box<dyn Write + Send>,
    next_id: i64,
    /// Responses read for requests pending further up the stack.
    parked: HashMap<Id, Result<Value, RpcError>>,
    /// The engine's requests waiting for a response, outermost first.
    pending: Vec<Id>,
    /// Set once the session has ended.
    ended: Option<Ended>,
}

impl Session {
    pub fn new(reader: Box<dyn BufRead + Send>, writer: Box<dyn Write + Send>) -> Session {
        Session {
            reader,
            writer,
            next_id: 1,
            parked: HashMap::new(),
            pending: Vec::new(),
            ended: None,
        }
    }

    /// Sends a request and waits for its result, serving host messages meanwhile.
    pub fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
        handler: &mut dyn RequestHandler,
    ) -> Result<Value, SessionError> {
        self.check_open()?;
        let id = Id::Number(self.next_id);
        self.next_id += 1;
        self.send(&Message::Request {
            id: id.clone(),
            method: method.to_string(),
            params,
        })?;
        self.pending.push(id.clone());
        let outcome = self.wait(&id, handler);
        self.pending.retain(|pending| pending != &id);
        self.parked.remove(&id);
        outcome
    }

    /// Sends a notification (`exit`, `$/cancel`).
    pub fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), SessionError> {
        self.check_open()?;
        self.send(&Message::Notification {
            method: method.to_string(),
            params,
        })
    }

    /// Whether the session can still carry messages: it has neither seen the end of the host's
    /// output nor a message that broke the protocol.
    pub fn is_open(&self) -> bool {
        self.ended.is_none()
    }

    /// Reads and serves messages until the response to `id` is parked.
    fn wait(&mut self, id: &Id, handler: &mut dyn RequestHandler) -> Result<Value, SessionError> {
        loop {
            if let Some(answer) = self.parked.remove(id) {
                return answer.map_err(SessionError::Rpc);
            }
            self.check_open()?;
            self.serve_one(handler)?;
        }
    }

    /// Reads one message and acts on it.
    fn serve_one(&mut self, handler: &mut dyn RequestHandler) -> Result<(), SessionError> {
        let body = match read_message(&mut self.reader) {
            Ok(Some(body)) => body,
            Ok(None) => return Err(self.end(Ended::Closed)),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(self.end(Ended::Closed));
            }
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                return Err(self.end(Ended::Broken(format!(
                    "the node host sent a malformed frame: {e}"
                ))));
            }
            Err(e) => return Err(self.end(Ended::Io(e.kind(), e.to_string()))),
        };
        let parsed = match std::str::from_utf8(&body) {
            Ok(text) => parse_json(text).map_err(|refused| refused.message),
            Err(_) => Err("it is not UTF-8".to_string()),
        };
        let value = match parsed {
            Ok(value) => value,
            Err(reason) => {
                return match lenient_request_id(&body) {
                    Some(id) => self.send(&Message::Error {
                        id: Some(id),
                        error: RpcError::new(
                            ErrorCode::ParseError,
                            format!("the message is not I-JSON: {reason}"),
                        ),
                    }),
                    None => Err(self.end(Ended::Broken(format!(
                        "the node host sent a message that is not I-JSON: {reason}"
                    )))),
                };
            }
        };
        let request_id = request_id_of(&value);
        let message = match Message::from_value(value) {
            Ok(message) => message,
            Err(error) => {
                return match request_id {
                    Some(id) => self.send(&Message::Error {
                        id: Some(id),
                        error,
                    }),
                    None => Err(self.end(Ended::Broken(format!(
                        "the node host sent a message that is not JSON-RPC 2.0: {}",
                        error.message
                    )))),
                };
            }
        };
        match message {
            Message::Request { id, method, params } => {
                let answer = handler.handle(self, &method, params);
                // A request the handler made may have ended the session.
                self.check_open()?;
                let reply = match answer {
                    Ok(result) => Message::Response { id, result },
                    Err(error) => Message::Error {
                        id: Some(id),
                        error,
                    },
                };
                self.send(&reply)
            }
            Message::Notification { method, params } => {
                handler.notify(&method, params);
                Ok(())
            }
            Message::Response { id, result } => self.deliver(id, Ok(result)),
            Message::Error {
                id: Some(id),
                error,
            } => self.deliver(id, Err(error)),
            Message::Error { id: None, error } => Err(self.end(Ended::Broken(format!(
                "the node host could not read a message: {}",
                error.message
            )))),
        }
    }

    /// Parks a response for one of the engine's pending requests.
    fn deliver(&mut self, id: Id, answer: Result<Value, RpcError>) -> Result<(), SessionError> {
        if !self.pending.contains(&id) {
            return Err(self.end(Ended::Broken(format!(
                "the node host answered request {id}, which is not pending"
            ))));
        }
        if self.parked.contains_key(&id) {
            return Err(self.end(Ended::Broken(format!(
                "the node host answered request {id} twice"
            ))));
        }
        self.parked.insert(id, answer);
        Ok(())
    }

    /// Writes one message.
    fn send(&mut self, message: &Message) -> Result<(), SessionError> {
        let body = serde_json::to_vec(&message.to_value())
            .map_err(|e| SessionError::Protocol(format!("a message cannot be written: {e}")))?;
        match write_message(&mut self.writer, &body) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Err(self.end(Ended::Closed)),
            Err(e) => Err(self.end(Ended::Io(e.kind(), e.to_string()))),
        }
    }

    fn check_open(&self) -> Result<(), SessionError> {
        match &self.ended {
            Some(ended) => Err(ended.error()),
            None => Ok(()),
        }
    }

    /// Ends the session (the first reason wins) and returns the error for it.
    fn end(&mut self, ended: Ended) -> SessionError {
        self.ended.get_or_insert(ended).error()
    }
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

/// The request id of a body the strict reader refused, read leniently; `None` when the body is
/// not a request even to a lenient reader.
fn lenient_request_id(body: &[u8]) -> Option<Id> {
    let value: Value = serde_json::from_slice(body).ok()?;
    request_id_of(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    /// A writer whose bytes the test can read after the session took it.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        /// Every message written so far.
        fn messages(&self) -> Vec<Value> {
            let bytes = self.0.lock().unwrap().clone();
            let mut reader = Cursor::new(bytes);
            let mut messages = Vec::new();
            while let Some(body) = read_message(&mut reader).unwrap() {
                messages.push(serde_json::from_slice(&body).unwrap());
            }
            messages
        }
    }

    fn frames(bodies: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for body in bodies {
            write_message(&mut bytes, body.as_bytes()).unwrap();
        }
        bytes
    }

    fn scripted(host_says: &[&str]) -> (Session, Shared) {
        let out = Shared::default();
        let session = Session::new(
            Box::new(Cursor::new(frames(host_says))),
            Box::new(out.clone()),
        );
        (session, out)
    }

    fn json(value: Value) -> String {
        serde_json::to_string(&value).unwrap()
    }

    #[test]
    fn ids_count_from_one() {
        let (mut session, out) = scripted(&[
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": "a"})),
            &json(json!({"jsonrpc": "2.0", "id": 2, "result": {"b": [1]}})),
        ]);
        assert_eq!(
            session
                .request("describe", Some(json!({"targets": []})), &mut NoRequests)
                .unwrap(),
            json!("a")
        );
        session.notify("progress", None).unwrap();
        assert_eq!(
            session.request("shutdown", None, &mut NoRequests).unwrap(),
            json!({"b": [1]})
        );
        assert_eq!(
            out.messages(),
            vec![
                json!({"jsonrpc": "2.0", "id": 1, "method": "describe", "params": {"targets": []}}),
                json!({"jsonrpc": "2.0", "method": "progress"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
            ]
        );
        // The bytes on the wire are compact JSON with jsonrpc first.
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"describe","params":{"targets":[]}}"#;
        let wire = out.0.lock().unwrap().clone();
        let mut expected = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        expected.extend_from_slice(body.as_bytes());
        assert!(wire.starts_with(&expected));
    }

    #[test]
    fn errors_reach_the_caller() {
        let (mut session, _) = scripted(&[&json(json!({
            "jsonrpc": "2.0", "id": 1,
            "error": {"code": -32004, "message": "nodes/x.py has no function build"}
        }))]);
        match session.request("build", None, &mut NoRequests) {
            Err(SessionError::Rpc(error)) => {
                assert_eq!(error.kind(), Some(ErrorCode::LoadFailed));
                assert_eq!(error.message, "nodes/x.py has no function build");
            }
            other => panic!("{other:?}"),
        }
        // The session goes on after an error response.
        assert!(session.is_open());
    }

    /// Records host notifications; answers requests with their method, or refuses them.
    #[derive(Default)]
    struct Recorder {
        notes: Vec<(String, Option<Value>)>,
        requests: Vec<String>,
    }

    impl RequestHandler for Recorder {
        fn handle(
            &mut self,
            _session: &mut Session,
            method: &str,
            _params: Option<Value>,
        ) -> Result<Value, RpcError> {
            self.requests.push(method.to_string());
            if method == "fact" {
                return Ok(json!({}));
            }
            Err(RpcError::new(ErrorCode::InvalidParams, "no"))
        }

        fn notify(&mut self, method: &str, params: Option<Value>) {
            self.notes.push((method.to_string(), params));
        }
    }

    #[test]
    fn host_messages_are_served_while_waiting() {
        let (mut session, out) = scripted(&[
            &json(
                json!({"jsonrpc": "2.0", "id": 1, "method": "fact", "params": {"run_id": "r1", "name": "n", "value": 1}}),
            ),
            &json(
                json!({"jsonrpc": "2.0", "method": "progress", "params": {"run_id": "r1", "text": "half"}}),
            ),
            &json(json!({"jsonrpc": "2.0", "id": "h2", "method": "annotate", "params": {}})),
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": {"outputs": {}}})),
        ]);
        let mut recorder = Recorder::default();
        assert_eq!(
            session
                .request("run", Some(json!({})), &mut recorder)
                .unwrap(),
            json!({"outputs": {}})
        );
        assert_eq!(recorder.requests, ["fact", "annotate"]);
        assert_eq!(
            recorder.notes,
            vec![(
                "progress".to_string(),
                Some(json!({"run_id": "r1", "text": "half"}))
            )]
        );
        let written = out.messages();
        assert_eq!(written.len(), 3);
        assert_eq!(written[1], json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
        assert_eq!(
            written[2],
            json!({"jsonrpc": "2.0", "id": "h2", "error": {"code": -32602, "message": "no"}})
        );
    }

    #[test]
    fn no_requests_answers_method_not_found() {
        let (mut session, out) = scripted(&[
            &json(json!({"jsonrpc": "2.0", "id": 1, "method": "capability", "params": {}})),
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": null})),
        ]);
        assert_eq!(
            session.request("describe", None, &mut NoRequests).unwrap(),
            Value::Null
        );
        let reply = &out.messages()[1];
        assert_eq!(reply["id"], json!(1));
        assert_eq!(reply["error"]["code"], json!(-32601));
        assert_eq!(
            reply["error"]["message"],
            json!("the engine serves no capability here")
        );
    }

    /// Sends `tool.invoke` back through the session when the host asks for `agent.run`.
    struct Nesting {
        inner: Option<Result<Value, String>>,
    }

    impl RequestHandler for Nesting {
        fn handle(
            &mut self,
            session: &mut Session,
            method: &str,
            _params: Option<Value>,
        ) -> Result<Value, RpcError> {
            assert_eq!(method, "agent.run");
            let answer = session.request("tool.invoke", Some(json!({"name": "look"})), self);
            self.inner = Some(answer.as_ref().map(Value::clone).map_err(|e| e.to_string()));
            answer.map_err(|e| RpcError::new(ErrorCode::NodeFailure, e.to_string()))
        }
    }

    #[test]
    fn nested_requests_park_outer_responses() {
        let (mut session, out) = scripted(&[
            // The host asks for an agent while the engine's run (1) is pending.
            &json(json!({"jsonrpc": "2.0", "id": 7, "method": "agent.run", "params": {}})),
            // It answers the outer run first, then the inner tool.invoke (2).
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": "outer"})),
            &json(json!({"jsonrpc": "2.0", "id": 2, "result": {"content": "a cat"}})),
        ]);
        let mut handler = Nesting { inner: None };
        assert_eq!(
            session.request("run", None, &mut handler).unwrap(),
            json!("outer")
        );
        assert_eq!(handler.inner, Some(Ok(json!({"content": "a cat"}))));
        let written = out.messages();
        assert_eq!(written[0]["id"], json!(1));
        assert_eq!(written[0]["method"], json!("run"));
        assert_eq!(written[1]["id"], json!(2));
        assert_eq!(written[1]["method"], json!("tool.invoke"));
        assert_eq!(
            written[2],
            json!({"jsonrpc": "2.0", "id": 7, "result": {"content": "a cat"}})
        );
        assert!(session.parked.is_empty());
        assert!(session.pending.is_empty());
    }

    #[test]
    fn a_session_that_ends_inside_a_nested_request_ends_the_outer_one() {
        let (mut session, out) = scripted(&[&json(
            json!({"jsonrpc": "2.0", "id": 7, "method": "agent.run", "params": {}}),
        )]);
        let mut handler = Nesting { inner: None };
        assert!(matches!(
            session.request("run", None, &mut handler),
            Err(SessionError::Closed)
        ));
        assert!(matches!(handler.inner, Some(Err(_))));
        // Nothing was answered to the host after the stream ended.
        assert_eq!(out.messages().len(), 2);
    }

    #[test]
    fn end_of_stream_is_closed() {
        let (mut session, _) = scripted(&[]);
        assert!(matches!(
            session.request("describe", None, &mut NoRequests),
            Err(SessionError::Closed)
        ));
        assert!(!session.is_open());
        assert!(matches!(
            session.notify("exit", None),
            Err(SessionError::Closed)
        ));

        let mut truncated = frames(&[]);
        truncated.extend_from_slice(b"Content-Length: 40\r\n\r\n{\"jsonrpc\"");
        let mut session = Session::new(
            Box::new(Cursor::new(truncated)),
            Box::new(Shared::default()),
        );
        assert!(matches!(
            session.request("describe", None, &mut NoRequests),
            Err(SessionError::Closed)
        ));
    }

    fn protocol_error(session: &mut Session) -> String {
        match session.request("describe", None, &mut NoRequests) {
            Err(SessionError::Protocol(message)) => message,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_and_repeated_response_ids_break_the_session() {
        let (mut session, out) =
            scripted(&[&json(json!({"jsonrpc": "2.0", "id": 5, "result": null}))]);
        assert_eq!(
            protocol_error(&mut session),
            "the node host answered request 5, which is not pending"
        );
        // Ended: the next call fails the same way and writes nothing.
        assert_eq!(
            protocol_error(&mut session),
            "the node host answered request 5, which is not pending"
        );
        assert_eq!(out.messages().len(), 1);

        let (mut session, _) = scripted(&[
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": 1})),
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": 2})),
        ]);
        assert_eq!(
            session.request("a", None, &mut NoRequests).unwrap(),
            json!(1)
        );
        assert_eq!(
            protocol_error(&mut session),
            "the node host answered request 1, which is not pending"
        );
    }

    #[test]
    fn a_second_answer_to_a_parked_request_breaks_the_session() {
        let (mut session, _) = scripted(&[
            &json(json!({"jsonrpc": "2.0", "id": 7, "method": "agent.run", "params": {}})),
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": "outer"})),
            &json(json!({"jsonrpc": "2.0", "id": 1, "result": "again"})),
        ]);
        let mut handler = Nesting { inner: None };
        assert!(matches!(
            session.request("run", None, &mut handler),
            Err(SessionError::Protocol(m)) if m == "the node host answered request 1 twice"
        ));
    }

    #[test]
    fn a_request_outside_ijson_is_answered_parse_error() {
        let (mut session, out) = scripted(&[
            r#"{"jsonrpc":"2.0","id":3,"method":"fact","params":{"a":1,"a":2}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"fact","params":{"n":9007199254740993}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":true}"#,
        ]);
        assert_eq!(
            session.request("run", None, &mut NoRequests).unwrap(),
            json!(true)
        );
        let written = out.messages();
        for (reply, id) in written[1..].iter().zip([3, 4]) {
            assert_eq!(reply["id"], json!(id));
            assert_eq!(reply["error"]["code"], json!(-32700));
            assert!(
                reply["error"]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("the message is not I-JSON: ")
            );
        }
    }

    #[test]
    fn a_response_outside_ijson_ends_the_session() {
        let (mut session, _) = scripted(&[r#"{"jsonrpc":"2.0","id":1,"result":{"a":1,"a":2}}"#]);
        assert!(
            protocol_error(&mut session)
                .starts_with("the node host sent a message that is not I-JSON")
        );
        let (mut session, _) = scripted(&["not json"]);
        assert!(
            protocol_error(&mut session)
                .starts_with("the node host sent a message that is not I-JSON")
        );
        let mut bytes = Vec::new();
        write_message(
            &mut bytes,
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"\xff\"}",
        )
        .unwrap();
        let mut session = Session::new(Box::new(Cursor::new(bytes)), Box::new(Shared::default()));
        assert!(protocol_error(&mut session).contains("not UTF-8"));
    }

    #[test]
    fn an_invalid_request_is_answered_invalid_request() {
        let (mut session, out) = scripted(&[
            r#"{"id":4,"method":"fact"}"#,
            r#"{"jsonrpc":"2.0","id":"x","method":"fact","params":7}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":null}"#,
        ]);
        session.request("run", None, &mut NoRequests).unwrap();
        let written = out.messages();
        assert_eq!(written[1]["id"], json!(4));
        assert_eq!(written[1]["error"]["code"], json!(-32600));
        assert_eq!(written[2]["id"], json!("x"));
        assert_eq!(written[2]["error"]["code"], json!(-32600));
    }

    #[test]
    fn other_broken_messages_end_the_session() {
        let (mut session, _) = scripted(&[r#"{"jsonrpc":"2.0","result":1}"#]);
        assert!(
            protocol_error(&mut session)
                .starts_with("the node host sent a message that is not JSON-RPC 2.0")
        );
        let (mut session, _) = scripted(&[r#"[{"jsonrpc":"2.0","id":1,"result":1}]"#]);
        assert!(protocol_error(&mut session).contains("batches are not used"));
        let (mut session, _) = scripted(&[
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"bad frame"}}"#,
        ]);
        assert_eq!(
            protocol_error(&mut session),
            "the node host could not read a message: bad frame"
        );
        let mut bytes = b"Content-Length: x\r\n\r\n".to_vec();
        bytes.extend_from_slice(b"{}");
        let mut session = Session::new(Box::new(Cursor::new(bytes)), Box::new(Shared::default()));
        assert!(protocol_error(&mut session).starts_with("the node host sent a malformed frame"));
    }

    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_input_is_closed() {
        let mut session = Session::new(Box::new(Cursor::new(Vec::new())), Box::new(BrokenPipe));
        assert!(matches!(
            session.request("describe", None, &mut NoRequests),
            Err(SessionError::Closed)
        ));
        // The id was spent: ids never repeat.
        assert_eq!(session.next_id, 2);
    }
}
