use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::action::Action;
use crate::pane::PaneId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    UpdateAvailable(String),

    // Control-plane request (from the control socket)
    Control(crate::control::ControlEnvelope),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    Consumed,
    Ignored,
}

/// State changes published by the app loop on a `tokio::sync::broadcast` channel.
///
/// Consumers (the control server's `agent.wait` / `events.subscribe`, the Workers table,
/// the orchestrator scheduler) subscribe to these instead of polling pane state.
/// Status strings are snake_case (`idle`, `running`, `awaiting_input`, `done`, `exited`, `failed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StateEvent {
    /// An agent pane's detected status changed.
    PaneStatusChanged {
        pane: PaneId,
        from: Option<String>,
        to: String,
        at: DateTime<Utc>,
    },
    /// A pane's child process exited and the pane was removed.
    PaneExited {
        pane: PaneId,
        exit_code: Option<i32>,
        at: DateTime<Utc>,
    },
    /// An orchestrator task changed state.
    TaskStateChanged {
        run: String,
        task: String,
        from: String,
        to: String,
        at: DateTime<Utc>,
    },
    /// An orchestrator run changed state.
    RunStateChanged {
        run: String,
        from: String,
        to: String,
        at: DateTime<Utc>,
    },
    /// The attention queue gained an item.
    AttentionAdded {
        item: u64,
        pane: Option<PaneId>,
        agent: String,
        event: HookEvent,
        message: String,
        at: DateTime<Utc>,
    },
}

impl StateEvent {
    /// Short snake_case name of the event kind, used for subscription filters.
    pub fn kind(&self) -> &'static str {
        match self {
            StateEvent::PaneStatusChanged { .. } => "pane_status_changed",
            StateEvent::PaneExited { .. } => "pane_exited",
            StateEvent::TaskStateChanged { .. } => "task_state_changed",
            StateEvent::RunStateChanged { .. } => "run_state_changed",
            StateEvent::AttentionAdded { .. } => "attention_added",
        }
    }

    /// The pane this event concerns, if any.
    pub fn pane(&self) -> Option<PaneId> {
        match self {
            StateEvent::PaneStatusChanged { pane, .. } | StateEvent::PaneExited { pane, .. } => {
                Some(*pane)
            }
            StateEvent::AttentionAdded { pane, .. } => *pane,
            _ => None,
        }
    }

    /// The run this event concerns, if any.
    pub fn run(&self) -> Option<&str> {
        match self {
            StateEvent::TaskStateChanged { run, .. } | StateEvent::RunStateChanged { run, .. } => {
                Some(run)
            }
            _ => None,
        }
    }
}

/// Normalize a pane's display status (e.g. `awaiting-input`) to the snake_case form used on
/// the control plane (`awaiting_input`).
pub fn normalize_status(status: &str) -> String {
    status.replace('-', "_")
}
