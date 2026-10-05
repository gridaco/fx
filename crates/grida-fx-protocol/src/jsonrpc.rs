//! JSON-RPC 2.0 messages and FX's error codes (protocol.md §1 "Messages", §7).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// A request id: an integer or a string, unique per sender for the session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    Text(String),
}

/// One message, classified (protocol.md §1 "Both directions"): with `method` and `id` a request,
/// with `method` and no `id` a notification, without `method` a response or an error response.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        id: Id,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Response {
        id: Id,
        result: Value,
    },
    Error {
        id: Option<Id>,
        error: RpcError,
    },
}

impl Message {
    /// Classifies a parsed JSON value. Refuses a value that is not a JSON-RPC 2.0 message
    /// (`jsonrpc` other than `"2.0"`, a batch, both `result` and `error`, …) with `-32600`.
    ///
    /// The members are held to fx-node-protocol-v1's `request`, `notification`, `response` and
    /// `error_response`: no member besides `jsonrpc`, `id`, `method`, `params`, `result` and
    /// `error`; an id is an integer or a string (an error response's may be `null`); `params`
    /// is an object, an array or `null`; an error is `{code, message, data?}` with an integer
    /// code (any integer, so a newer host's code survives), a non-empty message and an object
    /// `data`. Which methods exist and what their params hold is the receiver's business.
    pub fn from_value(value: Value) -> Result<Message, RpcError> {
        let mut map = match value {
            Value::Object(map) => map,
            Value::Array(_) => {
                return Err(invalid("a batch is not a message: batches are not used"));
            }
            other => {
                return Err(invalid(format!(
                    "a message is a JSON object, not {}",
                    json_type(&other)
                )));
            }
        };
        match map.get("jsonrpc") {
            Some(Value::String(version)) if version == "2.0" => {}
            Some(_) => return Err(invalid("a message's jsonrpc member must be \"2.0\"")),
            None => return Err(invalid("a message has no jsonrpc member")),
        }
        if let Some(key) = map.keys().find(|key| !MEMBERS.contains(&key.as_str())) {
            return Err(invalid(format!("a message has an unknown member {key:?}")));
        }
        if let Some(method) = map.remove("method") {
            let Value::String(method) = method else {
                return Err(invalid("a message's method must be a string"));
            };
            if map.contains_key("result") || map.contains_key("error") {
                return Err(invalid(
                    "a request or notification has no result or error member",
                ));
            }
            let params = match map.remove("params") {
                None => None,
                Some(params @ (Value::Object(_) | Value::Array(_) | Value::Null)) => Some(params),
                Some(other) => {
                    return Err(invalid(format!(
                        "params must be an object, an array or null, not {}",
                        json_type(&other)
                    )));
                }
            };
            return Ok(match map.remove("id") {
                None => Message::Notification { method, params },
                Some(id) => Message::Request {
                    id: Id::from_value(&id)?,
                    method,
                    params,
                },
            });
        }
        if map.contains_key("params") {
            return Err(invalid("a response has no params member"));
        }
        let Some(id) = map.remove("id") else {
            return Err(invalid("a response has no id"));
        };
        match (map.remove("result"), map.remove("error")) {
            (Some(_), Some(_)) => Err(invalid("a response has both result and error")),
            (None, None) => Err(invalid("a message has neither method, result nor error")),
            (Some(result), None) => {
                if id.is_null() {
                    return Err(invalid("a successful response's id cannot be null"));
                }
                Ok(Message::Response {
                    id: Id::from_value(&id)?,
                    result,
                })
            }
            (None, Some(error)) => Ok(Message::Error {
                id: if id.is_null() {
                    None
                } else {
                    Some(Id::from_value(&id)?)
                },
                error: RpcError::from_value(error)?,
            }),
        }
    }

    /// The message as JSON, with `"jsonrpc": "2.0"` first.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("jsonrpc".into(), Value::String("2.0".into()));
        match self {
            Message::Request { id, method, params } => {
                map.insert("id".into(), id.to_value());
                map.insert("method".into(), Value::String(method.clone()));
                if let Some(params) = params {
                    map.insert("params".into(), params.clone());
                }
            }
            Message::Notification { method, params } => {
                map.insert("method".into(), Value::String(method.clone()));
                if let Some(params) = params {
                    map.insert("params".into(), params.clone());
                }
            }
            Message::Response { id, result } => {
                map.insert("id".into(), id.to_value());
                map.insert("result".into(), result.clone());
            }
            Message::Error { id, error } => {
                map.insert("id".into(), id.as_ref().map_or(Value::Null, Id::to_value));
                map.insert("error".into(), error.to_value());
            }
        }
        Value::Object(map)
    }

    /// The id of a request, a response or an error response; `None` for a notification and an
    /// error response whose id is `null`.
    pub fn id(&self) -> Option<&Id> {
        match self {
            Message::Request { id, .. } | Message::Response { id, .. } => Some(id),
            Message::Error { id, .. } => id.as_ref(),
            Message::Notification { .. } => None,
        }
    }
}

/// The members a message may have.
const MEMBERS: [&str; 6] = ["jsonrpc", "id", "method", "params", "result", "error"];

/// The largest integer every double represents exactly, 2^53.
const EXACT: f64 = 9_007_199_254_740_992.0;

impl Id {
    /// Reads an id: an integer (a number with no fraction, within i64) or a string.
    pub fn from_value(value: &Value) -> Result<Id, RpcError> {
        match value {
            Value::String(text) => Ok(Id::Text(text.clone())),
            Value::Number(number) => {
                if let Some(n) = number.as_i64() {
                    return Ok(Id::Number(n));
                }
                match number.as_f64() {
                    Some(x) if x.fract() == 0.0 && x.abs() <= EXACT => Ok(Id::Number(x as i64)),
                    _ => Err(invalid(format!("an id must be an integer, not {number}"))),
                }
            }
            other => Err(invalid(format!(
                "an id is an integer or a string, not {}",
                json_type(other)
            ))),
        }
    }

    /// The id as JSON.
    pub fn to_value(&self) -> Value {
        match self {
            Id::Number(n) => Value::Number((*n).into()),
            Id::Text(text) => Value::String(text.clone()),
        }
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Id::Number(n) => write!(f, "{n}"),
            Id::Text(text) => write!(f, "{text:?}"),
        }
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidRequest, message)
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// FX's and JSON-RPC's error codes (protocol.md §7 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    NodeFailure,
    NodeError,
    Cancelled,
    ProtocolMismatch,
    LoadFailed,
    BuildFailed,
    CapabilityUndeclared,
    OverBound,
    NoRoute,
    NotLive,
    CeilingExceeded,
    CapabilityRefused,
    CallFailed,
    JobUnsettled,
    AgentUnfinished,
    UndeclaredResource,
    ExpressionError,
    OutsideWorkDir,
    UnknownFile,
    Internal,
}

impl ErrorCode {
    /// The numeric code: -32700, -32600, -32601, -32602, -32000 … -32024, -32099.
    pub fn code(self) -> i64 {
        match self {
            ErrorCode::ParseError => -32700,
            ErrorCode::InvalidRequest => -32600,
            ErrorCode::MethodNotFound => -32601,
            ErrorCode::InvalidParams => -32602,
            ErrorCode::NodeFailure => -32000,
            ErrorCode::NodeError => -32001,
            ErrorCode::Cancelled => -32002,
            ErrorCode::ProtocolMismatch => -32003,
            ErrorCode::LoadFailed => -32004,
            ErrorCode::BuildFailed => -32005,
            ErrorCode::CapabilityUndeclared => -32010,
            ErrorCode::OverBound => -32011,
            ErrorCode::NoRoute => -32012,
            ErrorCode::NotLive => -32013,
            ErrorCode::CeilingExceeded => -32014,
            ErrorCode::CapabilityRefused => -32015,
            ErrorCode::CallFailed => -32016,
            ErrorCode::JobUnsettled => -32017,
            ErrorCode::AgentUnfinished => -32020,
            ErrorCode::UndeclaredResource => -32021,
            ErrorCode::ExpressionError => -32022,
            ErrorCode::OutsideWorkDir => -32023,
            ErrorCode::UnknownFile => -32024,
            ErrorCode::Internal => -32099,
        }
    }

    /// The code for a number; `None` for a number FX does not define.
    pub fn from_code(code: i64) -> Option<ErrorCode> {
        ErrorCode::ALL.into_iter().find(|c| c.code() == code)
    }

    /// The code's name as protocol.md §7 writes it (`node_failure`, `load_failed`, …).
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::ParseError => "parse_error",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::MethodNotFound => "method_not_found",
            ErrorCode::InvalidParams => "invalid_params",
            ErrorCode::NodeFailure => "node_failure",
            ErrorCode::NodeError => "node_error",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::ProtocolMismatch => "protocol_mismatch",
            ErrorCode::LoadFailed => "load_failed",
            ErrorCode::BuildFailed => "build_failed",
            ErrorCode::CapabilityUndeclared => "capability_undeclared",
            ErrorCode::OverBound => "over_bound",
            ErrorCode::NoRoute => "no_route",
            ErrorCode::NotLive => "not_live",
            ErrorCode::CeilingExceeded => "ceiling_exceeded",
            ErrorCode::CapabilityRefused => "capability_refused",
            ErrorCode::CallFailed => "call_failed",
            ErrorCode::JobUnsettled => "job_unsettled",
            ErrorCode::AgentUnfinished => "agent_unfinished",
            ErrorCode::UndeclaredResource => "undeclared_resource",
            ErrorCode::ExpressionError => "expression_error",
            ErrorCode::OutsideWorkDir => "outside_work_dir",
            ErrorCode::UnknownFile => "unknown_file",
            ErrorCode::Internal => "internal",
        }
    }

    /// The code for a name as protocol.md §7 writes it.
    pub fn from_name(name: &str) -> Option<ErrorCode> {
        ErrorCode::ALL.into_iter().find(|c| c.name() == name)
    }

    /// Every code, in the order of protocol.md §7 (JSON-RPC's own first).
    pub const ALL: [ErrorCode; 24] = [
        ErrorCode::ParseError,
        ErrorCode::InvalidRequest,
        ErrorCode::MethodNotFound,
        ErrorCode::InvalidParams,
        ErrorCode::NodeFailure,
        ErrorCode::NodeError,
        ErrorCode::Cancelled,
        ErrorCode::ProtocolMismatch,
        ErrorCode::LoadFailed,
        ErrorCode::BuildFailed,
        ErrorCode::CapabilityUndeclared,
        ErrorCode::OverBound,
        ErrorCode::NoRoute,
        ErrorCode::NotLive,
        ErrorCode::CeilingExceeded,
        ErrorCode::CapabilityRefused,
        ErrorCode::CallFailed,
        ErrorCode::JobUnsettled,
        ErrorCode::AgentUnfinished,
        ErrorCode::UndeclaredResource,
        ErrorCode::ExpressionError,
        ErrorCode::OutsideWorkDir,
        ErrorCode::UnknownFile,
        ErrorCode::Internal,
    ];
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A JSON-RPC error object `{code, message, data?}`. `code` keeps the raw number so an unknown
/// code from a newer host survives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.code(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The code as an [`ErrorCode`], when FX defines it.
    pub fn kind(&self) -> Option<ErrorCode> {
        ErrorCode::from_code(self.code)
    }

    /// Reads an error object `{code, message, data?}` (fx-node-protocol-v1 `error`): an integer
    /// code, a non-empty message, an object `data`, and no other member. Refused with `-32600`.
    pub fn from_value(value: Value) -> Result<RpcError, RpcError> {
        let Value::Object(mut map) = value else {
            return Err(invalid("an error is an object {code, message, data?}"));
        };
        if let Some(key) = map
            .keys()
            .find(|key| !matches!(key.as_str(), "code" | "message" | "data"))
        {
            return Err(invalid(format!("an error has an unknown member {key:?}")));
        }
        let code = match map.remove("code") {
            Some(Value::Number(number)) => match Id::from_value(&Value::Number(number)) {
                Ok(Id::Number(code)) => code,
                _ => return Err(invalid("an error's code must be an integer")),
            },
            Some(_) => return Err(invalid("an error's code must be an integer")),
            None => return Err(invalid("an error has no code")),
        };
        let message = match map.remove("message") {
            Some(Value::String(message)) if !message.is_empty() => message,
            Some(Value::String(_)) => return Err(invalid("an error's message is empty")),
            Some(_) => return Err(invalid("an error's message must be a string")),
            None => return Err(invalid("an error has no message")),
        };
        let data = match map.remove("data") {
            None => None,
            Some(data @ Value::Object(_)) => Some(data),
            Some(_) => return Err(invalid("an error's data must be an object")),
        };
        Ok(RpcError {
            code,
            message,
            data,
        })
    }

    /// The error object as JSON: `{code, message, data?}`.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("code".into(), Value::Number(self.code.into()));
        map.insert("message".into(), Value::String(self.message.clone()));
        if let Some(data) = &self.data {
            map.insert("data".into(), data.clone());
        }
        Value::Object(map)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RpcError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(value: Value) -> Message {
        Message::from_value(value).unwrap()
    }

    fn refused(value: Value) -> RpcError {
        let error = Message::from_value(value).unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidRequest), "{error:?}");
        error
    }

    #[test]
    fn classifies_the_four_shapes() {
        assert_eq!(
            message(json!({"jsonrpc": "2.0", "id": 1, "method": "describe", "params": {"a": 1}})),
            Message::Request {
                id: Id::Number(1),
                method: "describe".into(),
                params: Some(json!({"a": 1})),
            }
        );
        assert_eq!(
            message(json!({"jsonrpc": "2.0", "id": "x", "method": "shutdown"})),
            Message::Request {
                id: Id::Text("x".into()),
                method: "shutdown".into(),
                params: None,
            }
        );
        assert_eq!(
            message(json!({"jsonrpc": "2.0", "method": "exit"})),
            Message::Notification {
                method: "exit".into(),
                params: None,
            }
        );
        assert_eq!(
            message(json!({"jsonrpc": "2.0", "method": "exit", "params": null})),
            Message::Notification {
                method: "exit".into(),
                params: Some(Value::Null),
            }
        );
        assert_eq!(
            message(json!({"jsonrpc": "2.0", "id": 4, "result": null})),
            Message::Response {
                id: Id::Number(4),
                result: Value::Null,
            }
        );
        assert_eq!(
            message(json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32601, "message": "no"}})),
            Message::Error {
                id: Some(Id::Number(2)),
                error: RpcError::new(ErrorCode::MethodNotFound, "no"),
            }
        );
        assert_eq!(
            message(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "bad", "data": {"x": 1}}})
            ),
            Message::Error {
                id: None,
                error: RpcError::new(ErrorCode::ParseError, "bad").with_data(json!({"x": 1})),
            }
        );
    }

    #[test]
    fn round_trips_with_jsonrpc_first() {
        let values = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocol": "p"}}),
            json!({"jsonrpc": "2.0", "id": "a", "method": "shutdown"}),
            json!({"jsonrpc": "2.0", "method": "progress", "params": {"run_id": "r1", "text": "t"}}),
            json!({"jsonrpc": "2.0", "id": 3, "result": {"outputs": {}}}),
            json!({"jsonrpc": "2.0", "id": 3, "error": {"code": -32000, "message": "m", "data": {"facts": {}}}}),
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -31999, "message": "newer"}}),
        ];
        for value in values {
            let back = message(value.clone()).to_value();
            assert_eq!(back, value);
            assert_eq!(back.as_object().unwrap().keys().next().unwrap(), "jsonrpc");
        }
        let request = Message::Request {
            id: Id::Number(7),
            method: "build".into(),
            params: Some(json!({})),
        };
        assert_eq!(
            serde_json::to_string(&request.to_value()).unwrap(),
            r#"{"jsonrpc":"2.0","id":7,"method":"build","params":{}}"#
        );
    }

    #[test]
    fn refuses_what_is_not_a_message() {
        refused(json!([{"jsonrpc": "2.0", "method": "exit"}]));
        refused(json!("exit"));
        refused(json!({"method": "exit"}));
        refused(json!({"jsonrpc": "1.0", "method": "exit"}));
        refused(json!({"jsonrpc": 2.0, "method": "exit"}));
        refused(
            json!({"jsonrpc": "2.0", "id": 1, "result": 1, "error": {"code": 1, "message": "m"}}),
        );
        refused(json!({"jsonrpc": "2.0", "id": 1}));
        refused(json!({"jsonrpc": "2.0", "result": 1}));
        refused(json!({"jsonrpc": "2.0", "id": null, "result": 1}));
        refused(json!({"jsonrpc": "2.0", "id": 1.5, "result": 1}));
        refused(json!({"jsonrpc": "2.0", "id": true, "method": "run"}));
        refused(json!({"jsonrpc": "2.0", "id": null, "method": "run"}));
        refused(json!({"jsonrpc": "2.0", "method": 3}));
        refused(json!({"jsonrpc": "2.0", "method": "run", "params": 3}));
        refused(json!({"jsonrpc": "2.0", "method": "run", "result": 3}));
        refused(json!({"jsonrpc": "2.0", "id": 1, "result": 1, "params": {}}));
        refused(json!({"jsonrpc": "2.0", "method": "exit", "extra": 1}));
        refused(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": "x", "message": "m"}}));
        refused(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": 1.5, "message": "m"}}));
        refused(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000}}));
        refused(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": ""}}));
        refused(
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "m", "data": 1}}),
        );
        refused(
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "m", "x": 1}}),
        );
        refused(json!({"jsonrpc": "2.0", "id": 1, "error": "boom"}));
    }

    #[test]
    fn integral_numbers_are_integer_ids() {
        assert_eq!(Id::from_value(&json!(2.0)).unwrap(), Id::Number(2));
        assert_eq!(Id::from_value(&json!(-3)).unwrap(), Id::Number(-3));
        assert!(Id::from_value(&json!(1e300)).is_err());
        assert_eq!(Id::Number(5).to_string(), "5");
        assert_eq!(Id::Text("a".into()).to_string(), "\"a\"");
    }

    #[test]
    fn error_codes_and_names() {
        assert_eq!(ErrorCode::ALL.len(), 24);
        for code in ErrorCode::ALL {
            assert_eq!(ErrorCode::from_code(code.code()), Some(code));
            assert_eq!(ErrorCode::from_name(code.name()), Some(code));
        }
        assert_eq!(ErrorCode::from_code(-32603), None);
        assert_eq!(ErrorCode::from_code(-32018), None);
        assert_eq!(ErrorCode::NodeFailure.name(), "node_failure");
        assert_eq!(ErrorCode::LoadFailed.name(), "load_failed");
        assert_eq!(ErrorCode::Internal.code(), -32099);
        assert_eq!(ErrorCode::OutsideWorkDir.to_string(), "outside_work_dir");
        let unknown = RpcError {
            code: -31000,
            message: "m".into(),
            data: None,
        };
        assert_eq!(unknown.kind(), None);
    }
}
