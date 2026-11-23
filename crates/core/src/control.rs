//! Types shared between the control server (`tag-control`) and the app loop.
//!
//! They live in `tag-core` so [`crate::event::AppEvent`] can carry control requests without
//! `tag-core` depending on `tag-control`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// Permission tier of a control method (§4.5 of the orchestration roadmap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Inspect state only.
    Read,
    /// Change state (spawn, send input, create worktrees, run tests).
    Act,
    /// Irreversible or high-impact (merge, remove, kill, cancel, approve).
    Dangerous,
}

/// Who a control client is acting as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// A client without a token (a human's CLI, an external assistant via MCP).
    External,
    /// A conductor agent launched by TAG for one run.
    Conductor,
    /// A worker agent launched by TAG for one task.
    Worker,
}

/// Identity and scope of an authenticated control connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCtx {
    /// Client name sent in `tag.hello` (e.g. `claude-desktop`, `mcp`, `ctl`).
    pub client: String,
    pub role: Role,
    pub tiers: Vec<Tier>,
    /// Run the token is bound to (conductor/worker).
    pub run: Option<String>,
    /// Task the token is bound to (worker).
    pub task: Option<String>,
    /// Pane the token is bound to, once known.
    pub pane: Option<u32>,
}

impl ClientCtx {
    /// Whether this client may call methods of `tier`.
    pub fn allows(&self, tier: Tier) -> bool {
        self.tiers.contains(&tier)
    }
}

/// JSON-RPC error codes used by the control plane (§4.3).
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const NOT_AUTHENTICATED: i64 = -32001;
    pub const FORBIDDEN: i64 = -32002;
    pub const NOT_FOUND: i64 = -32003;
    pub const CONFLICT: i64 = -32004;
    pub const BUSY: i64 = -32005;
    pub const TIMEOUT: i64 = -32006;
    pub const POLICY_BLOCKED: i64 = -32007;
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ControlError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(codes::INVALID_PARAMS, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(codes::NOT_FOUND, message)
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(codes::FORBIDDEN, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(codes::INTERNAL_ERROR, message)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(codes::CONFLICT, message)
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(codes::BUSY, message)
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(codes::TIMEOUT, message)
    }
    pub fn policy_blocked(message: impl Into<String>) -> Self {
        Self::new(codes::POLICY_BLOCKED, message)
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ControlError {}

/// Result of handling a control request: a JSON result value or an error.
pub type ControlResult = Result<serde_json::Value, ControlError>;

/// One-shot reply channel for a control request, clonable so it can ride in `AppEvent`.
///
/// The first [`ReplyHandle::send`] wins; later sends are ignored.
#[derive(Clone)]
pub struct ReplyHandle(Arc<Mutex<Option<oneshot::Sender<ControlResult>>>>);

impl ReplyHandle {
    /// Create a handle and the receiver the server awaits.
    pub fn new() -> (Self, oneshot::Receiver<ControlResult>) {
        let (tx, rx) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(tx)))), rx)
    }

    /// Deliver the result. Returns false if a reply was already sent or the client went away.
    pub fn send(&self, result: ControlResult) -> bool {
        let sender = self.0.lock().ok().and_then(|mut g| g.take());
        match sender {
            Some(tx) => tx.send(result).is_ok(),
            None => false,
        }
    }
}

impl fmt::Debug for ReplyHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplyHandle")
    }
}

/// An authenticated request forwarded from the control server to the app loop.
#[derive(Debug, Clone)]
pub struct ControlEnvelope {
    pub client: ClientCtx,
    pub method: String,
    pub params: serde_json::Value,
    pub reply: ReplyHandle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_handle_first_send_wins() {
        let (h, mut rx) = ReplyHandle::new();
        let h2 = h.clone();
        assert!(h.send(Ok(serde_json::json!(1))));
        assert!(!h2.send(Ok(serde_json::json!(2))));
        assert_eq!(rx.try_recv().unwrap().unwrap(), serde_json::json!(1));
    }

    #[test]
    fn tiers_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&Tier::Dangerous).unwrap(),
            "\"dangerous\""
        );
        assert_eq!(
            serde_json::to_string(&Role::External).unwrap(),
            "\"external\""
        );
    }
}
