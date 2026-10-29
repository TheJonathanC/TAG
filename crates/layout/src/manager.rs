use ratatui::layout::Rect;
use std::collections::HashMap;

use tag_core::pane::{PaneId, WorkspaceId};

use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct LayoutManager {
    workspaces: Vec<Workspace>,
    active_idx: usize,
}

impl Default for LayoutManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutManager {
    pub fn new() -> Self {
        let default_ws = Workspace::new(WorkspaceId(1), "Main");
        Self {
            workspaces: vec![default_ws],
            active_idx: 0,
        }
    }

    pub fn from_workspaces(workspaces: Vec<Workspace>, active_id: WorkspaceId) -> Self {
        if workspaces.is_empty() {
            return Self::new();
        }
        let active_idx = workspaces
            .iter()
            .position(|w| w.id == active_id)
            .unwrap_or(0);
        Self {
            workspaces,
            active_idx,
        }
    }

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_idx]
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_idx]
    }

    pub fn active_workspace_id(&self) -> WorkspaceId {
        self.workspaces[self.active_idx].id
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn workspaces_mut(&mut self) -> &mut [Workspace] {
        &mut self.workspaces
    }

    pub fn find_workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn find_pane_workspace(&self, pid: PaneId) -> Option<WorkspaceId> {
        for ws in &self.workspaces {
            if ws.all_panes().contains(&pid) || ws.minimized_panes.contains(&pid) {
                return Some(ws.id);
            }
        }
        None
    }

    pub fn switch_workspace(&mut self, id: WorkspaceId) -> bool {
        if let Some(idx) = self.workspaces.iter().position(|w| w.id == id) {
            self.active_idx = idx;
            true
        } else {
            false
        }
    }

    pub fn switch_workspace_index(&mut self, index: usize) -> bool {
        if index < self.workspaces.len() {
            self.active_idx = index;
            true
        } else {
            false
        }
    }

    pub fn next_workspace(&mut self) {
        if !self.workspaces.is_empty() {
            self.active_idx = (self.active_idx + 1) % self.workspaces.len();
        }
    }

    pub fn prev_workspace(&mut self) {
        if !self.workspaces.is_empty() {
            self.active_idx = if self.active_idx == 0 {
                self.workspaces.len() - 1
            } else {
                self.active_idx - 1
            };
        }
    }

    pub fn active_idx(&self) -> usize {
        self.active_idx
    }

    pub fn add_workspace(&mut self, name: impl Into<String>) -> WorkspaceId {
        // IDs must stay unique even after workspaces are closed
        let next_id = WorkspaceId(self.workspaces.iter().map(|w| w.id.0).max().unwrap_or(0) + 1);
        let ws = Workspace::new(next_id, name);
        self.workspaces.push(ws);
        self.active_idx = self.workspaces.len() - 1;
        next_id
    }

    pub fn close_workspace(&mut self, id: WorkspaceId) -> (bool, Vec<PaneId>) {
        if self.workspaces.len() <= 1 {
            return (false, Vec::new());
        }
        if let Some(pos) = self.workspaces.iter().position(|w| w.id == id) {
            let removed = self.workspaces.remove(pos);
            let mut panes = removed.all_panes();
            panes.extend(removed.minimized_panes.iter().copied());
            if self.active_idx >= self.workspaces.len() {
                self.active_idx = self.workspaces.len().saturating_sub(1);
            }
            (true, panes)
        } else {
            (false, Vec::new())
        }
    }

    /// Drop references to panes that do not exist (e.g. failed to respawn on session restore),
    /// keeping focus and zoom pointing at real panes.
    pub fn retain_panes(&mut self, live: &dyn Fn(PaneId) -> bool) {
        for ws in &mut self.workspaces {
            for pid in ws.all_panes() {
                if !live(pid) {
                    if let Some(root) = ws.root.take() {
                        ws.root = root.remove_pane(pid);
                    }
                }
            }
            ws.minimized_panes.retain(|p| live(*p));
            let shown = ws.all_panes();
            if ws.zoomed_pane.is_some_and(|z| !shown.contains(&z)) {
                ws.zoomed_pane = None;
            }
            if !ws.focused_pane.is_some_and(|f| shown.contains(&f)) {
                ws.focused_pane = shown.first().copied();
            }
            ws.undo_stack.clear();
        }
    }

    pub fn compute_rects(&self, area: Rect) -> HashMap<PaneId, Rect> {
        self.active_workspace().compute_rects(area)
    }

    pub fn hit_test(&self, area: Rect, col: u16, row: u16) -> Option<PaneId> {
        self.active_workspace().hit_test(area, col, row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_ids_unique_after_close() {
        let mut mgr = LayoutManager::new();
        let w2 = mgr.add_workspace("WS-2");
        let w3 = mgr.add_workspace("WS-3");
        assert!(mgr.close_workspace(w2).0);
        let w4 = mgr.add_workspace("WS-4");
        assert_ne!(w4, w3);
        let ids: Vec<_> = mgr.workspaces().iter().map(|w| w.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids
            .iter()
            .all(|id| ids.iter().filter(|o| *o == id).count() == 1));
    }

    #[test]
    fn test_close_workspace_returns_minimized_panes() {
        let mut mgr = LayoutManager::new();
        let ws = mgr.add_workspace("WS-2");
        mgr.active_workspace_mut().add_initial_pane(PaneId(1));
        let _ = mgr
            .active_workspace_mut()
            .split_current(PaneId(2), tag_core::action::SplitDirection::Vertical);
        mgr.active_workspace_mut().minimize_pane(PaneId(2));
        let (closed, panes) = mgr.close_workspace(ws);
        assert!(closed);
        assert!(panes.contains(&PaneId(1)) && panes.contains(&PaneId(2)));
    }

    #[test]
    fn test_from_empty_workspaces_does_not_panic() {
        let mgr = LayoutManager::from_workspaces(Vec::new(), WorkspaceId(7));
        assert_eq!(mgr.workspaces().len(), 1);
        let _ = mgr.active_workspace();
    }

    #[test]
    fn test_retain_panes_drops_missing() {
        let mut mgr = LayoutManager::new();
        mgr.active_workspace_mut().add_initial_pane(PaneId(1));
        let _ = mgr
            .active_workspace_mut()
            .split_current(PaneId(2), tag_core::action::SplitDirection::Vertical);
        mgr.retain_panes(&|p| p == PaneId(1));
        assert_eq!(mgr.active_workspace().all_panes(), vec![PaneId(1)]);
        assert_eq!(mgr.active_workspace().focused_pane, Some(PaneId(1)));
    }
}
