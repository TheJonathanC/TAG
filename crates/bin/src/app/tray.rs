#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use tag_ui::TrayItem;

pub(crate) fn build_tray_items(state: &AppState) -> Vec<TrayItem> {
    let mut tray_items = Vec::new();
    for ws in state.layout_mgr.workspaces() {
        for pid in ws.all_panes() {
            if let Some(p) = state.panes.get(&pid) {
                tray_items.push(TrayItem {
                    pane_id: pid,
                    workspace_id: ws.id,
                    workspace_name: ws.name.clone(),
                    title: p.title(),
                    icon: p.kind().icon(),
                    branch: p.branch(),
                    status: p.agent_status().unwrap_or("running"),
                    is_visible: ws.id == state.layout_mgr.active_workspace_id(),
                    needs_attention: false,
                    is_minimized: false,
                });
            }
        }
        for &pid in &ws.minimized_panes {
            if let Some(p) = state.panes.get(&pid) {
                tray_items.push(TrayItem {
                    pane_id: pid,
                    workspace_id: ws.id,
                    workspace_name: ws.name.clone(),
                    title: p.title(),
                    icon: p.kind().icon(),
                    branch: p.branch(),
                    status: p.agent_status().unwrap_or("running"),
                    is_visible: false,
                    needs_attention: false,
                    is_minimized: true,
                });
            }
        }
    }
    tray_items
}
