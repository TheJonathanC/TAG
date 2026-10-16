use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::action::Action;
use crate::pane::PaneId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    NeedsPermission,
    NeedsInput,
    TaskDone,
    Error,
    Exited,
    UsageThreshold,
    PasteImage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPayload {
    pub event: HookEvent,
    pub pane_id: Option<PaneId>,
    pub agent: String,
    pub worktree: Option<PathBuf>,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    // PTY events
    PtyOutput {
        pane_id: PaneId,
        data: Vec<u8>,
    },
    PtyExited {
        pane_id: PaneId,
        exit_code: Option<i32>,
    },

    // Agent hooks (via Unix domain socket)
    AgentHook(HookPayload),

    // Terminal user input
    TerminalInput(crossterm::event::Event),

    // Semantic user actions
    Action(Action),

    // System ticks and render signals
    Tick,
    Resize {
        width: u16,
        height: u16,
    },
    FocusGained,
    FocusLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    Consumed,
    Ignored,
}
