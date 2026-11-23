#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::time::Duration;
use tag_clipboard::{SnippetBank, SnippetOverlayState};
use tag_core::command::CommandRegistry;
use tag_core::config::Config;
use tag_core::event::AppEvent;
use tag_core::pane::{Pane, PaneId};
use tag_core::theme::Theme;
use tag_git::{GitOverlayState, StandardGitBackend};
use tag_keys::KeyStateMachine;
use tag_layout::LayoutManager;
use tag_notify::{AttentionOverlayState, AttentionQueue};
use tag_ui::{PaletteState, RenameModalState, TabTrayState, Toast, TourState};
use tag_usage::{UsageOverlayState, UsageStore};
use tag_vault::{VaultOverlayState, VaultStore};
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
