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
        let env = HashMap::new();
        let args = vec![
            "attach".to_string(),
            "-t".to_string(),
            session_name.to_string(),
        ];

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
                    HookEvent::UsageThreshold => {}
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

    fn scroll_to_bottom(&mut self) {
        self.pty.scroll_to_bottom();
    }

    fn scroll_offset(&self) -> usize {
        self.pty.scroll_offset()
    }

    fn copy_selection(&mut self) -> Option<String> {
        self.pty.copy_selection()
    }

    fn running_process(&self) -> (Option<String>, Vec<String>, Option<PathBuf>) {
        self.pty.running_process()
    }
}
