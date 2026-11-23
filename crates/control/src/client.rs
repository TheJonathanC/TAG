//! Blocking and async clients for the control socket.

use crate::discovery::{discover_socket, TOKEN_ENV};
use crate::methods::{HelloParams, HelloResult, Method, PROTOCOL_VERSION};
use crate::protocol::{to_line, Incoming, Request, EVENT_NOTIFICATION};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tag_core::control::{ControlError, ControlResult};
use tag_core::event::StateEvent;

/// Errors from a control client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No control socket could be found.
    #[error("TAG is not running (no control socket found). Start `tag` or `tag daemon`.")]
    NotRunning,
    #[error("cannot connect to {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Control(#[from] ControlError),
    #[error("bad params/result: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connection closed by TAG")]
    Closed,
}

impl ClientError {
    /// The JSON-RPC error, if the server returned one.
    pub fn control(&self) -> Option<&ControlError> {
        match self {
            ClientError::Control(e) => Some(e),
            _ => None,
        }
    }
}

/// Blocking control client (used by `tag ctl` and `tag mcp`).
pub struct ControlClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
    hello: HelloResult,
    events: VecDeque<StateEvent>,
}

impl ControlClient {
    /// Connect to `path` and authenticate as `client` with an optional scoped token.
    pub fn connect(path: &Path, client: &str, token: Option<String>) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(|source| ClientError::Connect {
            path: path.to_path_buf(),
            source,
        })?;
        let writer = stream.try_clone()?;
        let mut c = Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
            hello: HelloResult {
                protocol: PROTOCOL_VERSION,
                tag_version: String::new(),
                client: client.into(),
                role: tag_core::control::Role::External,
                tiers: Vec::new(),
                run_scope: None,
            },
            events: VecDeque::new(),
        };
        let params = HelloParams {
            client: client.into(),
            token,
            protocol: PROTOCOL_VERSION,
        };
        c.hello = c.call::<crate::methods::Hello>(&params)?;
        Ok(c)
    }

    /// Discover the socket (`$TAG_CONTROL_SOCKET` or newest live) and connect, using
    /// `$TAG_CONTROL_TOKEN` if set.
    pub fn connect_default(client: &str) -> Result<Self, ClientError> {
        let path = discover_socket().ok_or(ClientError::NotRunning)?;
        let token = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty());
        Self::connect(&path, client, token)
    }

    /// The handshake result (role, tiers, scope).
    pub fn hello(&self) -> &HelloResult {
        &self.hello
    }

    /// Call a typed method.
    pub fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Result, ClientError> {
        let v = self.call_raw(M::NAME, serde_json::to_value(params)?)?;
        Ok(serde_json::from_value(v)?)
    }

    /// Call a method by name with raw JSON params.
    pub fn call_raw(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.writer
            .write_all(to_line(&Request::new(id, method, params)).as_bytes())?;
        self.writer.flush()?;
        loop {
            match self.read_incoming()? {
                Incoming::Response(r) if r.id == id => {
                    return r.into_result().map_err(ClientError::Control)
                }
                Incoming::Response(_) => continue,
                Incoming::Notification(n) => self.stash(n.method, n.params),
            }
        }
    }

    /// Wait for the next state event (after `events.subscribe`). `None` on timeout.
    pub fn next_event(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<StateEvent>, ClientError> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        self.reader.get_ref().set_read_timeout(timeout)?;
        let res = loop {
            match self.read_incoming() {
                Ok(Incoming::Notification(n)) => {
                    self.stash(n.method, n.params);
                    if let Some(e) = self.events.pop_front() {
                        break Ok(Some(e));
                    }
                }
                Ok(Incoming::Response(_)) => continue,
                Err(ClientError::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break Ok(None)
                }
                Err(e) => break Err(e),
            }
        };
        self.reader.get_ref().set_read_timeout(None)?;
        res
    }

    fn stash(&mut self, method: String, params: Value) {
        if method == EVENT_NOTIFICATION {
            if let Ok(e) = serde_json::from_value(params) {
                self.events.push_back(e);
            }
        }
    }

    fn read_incoming(&mut self) -> Result<Incoming, ClientError> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(ClientError::Closed);
        }
        Ok(Incoming::parse(line.trim_end())?)
    }
}

/// Async control client (tokio), for in-process users and tests.
pub struct AsyncControlClient {
    reader: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
    events: VecDeque<StateEvent>,
    pub hello: Option<HelloResult>,
}

impl AsyncControlClient {
    /// Connect and authenticate.
    pub async fn connect(
        path: &Path,
        client: &str,
        token: Option<String>,
    ) -> Result<Self, ClientError> {
        let stream = tokio::net::UnixStream::connect(path)
            .await
            .map_err(|source| ClientError::Connect {
                path: path.to_path_buf(),
                source,
            })?;
        let (r, w) = stream.into_split();
        let mut c = Self {
            reader: tokio::io::BufReader::new(r),
            writer: w,
            next_id: 1,
            events: VecDeque::new(),
            hello: None,
        };
        let params = HelloParams {
            client: client.into(),
            token,
            protocol: PROTOCOL_VERSION,
        };
        c.hello = Some(c.call::<crate::methods::Hello>(&params).await?);
        Ok(c)
    }

    /// Call a typed method.
    pub async fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Result, ClientError> {
        let v = self
            .call_raw(M::NAME, serde_json::to_value(params)?)
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Send a raw line (for protocol tests) and return the next response.
    pub async fn send_line(&mut self, line: &str) -> Result<ControlResult, ClientError> {
        use tokio::io::AsyncWriteExt;
        self.writer.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            self.writer.write_all(b"\n").await?;
        }
        loop {
            match self.read_incoming().await? {
                Incoming::Response(r) => return Ok(r.into_result()),
                Incoming::Notification(n) => self.stash(n.method, n.params),
            }
        }
    }

    /// Call a method by name with raw JSON params.
    pub async fn call_raw(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        use tokio::io::AsyncWriteExt;
        let id = self.next_id;
        self.next_id += 1;
        self.writer
            .write_all(to_line(&Request::new(id, method, params)).as_bytes())
            .await?;
        loop {
            match self.read_incoming().await? {
                Incoming::Response(r) if r.id == id => {
                    return r.into_result().map_err(ClientError::Control)
                }
                Incoming::Response(_) => continue,
                Incoming::Notification(n) => self.stash(n.method, n.params),
            }
        }
    }

    /// Next state event, waiting up to `timeout`.
    pub async fn next_event(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<StateEvent>, ClientError> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let read = tokio::time::timeout_at(deadline, self.read_incoming()).await;
            match read {
                Err(_) => return Ok(None),
                Ok(Ok(Incoming::Notification(n))) => {
                    self.stash(n.method, n.params);
                    if let Some(e) = self.events.pop_front() {
                        return Ok(Some(e));
                    }
                }
                Ok(Ok(Incoming::Response(_))) => continue,
                Ok(Err(e)) => return Err(e),
            }
        }
    }

    fn stash(&mut self, method: String, params: Value) {
        if method == EVENT_NOTIFICATION {
            if let Ok(e) = serde_json::from_value(params) {
                self.events.push_back(e);
            }
        }
    }

    async fn read_incoming(&mut self) -> Result<Incoming, ClientError> {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(ClientError::Closed);
        }
        Ok(Incoming::parse(line.trim_end())?)
    }
}
