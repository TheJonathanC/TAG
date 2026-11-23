//! The control socket server (§4.1–§4.4).
//!
//! Runs on tokio tasks and never blocks the app loop. It authenticates each connection with
//! `tag.hello`, enforces tiers, roles and coarse token scope, audit-logs act/dangerous calls,
//! implements `agent.wait` and `events.subscribe` on the [`StateEvent`] broadcast, and forwards
//! everything else to the app loop as [`AppEvent::Control`].

use crate::methods::{
    method_info, AgentWaitParams, HelloParams, HelloResult, SubscribeParams, WaitResult,
    PROTOCOL_VERSION,
};
use crate::protocol::{parse_request, to_line, Notification, Response, MAX_LINE_BYTES};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tag_core::config::ControlConfig;
use tag_core::control::{
    codes, ClientCtx, ControlEnvelope, ControlError, ControlResult, ReplyHandle, Role, Tier,
};
use tag_core::event::{AppEvent, StateEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

/// Connections with no traffic (and no subscription) for this long are closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// How long the server waits for the app loop to answer a forwarded request.
const LOOP_REPLY_TIMEOUT: Duration = Duration::from_secs(3600);

/// What a scoped token grants (D6).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenGrant {
    pub role: Role,
    pub run: Option<String>,
    pub task: Option<String>,
    pub pane: Option<u32>,
}

impl TokenGrant {
    /// Tiers a token of this role carries. Role/method restrictions and run policy narrow it
    /// further (e.g. conductors can only merge under `merge_policy = "auto"`).
    pub fn tiers(&self) -> Vec<Tier> {
        match self.role {
            Role::Conductor => vec![Tier::Read, Tier::Act, Tier::Dangerous],
            Role::Worker => vec![Tier::Read, Tier::Act],
            Role::External => vec![Tier::Read],
        }
    }
}

/// In-memory registry of scoped tokens minted for agents TAG launches.
#[derive(Debug, Clone, Default)]
pub struct TokenRegistry(Arc<RwLock<HashMap<String, TokenGrant>>>);

impl TokenRegistry {
    /// Mint a fresh random token for `grant`.
    pub fn mint(&self, grant: TokenGrant) -> String {
        let bytes: [u8; 16] = rand::random();
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let token = format!("tagt_{token}");
        if let Ok(mut m) = self.0.write() {
            m.insert(token.clone(), grant);
        }
        token
    }

    pub fn get(&self, token: &str) -> Option<TokenGrant> {
        self.0.read().ok()?.get(token).cloned()
    }

    /// Bind a token to the pane its agent runs in.
    pub fn set_pane(&self, token: &str, pane: u32) {
        if let Ok(mut m) = self.0.write() {
            if let Some(g) = m.get_mut(token) {
                g.pane = Some(pane);
            }
        }
    }

    pub fn revoke(&self, token: &str) {
        if let Ok(mut m) = self.0.write() {
            m.remove(token);
        }
    }

    /// Revoke the worker tokens bound to `run` (the conductor keeps its token).
    pub fn revoke_run_workers(&self, run: &str) {
        if let Ok(mut m) = self.0.write() {
            m.retain(|_, g| !(g.role == Role::Worker && g.run.as_deref() == Some(run)));
        }
    }

    /// Revoke every token bound to `run`.
    pub fn revoke_run(&self, run: &str) {
        if let Ok(mut m) = self.0.write() {
            m.retain(|_, g| g.run.as_deref() != Some(run));
        }
    }
}

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub tag_version: String,
    pub control: ControlConfig,
    /// Append-only audit log for act/dangerous calls.
    pub audit_path: Option<PathBuf>,
    pub idle_timeout: Duration,
}

impl ServerSettings {
    pub fn new(tag_version: impl Into<String>, control: ControlConfig) -> Self {
        Self {
            tag_version: tag_version.into(),
            control,
            audit_path: None,
            idle_timeout: IDLE_TIMEOUT,
        }
    }
}

struct Shared {
    settings: ServerSettings,
    tokens: TokenRegistry,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    state_tx: broadcast::Sender<StateEvent>,
    audit: Mutex<()>,
}

/// Handle to a running server; removes the socket file on drop.
pub struct ServerHandle {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bind the control socket at `path` and serve connections.
pub async fn serve(
    path: PathBuf,
    settings: ServerSettings,
    tokens: TokenRegistry,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    state_tx: broadcast::Sender<StateEvent>,
) -> std::io::Result<ServerHandle> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        if parent != Path::new("/tmp") && parent != Path::new("/var/tmp") {
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let owner_uid = std::fs::metadata(&path).map(|m| m.uid()).ok();
    info!("Control socket listening at {:?}", path);

    let shared = Arc::new(Shared {
        settings,
        tokens,
        event_tx,
        state_tx,
        audit: Mutex::new(()),
    });
    let task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    if let (Some(uid), Ok(cred)) = (owner_uid, stream.peer_cred()) {
                        if cred.uid() != uid {
                            error!("Rejected control connection from UID {}", cred.uid());
                            continue;
                        }
                    }
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, shared).await;
                    });
                }
                Err(e) => {
                    error!("Control listener accept error: {}", e);
                    break;
                }
            }
        }
    });
    Ok(ServerHandle { path, task })
}

/// Read one `\n`-terminated line of at most `max` bytes. `Ok(None)` on EOF.
/// Over-long lines yield `Err(true)`; I/O errors yield `Err(false)`.
async fn read_line_limited(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    max: usize,
) -> Result<Option<String>, bool> {
    let mut buf = Vec::new();
    loop {
        let available = match reader.fill_buf().await {
            Ok(b) => b,
            Err(_) => return Err(false),
        };
        if available.is_empty() {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
            };
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            if buf.len() > max {
                return Err(true);
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        let n = available.len();
        buf.extend_from_slice(available);
        reader.consume(n);
        if buf.len() > max {
            return Err(true);
        }
    }
}

async fn handle_connection(stream: UnixStream, shared: Arc<Shared>) {
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let mut ctx: Option<Arc<ClientCtx>> = None;
    let subscribed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    loop {
        let timeout = if subscribed.load(std::sync::atomic::Ordering::Relaxed) {
            Duration::from_secs(u64::MAX / 4)
        } else {
            shared.settings.idle_timeout
        };
        let line =
            match tokio::time::timeout(timeout, read_line_limited(&mut reader, MAX_LINE_BYTES))
                .await
            {
                Err(_) => break, // idle
                Ok(Ok(None)) => break,
                Ok(Ok(Some(l))) => l,
                Ok(Err(too_long)) => {
                    if too_long {
                        let _ = out_tx.send(to_line(&Response::error(
                            Value::Null,
                            ControlError::new(codes::INVALID_REQUEST, "request line exceeds 1 MB"),
                        )));
                    }
                    break;
                }
            };
        if line.trim().is_empty() {
            continue;
        }
        let req = match parse_request(&line) {
            Ok(r) => r,
            Err(resp) => {
                let _ = out_tx.send(to_line(&*resp));
                continue;
            }
        };
        let id = req.id.clone().unwrap_or(Value::Null);
        let is_notification = req.id.is_none();

        // Handshake
        let Some(client) = ctx.clone() else {
            let result = if req.method == "tag.hello" {
                match hello(&shared, req.params) {
                    Ok((c, res)) => {
                        ctx = Some(Arc::new(c));
                        Ok(serde_json::to_value(res).unwrap_or(Value::Null))
                    }
                    Err(e) => Err(e),
                }
            } else {
                Err(ControlError::new(
                    codes::NOT_AUTHENTICATED,
                    "call tag.hello first",
                ))
            };
            if !is_notification {
                let _ = out_tx.send(to_line(&Response::from_result(id, result)));
            }
            continue;
        };

        let shared = shared.clone();
        let out_tx = out_tx.clone();
        let subscribed = subscribed.clone();
        tokio::spawn(async move {
            let result = dispatch(
                &shared,
                &client,
                &req.method,
                req.params,
                &out_tx,
                &subscribed,
            )
            .await;
            if !is_notification {
                let _ = out_tx.send(to_line(&Response::from_result(id, result)));
            }
        });
    }
    drop(out_tx);
    let _ = writer.await;
}

fn hello(shared: &Shared, params: Value) -> Result<(ClientCtx, HelloResult), ControlError> {
    let p: HelloParams =
        serde_json::from_value(params).map_err(|e| ControlError::invalid_params(e.to_string()))?;
    let ctx = match p.token.as_deref().filter(|t| !t.is_empty()) {
        Some(token) => {
            let grant = shared.tokens.get(token).ok_or_else(|| {
                ControlError::new(codes::NOT_AUTHENTICATED, "unknown or revoked token")
            })?;
            ClientCtx {
                client: p.client.clone(),
                role: grant.role,
                tiers: grant.tiers(),
                run: grant.run.clone(),
                task: grant.task.clone(),
                pane: grant.pane,
            }
        }
        None => ClientCtx {
            client: p.client.clone(),
            role: Role::External,
            tiers: shared.settings.control.tiers_for(&p.client),
            run: None,
            task: None,
            pane: None,
        },
    };
    let res = HelloResult {
        protocol: PROTOCOL_VERSION,
        tag_version: shared.settings.tag_version.clone(),
        client: ctx.client.clone(),
        role: ctx.role,
        tiers: ctx.tiers.clone(),
        run_scope: ctx.run.clone(),
    };
    Ok((ctx, res))
}

/// Tier, role and coarse scope checks shared by every method. Fine-grained checks (e.g. a
/// worker's own worktree, a conductor's merge policy) happen in the app loop.
pub fn authorize(ctx: &ClientCtx, method: &str, params: &Value) -> Result<Tier, ControlError> {
    let info = method_info(method).ok_or_else(|| {
        ControlError::new(codes::METHOD_NOT_FOUND, format!("unknown method {method}"))
    })?;
    let tier = info
        .tier
        .ok_or_else(|| ControlError::new(codes::INVALID_REQUEST, "tag.hello already done"))?;
    if !ctx.allows(tier) {
        return Err(ControlError::forbidden(format!(
            "{method} needs the {tier:?} tier; client '{}' has {:?}",
            ctx.client, ctx.tiers
        )));
    }
    if !info.roles.contains(&ctx.role) {
        return Err(ControlError::forbidden(format!(
            "{method} is not available to {:?} clients",
            ctx.role
        )));
    }
    if ctx.role != Role::External {
        if let Some(run) = params.get("run").and_then(Value::as_str) {
            if ctx.run.as_deref() != Some(run) {
                return Err(ControlError::forbidden("token is scoped to another run"));
            }
        }
    }
    if ctx.role == Role::Worker {
        if let Some(task) = params.get("task").and_then(Value::as_str) {
            if ctx.task.as_deref() != Some(task) {
                return Err(ControlError::forbidden("token is scoped to another task"));
            }
        }
        if let Some(pane) = params.get("pane").and_then(Value::as_u64) {
            if ctx.pane != Some(pane as u32) {
                return Err(ControlError::forbidden(
                    "workers may only target their own pane",
                ));
            }
        }
    }
    Ok(tier)
}

async fn dispatch(
    shared: &Shared,
    ctx: &Arc<ClientCtx>,
    method: &str,
    params: Value,
    out_tx: &mpsc::UnboundedSender<String>,
    subscribed: &std::sync::atomic::AtomicBool,
) -> ControlResult {
    let tier = authorize(ctx, method, &params)?;
    if tier != Tier::Read {
        audit(shared, ctx, method, &params);
    }
    match method {
        "agent.wait" => agent_wait(shared, ctx, params).await,
        "events.subscribe" => {
            let p: SubscribeParams = serde_json::from_value(params)
                .map_err(|e| ControlError::invalid_params(e.to_string()))?;
            let mut rx = shared.state_tx.subscribe();
            let out = out_tx.clone();
            let scoped_run = ctx.run.clone();
            subscribed.store(true, std::sync::atomic::Ordering::Relaxed);
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            if !event_matches(&ev, &p, scoped_run.as_deref()) {
                                continue;
                            }
                            if out.send(to_line(&Notification::event(&ev))).is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("control subscriber lagged by {} events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            Ok(json!({"subscribed": true}))
        }
        _ => forward(shared, ctx, method, params).await,
    }
}

fn event_matches(ev: &StateEvent, p: &SubscribeParams, scoped_run: Option<&str>) -> bool {
    if let Some(kinds) = &p.kinds {
        if !kinds.iter().any(|k| k == ev.kind()) {
            return false;
        }
    }
    let want_run = p.run.as_deref().or(scoped_run);
    if let Some(run) = want_run {
        // Pane/attention events carry no run; let them through only when no pane filter
        // excludes them. Run-bearing events must match.
        if let Some(r) = ev.run() {
            if r != run {
                return false;
            }
        }
    }
    if let Some(pane) = p.pane {
        if ev.pane().map(|x| x.0) != Some(pane) {
            return false;
        }
    }
    true
}

/// Forward a request to the app loop and await its reply.
pub async fn forward_to_loop(
    event_tx: &mpsc::UnboundedSender<AppEvent>,
    ctx: &ClientCtx,
    method: &str,
    params: Value,
) -> ControlResult {
    let (reply, rx) = ReplyHandle::new();
    let envelope = ControlEnvelope {
        client: ctx.clone(),
        method: method.to_string(),
        params,
        reply,
    };
    event_tx
        .send(AppEvent::Control(envelope))
        .map_err(|_| ControlError::internal("TAG is shutting down"))?;
    match tokio::time::timeout(LOOP_REPLY_TIMEOUT, rx).await {
        Ok(Ok(res)) => res,
        Ok(Err(_)) => Err(ControlError::internal("request dropped by TAG")),
        Err(_) => Err(ControlError::timeout("TAG did not answer in time")),
    }
}

async fn forward(shared: &Shared, ctx: &ClientCtx, method: &str, params: Value) -> ControlResult {
    forward_to_loop(&shared.event_tx, ctx, method, params).await
}

async fn agent_wait(shared: &Shared, ctx: &ClientCtx, params: Value) -> ControlResult {
    let p: AgentWaitParams =
        serde_json::from_value(params).map_err(|e| ControlError::invalid_params(e.to_string()))?;
    // Subscribe before reading the current status so no transition is missed.
    let mut rx = shared.state_tx.subscribe();
    let wanted = |s: &str| p.until.iter().any(|u| u == s);
    let current = current_status(shared, ctx, p.pane).await?;
    if let Some(s) = current.as_deref() {
        if wanted(s) {
            return ok_wait(s, "already");
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(p.timeout_s);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => {
                return Err(ControlError::timeout(format!(
                    "pane {} still '{}' after {}s",
                    p.pane,
                    current_status(shared, ctx, p.pane)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "unknown".into()),
                    p.timeout_s
                )))
            }
            Ok(Ok(StateEvent::PaneStatusChanged { pane, to, .. })) if pane.0 == p.pane => {
                if wanted(&to) {
                    return ok_wait(&to, "status");
                }
            }
            Ok(Ok(StateEvent::PaneExited { pane, .. })) if pane.0 == p.pane => {
                return ok_wait("exited", "exited");
            }
            Ok(Ok(_)) => {}
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                if let Some(s) = current_status(shared, ctx, p.pane).await? {
                    if wanted(&s) {
                        return ok_wait(&s, "status");
                    }
                }
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err(ControlError::internal("TAG is shutting down"))
            }
        }
    }
}

fn ok_wait(status: &str, reason: &str) -> ControlResult {
    Ok(serde_json::to_value(WaitResult {
        status: status.to_string(),
        reason: reason.to_string(),
    })
    .unwrap_or(Value::Null))
}

async fn current_status(
    shared: &Shared,
    ctx: &ClientCtx,
    pane: u32,
) -> Result<Option<String>, ControlError> {
    let v = forward(shared, ctx, "agent.status", json!({ "pane": pane })).await?;
    Ok(v.get("status").and_then(Value::as_str).map(String::from))
}

/// Truncate long free-text fields so prompts and pasted text don't bloat the audit log.
fn redact(params: &Value) -> Value {
    let mut v = params.clone();
    if let Some(obj) = v.as_object_mut() {
        for key in ["prompt", "text", "body", "summary", "notes"] {
            if let Some(Value::String(s)) = obj.get_mut(key) {
                if s.chars().count() > 200 {
                    *s = s.chars().take(200).collect::<String>() + "…";
                }
            }
        }
    }
    v
}

fn audit(shared: &Shared, ctx: &ClientCtx, method: &str, params: &Value) {
    let Some(path) = &shared.settings.audit_path else {
        return;
    };
    let line = json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "client": ctx.client,
        "role": ctx.role,
        "run": ctx.run,
        "task": ctx.task,
        "method": method,
        "params": redact(params),
    });
    let _guard = shared.audit.lock();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path);
    match file {
        Ok(mut f) => {
            let _ = writeln!(f, "{line}");
        }
        Err(e) => warn!("cannot write control audit log {:?}: {}", path, e),
    }
}

use std::os::unix::fs::OpenOptionsExt as _;

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(role: Role, tiers: Vec<Tier>) -> ClientCtx {
        ClientCtx {
            client: "t".into(),
            role,
            tiers,
            run: Some("r_1".into()),
            task: Some("t1".into()),
            pane: Some(5),
        }
    }

    #[test]
    fn redact_truncates_free_text() {
        let long = "x".repeat(500);
        let v = redact(&json!({"prompt": long, "pane": 1}));
        assert_eq!(v["prompt"].as_str().unwrap().chars().count(), 201);
        assert_eq!(v["pane"], 1);
    }

    #[test]
    fn worker_scope_checks() {
        let w = ctx(Role::Worker, vec![Tier::Read, Tier::Act]);
        assert!(authorize(&w, "pane.read", &json!({"pane": 5})).is_ok());
        assert_eq!(
            authorize(&w, "pane.read", &json!({"pane": 6}))
                .unwrap_err()
                .code,
            codes::FORBIDDEN
        );
        assert_eq!(
            authorize(&w, "task.report", &json!({"task": "t2"}))
                .unwrap_err()
                .code,
            codes::FORBIDDEN
        );
        assert!(authorize(
            &w,
            "task.report",
            &json!({"summary": "x", "status": "done"})
        )
        .is_ok());
    }

    #[test]
    fn unknown_method_is_not_found() {
        let e = ctx(Role::External, vec![Tier::Read]);
        assert_eq!(
            authorize(&e, "nope", &json!({})).unwrap_err().code,
            codes::METHOD_NOT_FOUND
        );
    }
}
