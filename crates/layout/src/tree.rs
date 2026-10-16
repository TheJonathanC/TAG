use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use tag_core::action::{Direction, SplitDirection};
use tag_core::pane::PaneId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32, // 0.1 to 0.9, typically 0.5
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn leaf(pane_id: PaneId) -> Self {
        LayoutNode::Leaf(pane_id)
    }

    pub fn split_pane(self, target: PaneId, new_pane: PaneId, direction: SplitDirection) -> Self {
        match self {
            LayoutNode::Leaf(id) if id == target => LayoutNode::Split {
                direction,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(id)),
                second: Box::new(LayoutNode::Leaf(new_pane)),
            },
            LayoutNode::Leaf(id) => LayoutNode::Leaf(id),
            LayoutNode::Split {
                direction: dir,
                ratio,
                first,
                second,
            } => LayoutNode::Split {
                direction: dir,
                ratio,
                first: Box::new(first.split_pane(target, new_pane, direction.clone())),
                second: Box::new(second.split_pane(target, new_pane, direction)),
            },
        }
    }

    pub fn remove_pane(self, target: PaneId) -> Option<Self> {
        match self {
            LayoutNode::Leaf(id) => {
                if id == target {
                    None
                } else {
                    Some(LayoutNode::Leaf(id))
                }
            }
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_opt = first.remove_pane(target);
                let second_opt = second.remove_pane(target);

                match (first_opt, second_opt) {
                    (Some(f), Some(s)) => Some(LayoutNode::Split {
                        direction,
                        ratio,
                        first: Box::new(f),
                        second: Box::new(s),
                    }),
                    (Some(f), None) => Some(f),
                    (None, Some(s)) => Some(s),
                    (None, None) => None,
                }
            }
        }
    }

    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) {
        match self {
            LayoutNode::Leaf(id) => {
                if *id == a {
                    *id = b;
                } else if *id == b {
                    *id = a;
                }
            }
            LayoutNode::Split { first, second, .. } => {
                first.swap_panes(a, b);
                second.swap_panes(a, b);
            }
        }
    }

    pub fn contains(&self, target: PaneId) -> bool {
        match self {
            LayoutNode::Leaf(id) => *id == target,
            LayoutNode::Split { first, second, .. } => {
                first.contains(target) || second.contains(target)
            }
        }
    }

    pub fn all_panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    fn collect_panes(&self, panes: &mut Vec<PaneId>) {
        match self {
            LayoutNode::Leaf(id) => panes.push(*id),
            LayoutNode::Split { first, second, .. } => {
                first.collect_panes(panes);
                second.collect_panes(panes);
            }
        }
    }

    pub fn adjust_ratio(&mut self, target: PaneId, dir: Direction, delta: f32) {
        if let LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } = self
        {
            let in_first = first.contains(target);
            let in_second = second.contains(target);

            if in_first || in_second {
                let affects_this_split = matches!(
                    (direction, &dir),
                    (SplitDirection::Vertical, Direction::Left | Direction::Right)
                        | (SplitDirection::Horizontal, Direction::Up | Direction::Down)
                );

                if affects_this_split {
                    let d = match dir {
                        Direction::Right | Direction::Down => delta,
                        Direction::Left | Direction::Up => -delta,
                    };
                    *ratio = (*ratio + d).clamp(0.1, 0.9);
                } else if in_first {
                    first.adjust_ratio(target, dir, delta);
                } else {
                    second.adjust_ratio(target, dir, delta);
                }
            }
        }
    }

    pub fn compute_rects(&self, area: Rect) -> HashMap<PaneId, Rect> {
        let mut rects = HashMap::new();
        self.calculate_rects(area, &mut rects);
        rects
    }

    fn calculate_rects(&self, area: Rect, map: &mut HashMap<PaneId, Rect>) {
        match self {
            LayoutNode::Leaf(id) => {
                map.insert(*id, area);
            }
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => match direction {
                SplitDirection::Vertical => {
                    let max_w = area.width.saturating_sub(4);
                    let first_width = if max_w >= 4 {
                        let w = ((area.width as f32) * ratio).round() as u16;
                        w.clamp(4, max_w)
                    } else {
                        area.width / 2
                    };
                    let second_width = area.width.saturating_sub(first_width);

                    let first_rect = Rect {
                        x: area.x,
                        y: area.y,
                        width: first_width,
                        height: area.height,
                    };
                    let second_rect = Rect {
                        x: area.x + first_width,
                        y: area.y,
                        width: second_width,
                        height: area.height,
                    };

                    first.calculate_rects(first_rect, map);
                    second.calculate_rects(second_rect, map);
                }
                SplitDirection::Horizontal => {
                    let max_h = area.height.saturating_sub(2);
                    let first_height = if max_h >= 2 {
                        let h = ((area.height as f32) * ratio).round() as u16;
                        h.clamp(2, max_h)
                    } else {
                        area.height / 2
                    };
                    let second_height = area.height.saturating_sub(first_height);

                    let first_rect = Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width,
                        height: first_height,
                    };
                    let second_rect = Rect {
                        x: area.x,
                        y: area.y + first_height,
                        width: area.width,
                        height: second_height,
                    };

                    first.calculate_rects(first_rect, map);
                    second.calculate_rects(second_rect, map);
                }
            },
        }
    }

    pub fn hit_test(&self, area: Rect, col: u16, row: u16) -> Option<PaneId> {
        let rects = self.compute_rects(area);
        for (pane_id, rect) in rects {
            if col >= rect.x
                && col < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height
            {
                return Some(pane_id);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_split_and_compute() {
        let root = LayoutNode::leaf(PaneId(1));
        let root = root.split_pane(PaneId(1), PaneId(2), SplitDirection::Vertical);

        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let rects = root.compute_rects(area);

        assert_eq!(rects.len(), 2);
        assert_eq!(rects.get(&PaneId(1)).unwrap().width, 50);
        assert_eq!(rects.get(&PaneId(2)).unwrap().width, 50);
        assert_eq!(rects.get(&PaneId(2)).unwrap().x, 50);
    }

    #[test]
    fn test_tree_split_small_area_no_panic() {
        let root = LayoutNode::leaf(PaneId(1));
        let root = root.split_pane(PaneId(1), PaneId(2), SplitDirection::Vertical);
        let root = root.split_pane(PaneId(2), PaneId(3), SplitDirection::Horizontal);

        // Very small area where width < 8 and height < 4 (previously panicked on clamp min > max)
        let area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        };
        let rects = root.compute_rects(area);
        assert_eq!(rects.len(), 3);
    }
}
