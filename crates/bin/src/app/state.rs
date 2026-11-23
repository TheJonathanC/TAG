#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::time::Duration;
use tag_clipboard::{SnippetBank, SnippetOverlayState};
use tag_core::command::CommandRegistry;
use tag_core::config::Config;
use tag_core::event::{normalize_status, AppEvent, StateEvent};
use tag_core::pane::{Pane, PaneId};
use tag_core::theme::Theme;
use tag_git::{GitOverlayState, StandardGitBackend};
use tag_keys::KeyStateMachine;
use tag_layout::LayoutManager;
use tag_notify::{AttentionOverlayState, AttentionQueue};
use tag_ui::{PaletteState, RenameModalState, TabTrayState, Toast, TourState};
use tag_usage::{UsageOverlayState, UsageStore};
use tag_vault::{VaultOverlayState, VaultStore};
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) struct CachedGitData {
    pub(crate) worktrees: Vec<tag_git::WorktreeInfo>,
    pub(crate) repo_status: tag_git::RepoStatus,
    pub(crate) diff: String,
    pub(crate) loaded_at: std::time::Instant,
}

/// The version this binary was released as (stamped from the release tag).
pub(crate) const TAG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often transcripts are re-scanned for token usage in the background.
pub(crate) const USAGE_SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Window in which a second `q` / Ctrl+C on an empty workspace confirms quitting.
pub(crate) const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(2);

pub(crate) struct AppState {
    pub(crate) panes: HashMap<PaneId, Box<dyn Pane>>,
    pub(crate) next_pane_id: u32,
    pub(crate) layout_mgr: LayoutManager,
    pub(crate) key_state_machine: KeyStateMachine,
    pub(crate) command_registry: CommandRegistry,
    pub(crate) theme: Theme,
    pub(crate) config: Config,
    pub(crate) attention_queue: AttentionQueue,
    pub(crate) toasts: Vec<Toast>,
    pub(crate) dnd_active: bool,
    pub(crate) tab_tray: TabTrayState,
    pub(crate) palette: PaletteState,
    pub(crate) help_open: bool,
    pub(crate) help_scroll: usize,
    pub(crate) tour: TourState,
    pub(crate) attention_state: AttentionOverlayState,
    pub(crate) usage_state: UsageOverlayState,
    pub(crate) git_state: GitOverlayState,
    pub(crate) snippet_state: SnippetOverlayState,
    pub(crate) vault_state: VaultOverlayState,
    pub(crate) rename_prompt: Option<RenameModalState>,
    pub(crate) usage_store: UsageStore,
    pub(crate) snippets: SnippetBank,
    pub(crate) vault: VaultStore,
    pub(crate) git_backend: StandardGitBackend,
    pub(crate) event_tx: UnboundedSender<AppEvent>,
    pub(crate) is_dirty: bool,
    pub(crate) needs_clear: bool,
    pub(crate) term_width: u16,
    pub(crate) term_height: u16,
    pub(crate) cached_usage_summary: Option<String>,
    pub(crate) last_usage_query: std::time::Instant,
    pub(crate) git_cache: Option<CachedGitData>,
    pub(crate) terminal_focused: bool,
    pub(crate) last_notification_time: Option<std::time::Instant>,
    pub(crate) selecting_pane: Option<(PaneId, Rect, (u16, u16))>,
    pub(crate) available_update: Option<String>,
    /// Error from parsing config.toml (defaults in use; file must not be overwritten)
    pub(crate) config_error: Option<String>,
    /// First `q`/Ctrl+C press on an empty workspace while other panes still run
    pub(crate) quit_confirm_at: Option<std::time::Instant>,
    /// Token usage events parsed by the background transcript scanner
    pub(crate) usage_rx: std::sync::mpsc::Receiver<Vec<tag_usage::UsageEvent>>,
    /// Git overlay data computed off the UI thread, tagged with a request generation
    pub(crate) git_tx: std::sync::mpsc::Sender<(u64, CachedGitData)>,
    pub(crate) git_rx: std::sync::mpsc::Receiver<(u64, CachedGitData)>,
    pub(crate) git_generation: u64,
    pub(crate) git_loading: bool,
    /// Filesystem locations for this instance.
    pub(crate) paths: AppPaths,
    /// Publishes [`StateEvent`]s to control-plane subscribers.
    pub(crate) state_tx: broadcast::Sender<StateEvent>,
    /// Last published agent status per pane, used to detect transitions.
    pub(crate) last_status: HashMap<PaneId, String>,
    /// When each pane's status last changed (for `agent.status`).
    pub(crate) status_since: HashMap<PaneId, chrono::DateTime<chrono::Utc>>,
    /// Scoped control tokens minted for agents TAG launches.
    pub(crate) tokens: tag_control::TokenRegistry,
    /// Running control socket server, if enabled.
    pub(crate) control_server: Option<tag_control::ServerHandle>,
    /// Control-plane metadata per pane (profile, run/task, worktree, visibility).
    pub(crate) pane_meta: HashMap<PaneId, PaneMeta>,
    /// Prompts waiting for their agent to become ready (`prompt_delivery = "type"`).
    pub(crate) pending_prompts: HashMap<PaneId, PendingPrompt>,
    /// When each pane last produced output.
    pub(crate) last_output: HashMap<PaneId, std::time::Instant>,
    /// Last `tests.run` result per (canonical) worktree path.
    pub(crate) test_results: HashMap<std::path::PathBuf, TestRecord>,
    /// Orchestration runs managed by this instance, by run id.
    pub(crate) runs: HashMap<String, ActiveRun>,
    /// Goal being typed for `^Space o` (orchestrate), when the prompt is open.
    pub(crate) goal_prompt: Option<String>,
    /// Registry of known projects (git repo roots), persisted in the state dir.
    pub(crate) projects: tag_projects::ProjectRegistry,
    /// Set when `projects.json` failed to parse; saving is skipped so it is never overwritten.
    pub(crate) projects_load_error: Option<String>,
    /// Cached name of the active project for the status bar.
    pub(crate) active_project_name: Option<String>,
    /// Repo root the cached name was resolved from, and when it was last checked.
    pub(crate) active_project_root: Option<std::path::PathBuf>,
    pub(crate) last_project_check: std::time::Instant,
    /// Id of the project whose workspaces are in `layout_mgr`.
    pub(crate) active_project_id: Option<String>,
    /// Layouts of every other project (their panes stay in `panes`).
    pub(crate) parked: HashMap<String, ParkedProject>,
}

/// Capacity of the [`StateEvent`] broadcast channel.
pub(crate) const STATE_EVENT_CAPACITY: usize = 1024;

impl AppState {
    /// Build a fresh state with no panes.
    pub(crate) fn new(
        config: Config,
        config_error: Option<String>,
        paths: AppPaths,
        event_tx: UnboundedSender<AppEvent>,
        usage_rx: std::sync::mpsc::Receiver<Vec<tag_usage::UsageEvent>>,
    ) -> Result<Self, anyhow::Error> {
        let theme = Theme::from_config(config.theme.clone());
        let command_registry = CommandRegistry::new();
        let key_state_machine = KeyStateMachine::from_config(&config, &command_registry);
        let usage_store = UsageStore::open_in_memory()?;
        let snippets = SnippetBank::load_from_file_or_default(&paths.snippets_path);
        let vault = VaultStore::new(paths.vault_path.clone(), config.vault_idle_timeout_mins);
        let (git_tx, git_rx) = std::sync::mpsc::channel();
        let (state_tx, _) = broadcast::channel(STATE_EVENT_CAPACITY);
        let projects_path = paths.state_dir.join("projects.json");
        let (projects, projects_load_error) =
            match tag_projects::ProjectRegistry::load(&projects_path) {
                Ok(r) => (r, None),
                Err(e) => (
                    tag_projects::ProjectRegistry::empty(projects_path),
                    Some(e.to_string()),
                ),
            };
        Ok(AppState {
            panes: HashMap::new(),
            next_pane_id: 1,
            layout_mgr: LayoutManager::new(),
            key_state_machine,
            command_registry,
            theme,
            config,
            attention_queue: AttentionQueue::new(),
            toasts: Vec::new(),
            dnd_active: false,
            tab_tray: TabTrayState::default(),
            palette: PaletteState::default(),
            help_open: false,
            help_scroll: 0,
            tour: TourState::default(),
            attention_state: AttentionOverlayState::default(),
            usage_state: UsageOverlayState::default(),
            git_state: GitOverlayState::default(),
            snippet_state: SnippetOverlayState::default(),
            vault_state: VaultOverlayState::default(),
            rename_prompt: None,
            usage_store,
            snippets,
            vault,
            git_backend: StandardGitBackend,
            event_tx,
            is_dirty: true,
            needs_clear: false,
            term_width: 80,
            term_height: 24,
            cached_usage_summary: None,
            last_usage_query: std::time::Instant::now(),
            git_cache: None,
            terminal_focused: true,
            last_notification_time: None,
            selecting_pane: None,
            available_update: None,
            config_error,
            quit_confirm_at: None,
            usage_rx,
            git_tx,
            git_rx,
            git_generation: 0,
            git_loading: false,
            paths,
            state_tx,
            last_status: HashMap::new(),
            status_since: HashMap::new(),
            tokens: tag_control::TokenRegistry::default(),
            control_server: None,
            pane_meta: HashMap::new(),
            pending_prompts: HashMap::new(),
            last_output: HashMap::new(),
            test_results: HashMap::new(),
            runs: HashMap::new(),
            goal_prompt: None,
            projects,
            projects_load_error,
            active_project_name: None,
            active_project_root: None,
            last_project_check: std::time::Instant::now(),
            active_project_id: None,
            parked: HashMap::new(),
        })
    }

    /// Start the control socket server (no-op when `[control] enabled = false`).
    /// Returns an error message suitable for a toast on failure.
    pub(crate) async fn start_control_server(&mut self) -> Option<String> {
        if !self.config.control.enabled {
            return None;
        }
        let path = self.paths.control_socket_path();
        let mut settings =
            tag_control::ServerSettings::new(TAG_VERSION, self.config.control.clone());
        settings.audit_path = Some(self.paths.state_dir.join("control-audit.jsonl"));
        match tag_control::serve(
            path.clone(),
            settings,
            self.tokens.clone(),
            self.event_tx.clone(),
            self.state_tx.clone(),
        )
        .await
        {
            Ok(handle) => {
                std::env::set_var(tag_control::discovery::SOCKET_ENV, handle.path());
                self.control_server = Some(handle);
                None
            }
            Err(e) => Some(format!("Could not listen on {}: {}", path.display(), e)),
        }
    }

    fn focused_pane_cwd(&self) -> Option<std::path::PathBuf> {
        let id = self.layout_mgr.active_workspace().focused_pane?;
        self.panes.get(&id)?.cwd()
    }

    /// Resolve the active project from the focused pane's cwd, record it in the
    /// registry and persist (unless the registry failed to load).
    pub(crate) fn active_project(&mut self) -> Option<tag_projects::Project> {
        let cwd = self.focused_pane_cwd();
        let project = self.projects.resolve(cwd.as_deref())?;
        self.projects.set_active(&project.id);
        if self.projects_load_error.is_none() {
            let _ = self.projects.save();
        }
        Some(project)
    }

    /// Cheap refresh of the cached status-bar project name; only re-resolves
    /// (and saves) when the focused pane's repo root changed. Returns true if the name changed.
    pub(crate) fn refresh_active_project_name(&mut self) -> bool {
        self.last_project_check = std::time::Instant::now();
        let root = self
            .focused_pane_cwd()
            .and_then(|c| tag_projects::find_repo_root(&c));
        if root.is_some() && root == self.active_project_root {
            return false;
        }
        let before = self.active_project_name.clone();
        if root.is_some() {
            self.active_project_root = root;
        }
        self.active_project_name = self.active_project().map(|p| p.name);
        before != self.active_project_name
    }

    /// Publish a state event. Having no subscribers is not an error.
    pub(crate) fn publish(&self, event: StateEvent) {
        let _ = self.state_tx.send(event);
    }

    /// Compare every pane's current agent status with the last published one and emit
    /// [`StateEvent::PaneStatusChanged`] for each transition.
    pub(crate) fn publish_status_changes(&mut self) {
        let mut changes = Vec::new();
        for (&pid, pane) in &self.panes {
            if let Some(status) = pane.agent_status() {
                let status = normalize_status(status);
                let prev = self.last_status.get(&pid);
                if prev != Some(&status) {
                    changes.push((pid, prev.cloned(), status));
                }
            }
        }
        self.last_status
            .retain(|pid, _| self.panes.contains_key(pid));
        let now = chrono::Utc::now();
        self.status_since
            .retain(|pid, _| self.panes.contains_key(pid));
        for (pid, from, to) in changes {
            self.last_status.insert(pid, to.clone());
            self.status_since.insert(pid, now);
            self.publish(StateEvent::PaneStatusChanged {
                pane: pid,
                from,
                to,
                at: now,
            });
        }
    }
}

impl AppState {
    pub(crate) fn update_usage_summary(&mut self) {
        let today_usage = self.usage_store.query_today().ok();
        self.cached_usage_summary = today_usage.map(|u| {
            let total_tokens = u.input_tokens + u.output_tokens;
            let tok_str = if total_tokens >= 1_000_000 {
                let m = total_tokens as f64 / 1_000_000.0;
                if m >= 10.0 {
                    format!("{:.1}M tok", m)
                } else {
                    format!("{:.2}M tok", m)
                }
            } else if total_tokens >= 1_000 {
                let k = total_tokens as f64 / 1_000.0;
                if k >= 10.0 {
                    format!("{:.0}k tok", k)
                } else {
                    format!("{:.1}k tok", k)
                }
            } else {
                format!("{} tok", total_tokens)
            };
            format!("{} (${:.2})", tok_str, u.estimated_cost_usd)
        });
        self.last_usage_query = std::time::Instant::now();
    }
}

impl AppState {
    pub(crate) fn allocate_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ratatui::Frame;
    use tag_core::event::{EventResult, HookEvent, HookPayload};
    use tag_core::pane::PaneKind;

    /// A fake agent pane whose status is driven by hook payloads.
    pub(crate) struct MockAgentPane {
        pub(crate) id: PaneId,
        pub(crate) status: &'static str,
    }

    impl Pane for MockAgentPane {
        fn id(&self) -> PaneId {
            self.id
        }
        fn title(&self) -> String {
            "mock".into()
        }
        fn kind(&self) -> PaneKind {
            PaneKind::Agent
        }
        fn render(&mut self, _: &mut Frame, _: Rect, _: bool, _: &Theme) {}
        fn handle_event(&mut self, event: &AppEvent) -> EventResult {
            if let AppEvent::AgentHook(p) = event {
                self.status = match p.event {
                    HookEvent::NeedsInput | HookEvent::NeedsPermission => "awaiting-input",
                    HookEvent::TaskDone => "done",
                    _ => self.status,
                };
            }
            EventResult::Consumed
        }
        fn on_resize(&mut self, _: u16, _: u16) {}
        fn is_active(&self) -> bool {
            true
        }
        fn agent_status(&self) -> Option<&'static str> {
            Some(self.status)
        }
    }

    /// An `AppState` rooted in a unique temp dir, with notifications disabled.
    pub(crate) fn test_state() -> (AppState, tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
        let dir =
            std::env::temp_dir().join(format!("tag-test-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = AppPaths::isolated(
            Some(dir.join("config.toml")),
            dir.join("state"),
            dir.join("sock"),
        );
        let mut config = Config::default();
        config.notifications.enabled = false;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (_usage_tx, usage_rx) = std::sync::mpsc::channel();
        let state = AppState::new(config, None, paths, tx, usage_rx).unwrap();
        (state, rx)
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn status_change_from_hook_is_broadcast() {
        let (mut state, _rx) = test_state();
        let pid = PaneId(7);
        state.panes.insert(
            pid,
            Box::new(MockAgentPane {
                id: pid,
                status: "running",
            }),
        );
        let mut sub = state.state_tx.subscribe();

        // First observation publishes the initial status.
        state.publish_status_changes();
        match sub.try_recv().unwrap() {
            StateEvent::PaneStatusChanged { pane, from, to, .. } => {
                assert_eq!(pane, pid);
                assert_eq!(from, None);
                assert_eq!(to, "running");
            }
            other => panic!("unexpected {:?}", other),
        }

        let payload = HookPayload {
            event: HookEvent::NeedsInput,
            pane_id: Some(pid),
            agent: "fake".into(),
            worktree: None,
            message: "needs input".into(),
            timestamp: chrono::Utc::now(),
        };
        handle_app_event(&mut state, AppEvent::AgentHook(payload));

        let mut saw_attention = false;
        let mut saw_status = false;
        while let Ok(ev) = sub.try_recv() {
            match ev {
                StateEvent::AttentionAdded { pane, .. } => {
                    assert_eq!(pane, Some(pid));
                    saw_attention = true;
                }
                StateEvent::PaneStatusChanged { from, to, .. } => {
                    assert_eq!(from.as_deref(), Some("running"));
                    assert_eq!(to, "awaiting_input");
                    saw_status = true;
                }
                other => panic!("unexpected {:?}", other),
            }
        }
        assert!(saw_attention && saw_status);

        // No change → no event.
        state.publish_status_changes();
        assert!(sub.try_recv().is_err());
    }
}
