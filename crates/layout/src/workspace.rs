use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;

use tag_core::action::{Direction, SplitDirection};
use tag_core::pane::{PaneId, WorkspaceId};

use crate::tree::LayoutNode;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub root: Option<LayoutNode>,
    pub focused_pane: Option<PaneId>,
    pub zoomed_pane: Option<PaneId>,
    pub worktree_binding: Option<PathBuf>,
    pub undo_stack: Vec<LayoutNode>,
}

impl Workspace {
    pub fn new(id: WorkspaceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            root: None,
            focused_pane: None,
            zoomed_pane: None,
            worktree_binding: None,
            undo_stack: Vec::new(),
        }
    }

    pub fn save_undo_state(&mut self) {
        if let Some(ref root) = self.root {
            self.undo_stack.push(root.clone());
            if self.undo_stack.len() > 20 {
                self.undo_stack.remove(0);
            }
        }
    }

    pub fn undo_layout(&mut self) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            self.root = Some(prev);
            true
        } else {
            false
        }
    }

    pub fn add_initial_pane(&mut self, pane_id: PaneId) {
        self.root = Some(LayoutNode::leaf(pane_id));
        self.focused_pane = Some(pane_id);
    }

    pub fn split_current(
        &mut self,
        new_pane_id: PaneId,
        direction: SplitDirection,
    ) -> Result<(), &'static str> {
        if self.root.is_none() || self.focused_pane.is_none() {
            self.add_initial_pane(new_pane_id);
            return Ok(());
        }

        let focused = self.focused_pane.unwrap();
        self.save_undo_state();

        if let Some(root) = self.root.take() {
            let new_root = root.split_pane(focused, new_pane_id, direction);
            self.root = Some(new_root);
            self.focused_pane = Some(new_pane_id);
            Ok(())
        } else {
            self.root = Some(LayoutNode::leaf(new_pane_id));
            self.focused_pane = Some(new_pane_id);
            Ok(())
        }
    }

    pub fn remove_pane(&mut self, pane_id: PaneId) {
        self.save_undo_state();
        if let Some(root) = self.root.take() {
            self.root = root.remove_pane(pane_id);
            if self.zoomed_pane == Some(pane_id) {
                self.zoomed_pane = None;
            }
            if self.focused_pane == Some(pane_id) {
                self.focused_pane = self.all_panes().first().copied();
            }
        }
    }

    pub fn toggle_zoom(&mut self) {
        if self.zoomed_pane.is_some() {
            self.zoomed_pane = None;
        } else {
            self.zoomed_pane = self.focused_pane;
        }
    }

    pub fn all_panes(&self) -> Vec<PaneId> {
        if let Some(ref root) = self.root {
            root.all_panes()
        } else {
            Vec::new()
        }
    }

    pub fn compute_rects(&self, area: Rect) -> HashMap<PaneId, Rect> {
        if let Some(zoomed) = self.zoomed_pane {
            let mut map = HashMap::new();
            map.insert(zoomed, area);
            return map;
        }

        if let Some(ref root) = self.root {
            root.compute_rects(area)
        } else {
            HashMap::new()
        }
    }

    pub fn hit_test(&self, area: Rect, col: u16, row: u16) -> Option<PaneId> {
        if let Some(zoomed) = self.zoomed_pane {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                return Some(zoomed);
            }
            return None;
        }

        if let Some(ref root) = self.root {
            root.hit_test(area, col, row)
        } else {
            None
        }
    }

    pub fn resize_focused(&mut self, dir: Direction, delta: f32) {
        if let Some(focused) = self.focused_pane {
            self.save_undo_state();
            if let Some(ref mut root) = self.root {
                root.adjust_ratio(focused, dir, delta);
            }
        }
    }
}
