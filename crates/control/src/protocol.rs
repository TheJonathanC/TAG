//! JSON-RPC 2.0 envelopes and newline framing (§4.1).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tag_core::control::{codes, ControlError, ControlResult};
use tag_core::event::StateEvent;

/// Maximum accepted line length (one request), in bytes.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Name of the server→client notification carrying a [`StateEvent`].
pub const EVENT_NOTIFICATION: &str = "event";

/// A JSON-RPC request (or a client notification when `id` is absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Some(Value::from(id)),
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

impl Response {
    pub fn from_result(id: Value, result: ControlResult) -> Self {
        match result {
            Ok(v) => Self {
                jsonrpc: "2.0".into(),
                id,
                result: Some(v),
                error: None,
            },
            Err(e) => Self::error(id, e),
        }
    }

    pub fn error(id: Value, error: ControlError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Convert back into a result (`null` result when neither field is set).
    pub fn into_result(self) -> ControlResult {
        match self.error {
            Some(e) => Err(e),
            None => Ok(self.result.unwrap_or(Value::Null)),
        }
    }
}

/// A server→client notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

impl Notification {
    pub fn event(event: &StateEvent) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: EVENT_NOTIFICATION.into(),
            params: serde_json::to_value(event).unwrap_or(Value::Null),
        }
    }
}

/// Any message a client may receive.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response(Response),
    Notification(Notification),
}

impl Incoming {
    /// Parse one line from the server.
    pub fn parse(line: &str) -> Result<Self, ControlError> {
        let v: Value = serde_json::from_str(line)
            .map_err(|e| ControlError::new(codes::PARSE_ERROR, format!("bad server line: {e}")))?;
        if v.get("id").is_some() && (v.get("result").is_some() || v.get("error").is_some()) {
            serde_json::from_value(v)
                .map(Incoming::Response)
                .map_err(|e| ControlError::new(codes::PARSE_ERROR, e.to_string()))
        } else {
            serde_json::from_value(v)
                .map(Incoming::Notification)
                .map_err(|e| ControlError::new(codes::PARSE_ERROR, e.to_string()))
        }
    }
}

/// Parse one request line. On failure returns the error response to send (with `id: null`
/// unless an id could be recovered).
pub fn parse_request(line: &str) -> Result<Request, Box<Response>> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Err(Box::new(Response::error(
                Value::Null,
                ControlError::new(codes::PARSE_ERROR, format!("parse error: {e}")),
            )))
        }
    };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let req: Request = serde_json::from_value(v).map_err(|e| {
        Box::new(Response::error(
            id.clone(),
            ControlError::new(codes::INVALID_REQUEST, format!("invalid request: {e}")),
        ))
    })?;
    if req.jsonrpc != "2.0" {
        return Err(Box::new(Response::error(
            id,
            ControlError::new(codes::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
        )));
    }
    Ok(req)
}

/// Serialize a message as one `\n`-terminated line.
pub fn to_line<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}
