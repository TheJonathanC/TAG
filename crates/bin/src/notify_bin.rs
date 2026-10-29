use chrono::Utc;
use clap::Parser;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use tag_core::event::{HookEvent, HookPayload};
use tag_core::pane::PaneId;

#[derive(Parser, Debug)]
#[command(name = "cockpit-notify")]
#[command(version)]
#[command(about = "Helper CLI for notifying TAG from agent hooks")]
struct Cli {
    /// Event type: NeedsPermission, NeedsInput, TaskDone, Error, Exited, UsageThreshold, PasteImage
    event: String,

    /// Optional JSON payload appended by some agents (e.g. Codex `notify`).
    /// Used as the message when --message is not given.
    payload: Option<String>,

    /// Optional message string
    #[arg(short, long, default_value = "")]
    message: String,

    /// Agent identifier
    #[arg(short, long, default_value = "agent")]
    agent: String,

    /// Target pane ID override
    #[arg(short, long)]
    pane_id: Option<u32>,

    /// Worktree path override
    #[arg(short, long)]
    worktree: Option<PathBuf>,

    /// Read the hook's JSON payload from stdin (e.g. Claude Code hooks) for the message
    #[arg(long)]
    stdin: bool,
}

fn main() {
    run_notify();
}

fn parse_event(name: &str) -> Option<HookEvent> {
    match name.to_lowercase().as_str() {
        "needspermission" | "permission" => Some(HookEvent::NeedsPermission),
        "needsinput" | "input" | "question" => Some(HookEvent::NeedsInput),
        "taskdone" | "done" | "stop" => Some(HookEvent::TaskDone),
        "error" => Some(HookEvent::Error),
        "exited" => Some(HookEvent::Exited),
        "usagethreshold" | "usage" => Some(HookEvent::UsageThreshold),
        "pasteimage" | "paste-image" | "paste" | "image" => Some(HookEvent::PasteImage),
        _ => None,
    }
}

/// Extract a human-readable message from an agent-supplied JSON payload.
fn message_from_payload(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    ["last-assistant-message", "message", "title"]
        .iter()
        .find_map(|k| value.get(*k).and_then(|v| v.as_str()))
        .map(|s| s.chars().take(300).collect())
}

pub fn run_notify() {
    // Parse first so --help / --version and usage errors always work.
    let cli = Cli::parse();

    let Some(event_type) = parse_event(&cli.event) else {
        eprintln!(
            "cockpit-notify: unknown event '{}'. Expected one of: NeedsPermission, NeedsInput, TaskDone, Error, Exited, UsageThreshold, PasteImage",
            cli.event
        );
        std::process::exit(2);
    };

    // If COCKPIT_SOCK is not set we are not running inside TAG: exit silently with success.
    let sock_path = match std::env::var("COCKPIT_SOCK") {
        Ok(path) if !path.is_empty() => path,
        _ => std::process::exit(0),
    };

    let env_pane_id = std::env::var("COCKPIT_PANE_ID")
        .ok()
        .and_then(|id_str| id_str.parse::<u32>().ok())
        .map(PaneId);

    let pane_id = cli.pane_id.map(PaneId).or(env_pane_id);

    let stdin_payload = if cli.stdin && cli.message.is_empty() {
        use std::io::Read;
        let mut buf = String::new();
        let _ = std::io::stdin().take(64 * 1024).read_to_string(&mut buf);
        Some(buf)
    } else {
        None
    };

    let message = if cli.message.is_empty() {
        cli.payload
            .as_deref()
            .or(stdin_payload.as_deref())
            .and_then(message_from_payload)
            .unwrap_or_default()
    } else {
        cli.message
    };

    let payload = HookPayload {
        event: event_type,
        pane_id,
        agent: cli.agent,
        worktree: cli.worktree,
        message,
        timestamp: Utc::now(),
    };

    if let Ok(mut stream) = UnixStream::connect(sock_path) {
        if let Ok(json_bytes) = serde_json::to_vec(&payload) {
            let _ = stream.write_all(&json_bytes);
            let _ = stream.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event_rejects_unknown() {
        assert!(matches!(parse_event("Stop"), Some(HookEvent::TaskDone)));
        assert!(parse_event("bogus").is_none());
    }

    #[test]
    fn test_message_from_codex_payload() {
        let p = r#"{"type":"agent-turn-complete","last-assistant-message":"All tests pass"}"#;
        assert_eq!(message_from_payload(p).as_deref(), Some("All tests pass"));
        assert_eq!(message_from_payload("not json"), None);
    }
}
