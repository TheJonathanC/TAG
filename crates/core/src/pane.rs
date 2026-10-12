use ratatui::layout::Rect;
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::event::{AppEvent, EventResult};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pane-{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub usize);

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "workspace-{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneKind {
    Shell,
    Agent,
    Tmux,
    Applet,
}

impl PaneKind {
    pub fn icon(&self) -> &'static str {
        match self {
            PaneKind::Shell => "❯_",
            PaneKind::Agent => "🤖",
            PaneKind::Tmux => "🪟",
            PaneKind::Applet => "🧩",
        }
    }
}

pub trait Pane: Send + Sync {
    fn id(&self) -> PaneId;
    fn title(&self) -> String;
    fn kind(&self) -> PaneKind;
    fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool, theme: &Theme);
    fn handle_event(&mut self, event: &AppEvent) -> EventResult;
    fn on_resize(&mut self, width: u16, height: u16);
    fn is_active(&self) -> bool;
    fn send_input(&mut self, _data: &[u8]) {}

    // Scrolling & selection
    fn scroll(&mut self, _delta: i32) {}
    fn scroll_to_bottom(&mut self) {}
    fn scroll_offset(&self) -> usize {
        0
    }
    fn copy_selection(&mut self) -> Option<String> {
        None
    }

    // Metadata
    fn cwd(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn branch(&self) -> Option<String> {
        None
    }
    fn agent_status(&self) -> Option<&'static str> {
        None
    }
}
