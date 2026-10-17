use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::Frame;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;

use tag_core::event::{AppEvent, EventResult, HookEvent};
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_core::theme::Theme;
use tag_pty::PtyPane;

use crate::profile::AgentProfile;
use crate::status::AgentStatus;

pub struct AgentPane {
    pty: PtyPane,
    profile_name: String,
    status: AgentStatus,
    start_time: DateTime<Utc>,
    worktree: Option<PathBuf>,
    client_session: Option<String>,
}

impl AgentPane {
    pub fn spawn(
        id: PaneId,
        profile: &AgentProfile,
        worktree: Option<PathBuf>,
        width: u16,
        height: u16,
        scrollback_capacity: usize,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<Self, anyhow::Error> {
        let mut env = profile.env.clone();
        if let Some(ref wt) = worktree {
            env.insert(
                "COCKPIT_WORKTREE".to_string(),
                wt.to_string_lossy().to_string(),
            );
        }

        let cwd = worktree.clone().or_else(|| profile.default_cwd.clone());

        let mut pty = PtyPane::spawn(
            id,
            profile.name.clone(),
            PaneKind::Agent,
            Some(profile.command.clone()),
            &profile.args,
            &env,
            cwd,
            width,
            height,
            scrollback_capacity,
            event_tx,
        )?;

        pty.set_agent_status(Some("running"));

        Ok(Self {
            pty,
            profile_name: profile.name.clone(),
            status: AgentStatus::Running,
            start_time: Utc::now(),
            worktree,
            client_session: None,
        })
    }

    pub fn spawn_tmux(
        id: PaneId,
        session_name: &str,
        width: u16,
        height: u16,
        scrollback_capacity: usize,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<Self, anyhow::Error> {
        let _ = std::process::Command::new("tmux")
            .args(["start-server"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-g", "mouse", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-g", "set-clipboard", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-g", "allow-passthrough", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-g", "window-size", "latest"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-g", "aggressive-resize", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", session_name, "mouse", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", session_name, "set-clipboard", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-t", session_name, "allow-passthrough", "on"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args([
                "set-window-option",
                "-t",
                session_name,
                "window-size",
                "latest",
            ])
            .status();
        let _ = std::process::Command::new("tmux")
            .args([
                "set-window-option",
                "-t",
                session_name,
                "aggressive-resize",
                "on",
            ])
            .status();

        let env = HashMap::new();
        // Attach directly to the requested session without creating a temporary suffixed session
        let args = vec![
            "new-session".to_string(),
            "-A".to_string(),
            "-s".to_string(),
            session_name.to_string(),
        ];
        let client_sess = None;

        let mut pty = PtyPane::spawn(
            id,
            format!("tmux: {}", session_name),
            PaneKind::Tmux,
            Some("tmux".to_string()),
            &args,
            &env,
            None,
            width,
            height,
            scrollback_capacity,
            event_tx,
        )?;

        pty.set_agent_status(Some("attached"));

        Ok(Self {
            pty,
            profile_name: "tmux".to_string(),
            status: AgentStatus::Running,
            start_time: Utc::now(),
            worktree: None,
            client_session: client_sess,
        })
    }

    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status;
        self.pty.set_agent_status(Some(status.as_str()));
        if status == AgentStatus::AwaitingInput {
            self.pty.set_needs_attention(true);
        } else if status == AgentStatus::Running {
            self.pty.set_needs_attention(false);
        }
    }

    pub fn status(&self) -> AgentStatus {
        self.status
    }

    pub fn start_time(&self) -> DateTime<Utc> {
        self.start_time
    }

    pub fn pty(&self) -> &PtyPane {
        &self.pty
    }

    pub fn pty_mut(&mut self) -> &mut PtyPane {
        &mut self.pty
    }
}

impl Pane for AgentPane {
    fn id(&self) -> PaneId {
        self.pty.id()
    }

    fn title(&self) -> String {
        self.pty.title()
    }

    fn set_title(&mut self, title: String) {
        self.pty.set_title(title);
    }

    fn set_zoomed(&mut self, zoomed: bool) {
        self.pty.set_zoomed(zoomed);
    }

    fn kind(&self) -> PaneKind {
        self.pty.kind()
    }

    fn is_active(&self) -> bool {
        self.pty.is_active()
    }

    fn send_input(&mut self, data: &[u8]) {
        self.pty.send_input(data);
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.worktree.clone().or_else(|| self.pty.cwd())
    }

    fn branch(&self) -> Option<String> {
        self.pty.branch()
    }

    fn agent_status(&self) -> Option<&'static str> {
        if self.profile_name == "tmux" {
            if let Some(session) = self.pty.tmux_session_name() {
                if self.pty.cached_detect_tmux_agent(&session).is_some() {
                    return Some("running");
                }
            }
            if let Some(st) = self.pty.agent_status() {
                return Some(st);
            }
        }
        Some(self.status.as_str())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool, theme: &Theme) {
        self.pty.render(frame, area, is_focused, theme);
    }

    fn handle_event(&mut self, event: &AppEvent) -> EventResult {
        if let AppEvent::AgentHook(payload) = event {
            if payload.pane_id == Some(self.id()) || payload.agent == self.profile_name {
                match payload.event {
                    HookEvent::NeedsPermission | HookEvent::NeedsInput => {
                        self.set_status(AgentStatus::AwaitingInput);
                    }
                    HookEvent::TaskDone => {
                        self.set_status(AgentStatus::Done);
                    }
                    HookEvent::Error => {
                        self.set_status(AgentStatus::Idle);
                    }
                    HookEvent::Exited => {
                        self.set_status(AgentStatus::Exited);
                    }
                    HookEvent::UsageThreshold | HookEvent::PasteImage => {}
                }
            }
        }

        self.pty.handle_event(event)
    }

    fn on_resize(&mut self, width: u16, height: u16) {
        self.pty.on_resize(width, height);
    }

    fn scroll(&mut self, delta: i32) {
        self.pty.scroll(delta);
    }

    fn scroll_at(&mut self, delta: i32, col: u16, row: u16) {
        self.pty.scroll_at(delta, col, row);
    }

    fn scroll_to_bottom(&mut self) {
        self.pty.scroll_to_bottom();
    }

    fn scroll_offset(&self) -> usize {
        self.pty.scroll_offset()
    }

    fn start_selection(&mut self, col: u16, row: u16) {
        Pane::start_selection(&mut self.pty, col, row);
    }

    fn update_selection(&mut self, col: u16, row: u16) {
        self.pty.update_selection(col, row);
    }

    fn clear_selection(&mut self) {
        self.pty.clear_selection();
    }

    fn has_selection(&self) -> bool {
        self.pty.has_selection()
    }

    fn copy_selection(&mut self) -> Option<String> {
        self.pty.copy_selection()
    }

    fn running_process(&self) -> (Option<String>, Vec<String>, Option<PathBuf>) {
        self.pty.running_process()
    }

    fn tmux_session_name(&self) -> Option<String> {
        self.pty.tmux_session_name()
    }

    fn kill(&mut self) {
        self.pty.kill();
        if let Some(ref sess) = self.client_session.take() {
            let _ = std::process::Command::new("tmux")
                .args(["kill-session", "-t", sess])
                .status();
        }
    }
}

impl Drop for AgentPane {
    fn drop(&mut self) {
        if let Some(ref sess) = self.client_session {
            let _ = std::process::Command::new("tmux")
                .args(["kill-session", "-t", sess])
                .status();
        }
    }
}
