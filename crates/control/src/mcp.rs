//! `tag mcp`: a hand-rolled MCP server (newline-delimited JSON-RPC 2.0 over stdio) that exposes
//! control methods as tools. Logs go to stderr only; stdout carries protocol messages.

use crate::client::{ClientError, ControlClient};
use crate::discovery::{discover_socket, TOKEN_ENV};
use crate::methods::{MethodInfo, CATALOG};
use crate::{Role, Tier};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_TEXT: usize = 100 * 1024;
const NOT_RUNNING: &str = "TAG is not running. Start `tag` or `tag daemon`.";
const INSTRUCTIONS: &str = "TAG is a terminal agent multiplexer. These tools act on the user's \
local TAG instance: list panes, read agent output, spawn agents and worktrees, and manage \
orchestration runs. Dangerous operations (such as merging) may be queued for human approval \
and can take a while or be denied.";

/// Options for [`run_mcp`].
#[derive(Debug, Clone, Default)]
pub struct McpOptions {
    /// Client name sent in `tag.hello` (decides default tiers).
    pub client_name: String,
    /// Explicit control socket; otherwise discovered.
    pub socket: Option<PathBuf>,
    /// Control token; otherwise `$TAG_CONTROL_TOKEN`.
    pub token: Option<String>,
}

struct State {
    opts: McpOptions,
    client: Option<ControlClient>,
}

impl State {
    /// Connect if not connected yet. Returns whether a client is available.
    fn ensure(&mut self) -> bool {
        if self.client.is_some() {
            return true;
        }
        let path = match self.opts.socket.clone().or_else(discover_socket) {
            Some(p) => p,
            None => return false,
        };
        let token = self
            .opts
            .token
            .clone()
            .or_else(|| std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty()));
        match ControlClient::connect(&path, &self.opts.client_name, token) {
            Ok(c) => {
                self.client = Some(c);
                true
            }
            Err(e) => {
                eprintln!("tag mcp: cannot connect: {e}");
                false
            }
        }
    }
}

fn tool_name(method: &str) -> String {
    format!("tag_{}", method.replace('.', "_"))
}

fn exposed(m: &MethodInfo) -> bool {
    m.tier.is_some() && m.name != "events.subscribe"
}

fn input_schema(m: &MethodInfo) -> Value {
    let mut v = serde_json::to_value((m.schema)()).unwrap_or_else(|_| json!({}));
    if let Some(o) = v.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
        o.insert("type".into(), json!("object"));
        o.entry("properties").or_insert_with(|| json!({}));
    }
    v
}

fn tier_note(t: Tier) -> &'static str {
    match t {
        Tier::Read => "[read-only]",
        Tier::Act => "[act: changes state]",
        Tier::Dangerous => "[dangerous: may require human approval]",
    }
}

fn tools_list(client: Option<&ControlClient>) -> Vec<Value> {
    CATALOG
        .iter()
        .filter(|m| exposed(m))
        .filter(|m| match client {
            Some(c) => {
                let h = c.hello();
                m.tier.is_some_and(|t| h.tiers.contains(&t)) && m.roles.contains(&h.role)
            }
            None => m.tier == Some(Tier::Read) && m.roles.contains(&Role::External),
        })
        .map(|m| {
            json!({
                "name": tool_name(m.name),
                "description": format!("{} {}", m.description, tier_note(m.tier.unwrap_or(Tier::Read))),
                "inputSchema": input_schema(m),
            })
        })
        .collect()
}

fn text_result(text: String, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n…(truncated)");
    }
    s
}

fn tools_call(state: &mut State, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let info = CATALOG
        .iter()
        .find(|m| exposed(m) && tool_name(m.name) == name)
        .ok_or_else(|| (-32602, format!("unknown tool: {name}")))?;
    if !state.ensure() {
        return Ok(text_result(NOT_RUNNING.into(), true));
    }
    let args = match params.get("arguments") {
        Some(Value::Null) | None => json!({}),
        Some(a) => a.clone(),
    };
    let client = state.client.as_mut().expect("ensured");
    match client.call_raw(info.name, args) {
        Ok(v) => {
            let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            Ok(text_result(truncate(text), false))
        }
        Err(ClientError::Control(e)) => Ok(text_result(
            format!("error {}: {}", e.code, e.message),
            true,
        )),
        Err(e) => {
            // Connection likely dead: drop it so the next call reconnects.
            state.client = None;
            eprintln!("tag mcp: call failed: {e}");
            Ok(text_result(NOT_RUNNING.into(), true))
        }
    }
}

fn handle(state: &mut State, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            let req = params.get("protocolVersion").and_then(Value::as_str);
            let ver = req
                .filter(|v| SUPPORTED_VERSIONS.contains(v))
                .unwrap_or(SUPPORTED_VERSIONS[0]);
            Ok(json!({
                "protocolVersion": ver,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "tag", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            state.ensure();
            Ok(json!({"tools": tools_list(state.client.as_ref())}))
        }
        "tools/call" => tools_call(state, params),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

/// Serve MCP over `input`/`output` until EOF.
/// One MCP session (protocol state plus the lazy control connection), shared by transports.
pub struct McpSession {
    state: State,
}

impl McpSession {
    pub fn new(opts: McpOptions) -> Self {
        Self {
            state: State { opts, client: None },
        }
    }

    /// Handle one JSON-RPC message; `None` for notifications (no reply).
    pub fn handle_message(&mut self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").filter(|i| !i.is_null()).cloned()?;
        if method.is_empty() {
            return None;
        }
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        Some(match handle(&mut self.state, method, &params) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
            }
        })
    }

    /// Handle one raw message text (parse errors become a JSON-RPC error reply).
    pub fn handle_text(&mut self, text: &str) -> Option<Value> {
        match serde_json::from_str::<Value>(text) {
            Ok(msg) => self.handle_message(&msg),
            Err(e) => Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}}),
            ),
        }
    }
}

/// Serve MCP over stdio: one JSON-RPC message per line.
pub fn run_mcp<R: BufRead, W: Write>(input: R, mut output: W, opts: McpOptions) -> io::Result<()> {
    let mut session = McpSession::new(opts);
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = session.handle_text(&line) {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}
