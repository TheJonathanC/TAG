#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;
use tag_applets::{PeerChatPane, PokerPane};
use tag_core::pane::{Pane, PaneId};
use tag_ui::{Toast, ToastLevel};

pub(crate) fn make_applet(name: &str, pid: PaneId) -> Option<Box<dyn Pane>> {
    match name {
        "poker" => Some(Box::new(PokerPane::new(pid))),
        "chat" => Some(Box::new(PeerChatPane::new(pid))),
        _ => None,
    }
}

pub(crate) fn push_toast(
    state: &mut AppState,
    title: &str,
    message: impl Into<String>,
    level: ToastLevel,
    duration_secs: u64,
) {
    state.toasts.push(Toast {
        title: title.into(),
        message: message.into(),
        level,
        created_at: chrono::Utc::now(),
        duration_secs,
    });
    state.is_dirty = true;
}

pub(crate) fn vault_env(state: &mut AppState) -> HashMap<String, String> {
    if state.vault.is_unlocked() {
        state.vault.get_env_set()
    } else {
        HashMap::new()
    }
}

/// Size for a newly spawned pane before its first render resizes it.
pub(crate) fn initial_pane_size(state: &AppState) -> (u16, u16) {
    if state.term_width > 0 && state.term_height > 0 {
        (
            state.term_width.saturating_sub(2),
            state.term_height.saturating_sub(4),
        )
    } else {
        (80, 24)
    }
}

/// Screen area used by panes (excludes the status bar and an open tab tray).
pub(crate) fn content_area(state: &AppState) -> Rect {
    let height = state.term_height.saturating_sub(1);
    let tray_w = tray_width(state);
    Rect {
        x: tray_w,
        y: 0,
        width: state.term_width.saturating_sub(tray_w),
        height,
    }
}

pub(crate) fn tray_width(state: &AppState) -> u16 {
    if state.tab_tray.is_open {
        (state.term_width * 25 / 100)
            .clamp(24, 45)
            .min(state.term_width)
    } else {
        0
    }
}

/// Current directory of the focused pane (falls back to its spawn directory).
pub(crate) fn focused_pane_cwd(state: &AppState) -> Option<PathBuf> {
    let pid = state.layout_mgr.active_workspace().focused_pane?;
    let pane = state.panes.get(&pid)?;
    let (_, _, cwd) = pane.running_process();
    cwd.or_else(|| pane.cwd()).filter(|p| p.is_dir())
}

/// Focus a pane wherever it lives: switch to its workspace and restore it if minimized.
pub(crate) fn focus_pane_anywhere(state: &mut AppState, pid: PaneId) -> bool {
    let Some(ws_id) = state.layout_mgr.find_pane_workspace(pid) else {
        return false;
    };
    state.layout_mgr.switch_workspace(ws_id);
    let ws = state.layout_mgr.active_workspace_mut();
    if ws.is_minimized(pid) {
        ws.restore_pane(pid, None);
    }
    ws.focused_pane = Some(pid);
    state.needs_clear = true;
    true
}

/// Close a pane: remove it from every workspace, kill its process and drop its alerts.
pub(crate) fn close_pane(state: &mut AppState, pid: PaneId) {
    for ws in state.layout_mgr.workspaces_mut() {
        ws.remove_pane(pid);
    }
    if let Some(mut pane) = state.panes.remove(&pid) {
        pane.kill();
    }
    state.attention_queue.clear_by_pane(pid);
    state.needs_clear = true;
    save_session_to_disk(state);
}
