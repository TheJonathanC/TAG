use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
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
    pub minimized_panes: Vec<PaneId>,
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
            minimized_panes: Vec::new(),
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

    /// Restore the previous layout tree.
    ///
    /// Panes that no longer exist (closed or exited since the snapshot) are pruned from the
    /// restored tree, panes that the restored tree no longer shows are moved to the minimized
    /// list so they stay reachable, and focus/zoom are kept consistent.
    pub fn undo_layout(&mut self) -> bool {
        let current = self.all_panes();
        let live: HashSet<PaneId> = current
            .iter()
            .chain(self.minimized_panes.iter())
            .copied()
            .collect();

        while let Some(prev) = self.undo_stack.pop() {
            let mut restored = Some(prev);
            for pid in restored.as_ref().map(|r| r.all_panes()).unwrap_or_default() {
                if !live.contains(&pid) {
                    restored = restored.and_then(|r| r.remove_pane(pid));
                }
            }
            let Some(restored) = restored else {
                continue; // snapshot only contained panes that are gone
            };
            if Some(&restored) == self.root.as_ref() {
                continue; // nothing would change
            }

            let shown = restored.all_panes();
            self.minimized_panes.retain(|p| !shown.contains(p));
            for pid in current.iter().filter(|p| !shown.contains(p)) {
                if !self.minimized_panes.contains(pid) {
                    self.minimized_panes.push(*pid);
                }
            }
            if self.zoomed_pane.is_some_and(|z| !shown.contains(&z)) {
                self.zoomed_pane = None;
            }
            if !self.focused_pane.is_some_and(|f| shown.contains(&f)) {
                self.focused_pane = shown.first().copied();
            }
            self.root = Some(restored);
            return true;
        }
        false
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
        if self.zoomed_pane.is_some() {
            return Err("Cannot split while a pane is maximized");
        }
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
        self.minimized_panes.retain(|&p| p != pane_id);
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

    pub fn minimize_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.all_panes().contains(&pane_id) {
            return false;
        }
        self.save_undo_state();
        if self.zoomed_pane == Some(pane_id) {
            self.zoomed_pane = None;
        }
        if let Some(root) = self.root.take() {
            self.root = root.remove_pane(pane_id);
            if self.focused_pane == Some(pane_id) {
                self.focused_pane = self.all_panes().first().copied();
            }
        }
        if !self.minimized_panes.contains(&pane_id) {
            self.minimized_panes.push(pane_id);
        }
        true
    }

    pub fn restore_pane(&mut self, pane_id: PaneId, direction: Option<SplitDirection>) -> bool {
        if let Some(pos) = self.minimized_panes.iter().position(|&p| p == pane_id) {
            self.minimized_panes.remove(pos);
            // A maximized pane would block the split and lose the restored pane
            self.zoomed_pane = None;
            if self.root.is_none() || self.focused_pane.is_none() {
                self.save_undo_state();
                self.add_initial_pane(pane_id);
            } else {
                let dir = direction.unwrap_or(SplitDirection::Vertical);
                let _ = self.split_current(pane_id, dir);
            }
            self.focused_pane = Some(pane_id);
            true
        } else {
            false
        }
    }

    pub fn is_minimized(&self, pane_id: PaneId) -> bool {
        self.minimized_panes.contains(&pane_id)
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

    /// Find the pane adjacent to `from` in `dir`, using the on-screen geometry.
    pub fn neighbor_in_direction(
        &self,
        area: Rect,
        from: PaneId,
        dir: Direction,
    ) -> Option<PaneId> {
        let rects = self.compute_rects(area);
        let cur = *rects.get(&from)?;
        let (cx0, cx1) = (cur.x as i32, (cur.x + cur.width) as i32);
        let (cy0, cy1) = (cur.y as i32, (cur.y + cur.height) as i32);

        rects
            .iter()
            .filter(|(id, _)| **id != from)
            .filter_map(|(id, r)| {
                let (x0, x1) = (r.x as i32, (r.x + r.width) as i32);
                let (y0, y1) = (r.y as i32, (r.y + r.height) as i32);
                // gap along the movement axis, overlap on the perpendicular axis
                let (gap, overlap, offset) = match dir {
                    Direction::Right if x0 >= cx1 => {
                        (x0 - cx1, y1.min(cy1) - y0.max(cy0), (y0 - cy0).abs())
                    }
                    Direction::Left if x1 <= cx0 => {
                        (cx0 - x1, y1.min(cy1) - y0.max(cy0), (y0 - cy0).abs())
                    }
                    Direction::Down if y0 >= cy1 => {
                        (y0 - cy1, x1.min(cx1) - x0.max(cx0), (x0 - cx0).abs())
                    }
                    Direction::Up if y1 <= cy0 => {
                        (cy0 - y1, x1.min(cx1) - x0.max(cx0), (x0 - cx0).abs())
                    }
                    _ => return None,
                };
                // Prefer panes that overlap on the other axis, then the closest, then aligned
                Some((((overlap <= 0) as i32, gap, offset, id.0), *id))
            })
            .min_by_key(|(score, _)| *score)
            .map(|(_, id)| id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimize_and_restore() {
        let mut ws = Workspace::new(WorkspaceId(1), "Test");
        ws.add_initial_pane(PaneId(1));
        assert!(ws
            .split_current(PaneId(2), SplitDirection::Vertical)
            .is_ok());

        assert_eq!(ws.all_panes(), vec![PaneId(1), PaneId(2)]);
        assert!(ws.minimized_panes.is_empty());

        // Minimize Pane 2
        assert!(ws.minimize_pane(PaneId(2)));
        assert_eq!(ws.all_panes(), vec![PaneId(1)]);
        assert_eq!(ws.minimized_panes, vec![PaneId(2)]);
        assert!(ws.is_minimized(PaneId(2)));
        assert!(!ws.is_minimized(PaneId(1)));

        // Restore Pane 2
        assert!(ws.restore_pane(PaneId(2), Some(SplitDirection::Horizontal)));
        assert_eq!(ws.minimized_panes.len(), 0);
        assert!(ws.all_panes().contains(&PaneId(2)));
        assert_eq!(ws.focused_pane, Some(PaneId(2)));
    }

    #[test]
    fn test_undo_does_not_resurrect_closed_or_orphan_panes() {
        let mut ws = Workspace::new(WorkspaceId(1), "Test");
        ws.add_initial_pane(PaneId(1));
        ws.split_current(PaneId(2), SplitDirection::Vertical)
            .unwrap();
        ws.split_current(PaneId(3), SplitDirection::Horizontal)
            .unwrap();

        // Close pane 2, then undo: pane 2 must not come back
        ws.remove_pane(PaneId(2));
        assert!(ws.undo_layout());
        assert!(!ws.all_panes().contains(&PaneId(2)));

        // Undoing the split of pane 3 must keep pane 3 reachable (minimized)
        while ws.undo_layout() {}
        let visible = ws.all_panes();
        assert!(visible.contains(&PaneId(1)));
        assert!(visible.contains(&PaneId(3)) || ws.minimized_panes.contains(&PaneId(3)));
        assert!(ws.focused_pane.is_some_and(|f| visible.contains(&f)));
    }

    #[test]
    fn test_restore_while_zoomed_keeps_pane() {
        let mut ws = Workspace::new(WorkspaceId(1), "Test");
        ws.add_initial_pane(PaneId(1));
        ws.split_current(PaneId(2), SplitDirection::Vertical)
            .unwrap();
        ws.minimize_pane(PaneId(2));
        ws.toggle_zoom();
        assert!(ws.restore_pane(PaneId(2), None));
        assert!(ws.all_panes().contains(&PaneId(2)));
        assert!(ws.zoomed_pane.is_none());
    }

    #[test]
    fn test_neighbor_in_direction_is_spatial() {
        // Layout: 1 | (2 / 3)
        let mut ws = Workspace::new(WorkspaceId(1), "Test");
        ws.add_initial_pane(PaneId(1));
        ws.split_current(PaneId(2), SplitDirection::Vertical)
            .unwrap();
        ws.split_current(PaneId(3), SplitDirection::Horizontal)
            .unwrap();
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(
            ws.neighbor_in_direction(area, PaneId(1), Direction::Right),
            Some(PaneId(2))
        );
        assert_eq!(
            ws.neighbor_in_direction(area, PaneId(2), Direction::Down),
            Some(PaneId(3))
        );
        assert_eq!(
            ws.neighbor_in_direction(area, PaneId(3), Direction::Up),
            Some(PaneId(2))
        );
        assert_eq!(
            ws.neighbor_in_direction(area, PaneId(3), Direction::Left),
            Some(PaneId(1))
        );
        assert_eq!(
            ws.neighbor_in_direction(area, PaneId(1), Direction::Left),
            None
        );
    }

    #[test]
    fn test_remove_minimized_pane() {
        let mut ws = Workspace::new(WorkspaceId(1), "Test");
        ws.add_initial_pane(PaneId(1));
        assert!(ws
            .split_current(PaneId(2), SplitDirection::Vertical)
            .is_ok());

        assert!(ws.minimize_pane(PaneId(2)));
        assert_eq!(ws.minimized_panes, vec![PaneId(2)]);

        ws.remove_pane(PaneId(2));
        assert_eq!(ws.minimized_panes.len(), 0);
    }
}
