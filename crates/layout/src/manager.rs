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
        let next_id = WorkspaceId(self.workspaces.len() + 1);
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
            let panes = removed.all_panes();
            if self.active_idx >= self.workspaces.len() {
                self.active_idx = self.workspaces.len().saturating_sub(1);
            }
            (true, panes)
        } else {
            (false, Vec::new())
        }
    }

    pub fn compute_rects(&self, area: Rect) -> HashMap<PaneId, Rect> {
        self.active_workspace().compute_rects(area)
    }

    pub fn hit_test(&self, area: Rect, col: u16, row: u16) -> Option<PaneId> {
        self.active_workspace().hit_test(area, col, row)
    }
}
