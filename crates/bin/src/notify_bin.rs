use chrono::Utc;
use clap::Parser;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use tag_core::event::{HookEvent, HookPayload};
use tag_core::pane::PaneId;

#[derive(Parser, Debug)]
#[command(name = "cockpit-notify")]
#[command(about = "Helper CLI for notifying TAG from agent hooks")]
struct Cli {
    /// Event type: NeedsPermission, NeedsInput, TaskDone, Error, Exited, UsageThreshold
    event: String,

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
}

#[allow(dead_code)]
fn main() {
    run_notify();
}

pub fn run_notify() {
    // If COCKPIT_SOCK is not set, exit silently with success (silent no-op)
    let sock_path = match std::env::var("COCKPIT_SOCK") {
        Ok(path) if !path.is_empty() => path,
        _ => std::process::exit(0),
    };

    let cli = Cli::parse();

    let event_type = match cli.event.to_lowercase().as_str() {
        "needspermission" | "permission" => HookEvent::NeedsPermission,
        "needsinput" | "input" | "question" => HookEvent::NeedsInput,
        "taskdone" | "done" | "stop" => HookEvent::TaskDone,
        "error" => HookEvent::Error,
        "exited" => HookEvent::Exited,
        "usagethreshold" | "usage" => HookEvent::UsageThreshold,
        "pasteimage" | "paste-image" | "paste" | "image" => HookEvent::PasteImage,
        _ => HookEvent::NeedsInput,
    };

    let env_pane_id = std::env::var("COCKPIT_PANE_ID")
        .ok()
        .and_then(|id_str| id_str.parse::<u32>().ok())
        .map(PaneId);

    let pane_id = cli.pane_id.map(PaneId).or(env_pane_id);

    let payload = HookPayload {
        event: event_type,
        pane_id,
        agent: cli.agent,
        worktree: cli.worktree,
        message: cli.message,
        timestamp: Utc::now(),
    };

    if let Ok(mut stream) = UnixStream::connect(sock_path) {
        if let Ok(json_bytes) = serde_json::to_vec(&payload) {
            let _ = stream.write_all(&json_bytes);
            let _ = stream.flush();
        }
    }
}
