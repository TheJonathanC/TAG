use ratatui::layout::Rect;
use tag_core::action::SplitDirection;
use tag_core::pane::{PaneId, WorkspaceId};
use tag_layout::{LayoutManager, Workspace};

#[test]
fn test_complex_layout_tree() {
    let mut ws = Workspace::new(WorkspaceId(1), "Dev");
    ws.add_initial_pane(PaneId(1));

    // Split pane 1 vertically with pane 2
    ws.split_current(PaneId(2), SplitDirection::Vertical)
        .unwrap();
    assert_eq!(ws.all_panes(), vec![PaneId(1), PaneId(2)]);

    // Split focused pane (pane 2) horizontally with pane 3
    ws.split_current(PaneId(3), SplitDirection::Horizontal)
        .unwrap();
    assert_eq!(ws.all_panes(), vec![PaneId(1), PaneId(2), PaneId(3)]);

    // Check hit test
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let hit_left = ws.hit_test(area, 10, 10);
    assert_eq!(hit_left, Some(PaneId(1)));

    // Zoom toggle
    ws.toggle_zoom();
    assert_eq!(ws.zoomed_pane, Some(PaneId(3)));
    let rects = ws.compute_rects(area);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects.get(&PaneId(3)).unwrap(), &area);

    ws.toggle_zoom();
    assert_eq!(ws.zoomed_pane, None);

    // Test undo
    let undone = ws.undo_layout();
    assert!(undone);
    assert_eq!(ws.all_panes(), vec![PaneId(1), PaneId(2)]);
}

#[test]
fn test_layout_manager_workspace_switching() {
    let mut mgr = LayoutManager::new();
    let ws2_id = mgr.add_workspace("Secondary");
    assert_eq!(mgr.workspaces().len(), 2);
    assert_eq!(mgr.active_workspace_id(), ws2_id);

    mgr.switch_workspace(WorkspaceId(1));
    assert_eq!(mgr.active_workspace_id(), WorkspaceId(1));
}
