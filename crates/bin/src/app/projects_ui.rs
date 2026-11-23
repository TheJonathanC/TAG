//! Project switcher overlay glue and cross-project navigation (last workspace, next agent
//! needing input).

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::Frame;
use std::path::PathBuf;
use tag_core::pane::{PaneId, WorkspaceId};
use tag_layout::LayoutManager;
use tag_orchestrator::Visibility;
use tag_ui::{
    preview_area, render_project_switcher, switcher_rows, SwitcherPane, SwitcherProject,
    SwitcherTarget, SwitcherWorkspace, ToastLevel,
};

/// Current and previous (project id, workspace id), for "last workspace".
#[derive(Debug, Default, Clone)]
pub(crate) struct NavHistory {
    pub(crate) cur: Option<(String, WorkspaceId)>,
    pub(crate) last: Option<(String, WorkspaceId)>,
}

/// Record the current location; when it differs from the last seen one, the old one becomes
/// "last". Called after every app event, so every kind of workspace/project switch is caught.
pub(crate) fn track_location(state: &mut AppState) {
    let Some(pid) = state.active_project_id.clone() else {
        return;
    };
    let loc = (pid, state.layout_mgr.active_workspace_id());
    match &state.nav.cur {
        Some(c) if *c == loc => {}
        Some(c) => {
            state.nav.last = Some(c.clone());
            state.nav.cur = Some(loc);
        }
        None => state.nav.cur = Some(loc),
    }
}

/// Layout of a project: the live one for the active project, else its parked layout.
fn layout_of<'a>(state: &'a AppState, project_id: &str) -> Option<&'a LayoutManager> {
    if state.active_project_id.as_deref() == Some(project_id) {
        Some(&state.layout_mgr)
    } else {
        state.parked.get(project_id).map(|p| &p.layout)
    }
}

fn workspace_panes(state: &AppState, ws: &tag_layout::Workspace) -> Vec<SwitcherPane> {
    let mut ids = ws.all_panes();
    ids.extend(ws.minimized_panes.iter().copied());
    ids.into_iter()
        .filter_map(|id| {
            let p = state.panes.get(&id)?;
            Some(SwitcherPane {
                id,
                title: p.title(),
                status: p.agent_status().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Projects for the switcher: registry order (pinned, recent first), then parked-only ids.
pub(crate) fn build_switcher_projects(state: &AppState) -> Vec<SwitcherProject> {
    let active = state.active_project_id.as_deref();
    let mut entries: Vec<(String, String, String)> = state
        .projects
        .list_recent()
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.root.display().to_string()))
        .collect();
    let mut extra: Vec<String> = state
        .parked
        .keys()
        .filter(|k| !entries.iter().any(|e| &e.0 == *k))
        .cloned()
        .collect();
    extra.sort();
    for id in extra {
        entries.push((id.clone(), id, String::new()));
    }
    if let Some(a) = active {
        if !entries.iter().any(|e| e.0 == a) {
            entries.insert(0, (a.to_string(), state.project_name(a), String::new()));
        }
    }
    entries
        .into_iter()
        .map(|(id, name, root)| {
            let workspaces = layout_of(state, &id)
                .map(|l| {
                    l.workspaces()
                        .iter()
                        .map(|w| SwitcherWorkspace {
                            id: w.id,
                            name: w.name.clone(),
                            panes: workspace_panes(state, w),
                        })
                        .collect()
                })
                .unwrap_or_default();
            SwitcherProject {
                active: Some(id.as_str()) == active,
                id,
                name,
                root,
                workspaces,
            }
        })
        .collect()
}

pub(crate) fn action_toggle_project_switcher(state: &mut AppState) {
    if state.project_switcher.is_open {
        state.project_switcher.is_open = false;
    } else {
        state.ensure_active_project();
        state.project_switcher.open();
        let active = state.active_project_id.clone();
        state.project_switcher.expanded.extend(active);
    }
    state.needs_clear = true;
}

/// Draw the overlay: tree plus live preview of the highlighted pane, and the folder prompt.
pub(crate) fn render_switcher_overlay(frame: &mut Frame, size: Rect, state: &mut AppState) {
    let projects = build_switcher_projects(state);
    let rows = switcher_rows(&projects, &state.project_switcher);
    state.project_switcher.clamp(rows.len());
    let pane = rows
        .get(state.project_switcher.selected)
        .and_then(|r| match &r.target {
            SwitcherTarget::Pane(_, _, pid) => Some(*pid),
            _ => None,
        });
    let area = preview_area(size);
    let mut title = String::new();
    let mut preview: Vec<String> = Vec::new();
    if let Some(p) = pane.and_then(|pid| state.panes.get_mut(&pid)) {
        title = p.title();
        let n = area.height.saturating_sub(1) as usize;
        if n > 0 {
            if let Some(text) = p.read_text(n) {
                let lines: Vec<&str> = text.lines().collect();
                let start = lines.len().saturating_sub(n);
                preview = lines[start..].iter().map(|l| l.to_string()).collect();
            }
        }
    }
    render_project_switcher(
        frame,
        size,
        &projects,
        &mut state.project_switcher,
        &preview,
        &title,
        &state.theme,
    );
    if let Some(ref path) = state.project_switcher.folder_prompt {
        tag_ui::render_prompt_modal(
            frame,
            size,
            " Open Folder ",
            " Path to open as a project:",
            path,
            72,
            &state.theme,
        );
    }
}

/// Make `project_id` active (registering nothing new). True when it is shown afterwards.
fn show_project(state: &mut AppState, project_id: &str) -> bool {
    if state.active_project_id.as_deref() == Some(project_id) {
        return true;
    }
    state.switch_project(project_id)
}

fn go_to(state: &mut AppState, target: &SwitcherTarget) {
    match target {
        SwitcherTarget::Project(id) => {
            show_project(state, id);
        }
        SwitcherTarget::Workspace(id, ws) => {
            if show_project(state, id) {
                state.layout_mgr.switch_workspace(*ws);
            }
        }
        SwitcherTarget::Pane(id, ws, pid) => {
            if show_project(state, id) {
                state.layout_mgr.switch_workspace(*ws);
                focus_pane_anywhere(state, *pid);
            }
        }
    }
    state.project_switcher.is_open = false;
    state.needs_clear = true;
    save_session_to_disk(state);
}

/// Expand a leading `~/` to the home directory.
fn expand_path(input: &str) -> PathBuf {
    let t = input.trim();
    if t == "~" {
        return dirs_or_home();
    }
    match t.strip_prefix("~/") {
        Some(rest) => dirs_or_home().join(rest),
        None => PathBuf::from(t),
    }
}

/// Open `input` (a folder) as a project: canonicalize, resolve the repo root, register it,
/// switch to it and start a shell there when it has no panes.
pub(crate) fn open_folder_as_project(state: &mut AppState, input: &str) -> bool {
    let Ok(canon) = expand_path(input).canonicalize() else {
        push_toast(
            state,
            "Projects",
            format!("Not a folder: {}", input.trim()),
            ToastLevel::Warning,
            3,
        );
        return false;
    };
    if !canon.is_dir() {
        push_toast(
            state,
            "Projects",
            format!("Not a folder: {}", canon.display()),
            ToastLevel::Warning,
            3,
        );
        return false;
    }
    state.ensure_active_project();
    let root = tag_projects::find_repo_root(&canon).unwrap_or_else(|| canon.clone());
    let project = state.projects.touch(&root).clone();
    if state.projects_load_error.is_none() {
        let _ = state.projects.save();
    }
    state.switch_project(&project.id);
    if state.layout_mgr.active_workspace().all_panes().is_empty() {
        open_shell_in(state, project.name.clone(), canon);
    }
    state.project_switcher.is_open = false;
    state.needs_clear = true;
    save_session_to_disk(state);
    true
}

/// Key handling while the switcher is open (caller checks `is_open`).
pub(crate) fn handle_switcher_key(state: &mut AppState, key: &KeyEvent) {
    if let Some(mut text) = state.project_switcher.folder_prompt.take() {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                if !text.trim().is_empty() && !open_folder_as_project(state, &text) {
                    state.project_switcher.folder_prompt = Some(text);
                }
            }
            KeyCode::Backspace => {
                text.pop();
                state.project_switcher.folder_prompt = Some(text);
            }
            _ => {
                if let Some(c) = text_input_char(key) {
                    text.push(c);
                }
                state.project_switcher.folder_prompt = Some(text);
            }
        }
        return;
    }
    let projects = build_switcher_projects(state);
    let rows = switcher_rows(&projects, &state.project_switcher);
    state.project_switcher.clamp(rows.len());
    let sel = rows.get(state.project_switcher.selected).cloned();
    let count = rows.len();
    match key.code {
        KeyCode::Esc => {
            state.project_switcher.is_open = false;
            state.needs_clear = true;
        }
        KeyCode::Up => state.project_switcher.move_by(-1, count),
        KeyCode::Down => state.project_switcher.move_by(1, count),
        KeyCode::Right | KeyCode::Tab => {
            if let Some(r) = sel {
                if let SwitcherTarget::Project(id) = r.target {
                    if r.expandable && !state.project_switcher.expanded.remove(&id) {
                        state.project_switcher.expanded.insert(id);
                    }
                }
            }
        }
        KeyCode::Left => {
            if let Some(r) = sel {
                match r.target {
                    SwitcherTarget::Project(id) => {
                        state.project_switcher.expanded.remove(&id);
                    }
                    SwitcherTarget::Workspace(id, _) | SwitcherTarget::Pane(id, _, _) => {
                        state.project_switcher.expanded.remove(&id);
                        let idx = rows
                            .iter()
                            .position(|x| x.target == SwitcherTarget::Project(id.clone()))
                            .unwrap_or(0);
                        state.project_switcher.selected = idx;
                    }
                }
            }
        }
        KeyCode::Enter => {
            if let Some(r) = sel {
                go_to(state, &r.target);
            }
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.project_switcher.folder_prompt = Some(String::new());
        }
        KeyCode::Backspace => {
            state.project_switcher.query.pop();
            state.project_switcher.selected = 0;
        }
        _ => {
            if let Some(c) = text_input_char(key) {
                state.project_switcher.query.push(c);
                state.project_switcher.selected = 0;
            }
        }
    }
}

// ---- navigation ----

/// Jump back to the previous (project, workspace), switching projects when needed.
pub(crate) fn action_last_workspace(state: &mut AppState) {
    track_location(state);
    let Some((proj, ws)) = state.nav.last.clone() else {
        push_toast(
            state,
            "Navigation",
            "No previous workspace",
            ToastLevel::Info,
            2,
        );
        return;
    };
    let exists = layout_of(state, &proj).is_some_and(|l| l.workspaces().iter().any(|w| w.id == ws));
    if !exists || !show_project(state, &proj) {
        state.nav.last = None;
        push_toast(
            state,
            "Navigation",
            "Previous workspace is gone",
            ToastLevel::Info,
            2,
        );
        return;
    }
    state.layout_mgr.switch_workspace(ws);
    track_location(state);
    state.needs_clear = true;
    save_session_to_disk(state);
}

fn is_awaiting(status: Option<&str>) -> bool {
    matches!(status, Some("awaiting-input") | Some("awaiting_input"))
}

/// (project id, workspace id) holding `pid` in an active or parked layout.
fn locate_pane(state: &AppState, pid: PaneId) -> Option<(String, WorkspaceId)> {
    if let (Some(active), Some(ws)) = (
        state.active_project_id.clone(),
        state.layout_mgr.find_pane_workspace(pid),
    ) {
        return Some((active, ws));
    }
    let mut ids: Vec<&String> = state.parked.keys().collect();
    ids.sort();
    ids.into_iter().find_map(|id| {
        state.parked[id]
            .layout
            .find_pane_workspace(pid)
            .map(|ws| (id.clone(), ws))
    })
}

/// Project owning a hidden pane (via its run's repo), else the active one.
fn owner_project(state: &AppState, pid: PaneId) -> Option<String> {
    let run = state.pane_meta.get(&pid)?.run.as_ref()?;
    let repo = &state.runs.get(run)?.repo;
    state
        .projects
        .projects
        .iter()
        .find(|p| repo.starts_with(&p.root))
        .map(|p| p.id.clone())
}

/// Jump to the next agent awaiting input (stable id order, after the focused pane).
pub(crate) fn action_next_attention(state: &mut AppState) {
    let mut ids: Vec<PaneId> = state
        .panes
        .iter()
        .filter(|(_, p)| is_awaiting(p.agent_status()))
        .map(|(id, _)| *id)
        .collect();
    ids.sort_by_key(|p| p.0);
    if ids.is_empty() {
        push_toast(
            state,
            "Navigation",
            "No agents need input",
            ToastLevel::Info,
            3,
        );
        return;
    }
    let focused = state.layout_mgr.active_workspace().focused_pane;
    let target = focused
        .and_then(|f| ids.iter().copied().find(|p| p.0 > f.0))
        .unwrap_or(ids[0]);
    track_location(state);
    state.peek = None;
    state.workers.is_open = false;
    match locate_pane(state, target) {
        Some((proj, ws)) => {
            if show_project(state, &proj) {
                state.layout_mgr.switch_workspace(ws);
                focus_pane_anywhere(state, target);
            }
        }
        None => {
            if let Some(owner) = owner_project(state, target) {
                show_project(state, &owner);
            }
            place_pane(state, target, Visibility::Pane, None);
            focus_pane_anywhere(state, target);
        }
    }
    track_location(state);
    state.needs_clear = true;
    save_session_to_disk(state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::tests::{test_state, MockAgentPane};
    use crossterm::event::KeyEventKind;
    use ratatui::{backend::TestBackend, Terminal};
    use std::sync::{Arc, Mutex};
    use tag_core::pane::{Pane, PaneKind};
    use tag_core::theme::Theme;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(c, KeyModifiers::NONE, KeyEventKind::Press)
    }

    struct TextPane(PaneId, String);
    impl Pane for TextPane {
        fn id(&self) -> PaneId {
            self.0
        }
        fn title(&self) -> String {
            "txt".into()
        }
        fn kind(&self) -> PaneKind {
            PaneKind::Shell
        }
        fn render(&mut self, _: &mut Frame, _: Rect, _: bool, _: &Theme) {}
        fn handle_event(&mut self, _: &tag_core::event::AppEvent) -> tag_core::event::EventResult {
            tag_core::event::EventResult::Consumed
        }
        fn on_resize(&mut self, _: u16, _: u16) {}
        fn is_active(&self) -> bool {
            true
        }
        fn read_text(&mut self, _: usize) -> Option<String> {
            Some(self.1.clone())
        }
    }

    /// Two projects "a" (active) and "b" (parked), each with one awaiting-input agent.
    fn two_projects() -> AppState {
        let (mut state, rx) = test_state();
        // keep the receiver alive for the whole test
        std::mem::forget(rx);
        let dir = std::env::temp_dir().join(format!("tag-nav-{}", std::process::id()));
        let (da, db) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&da).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let a = state.projects.touch(&da).clone();
        let b = state.projects.touch(&db).clone();
        state.projects.set_active(&a.id);
        state.active_project_id = Some(a.id.clone());
        state.active_project_name = Some(a.name.clone());
        for (pid, status) in [(1u32, "awaiting-input"), (2, "running")] {
            state.panes.insert(
                PaneId(pid),
                Box::new(MockAgentPane {
                    id: PaneId(pid),
                    status,
                }),
            );
        }
        state
            .layout_mgr
            .active_workspace_mut()
            .add_initial_pane(PaneId(1));
        let _ = state
            .layout_mgr
            .active_workspace_mut()
            .split_current(PaneId(2), tag_core::action::SplitDirection::Vertical);
        // project b: parked layout with pane 3 awaiting input
        state.panes.insert(
            PaneId(3),
            Box::new(MockAgentPane {
                id: PaneId(3),
                status: "awaiting-input",
            }),
        );
        let mut lm = crate::app::sessions::fresh_layout("b");
        lm.active_workspace_mut().add_initial_pane(PaneId(3));
        state
            .parked
            .insert(b.id.clone(), ParkedProject { layout: lm });
        state
    }

    fn ids(state: &AppState) -> (String, String) {
        let mut v: Vec<_> = state
            .projects
            .projects
            .iter()
            .map(|p| p.id.clone())
            .collect();
        v.sort_by_key(|i| state.project_name(i));
        (v[0].clone(), v[1].clone())
    }

    #[test]
    fn switcher_filters_expands_and_switches_without_touching_panes() {
        let mut state = two_projects();
        let (a, b) = ids(&state);
        let panes_before: Vec<PaneId> = {
            let mut v: Vec<_> = state.panes.keys().copied().collect();
            v.sort_by_key(|p| p.0);
            v
        };
        action_toggle_project_switcher(&mut state);
        assert!(state.project_switcher.is_open);
        assert!(is_any_overlay_open(&state));
        // fuzzy filter to project "b"
        handle_switcher_key(&mut state, &key(KeyCode::Char('b')));
        let projects = build_switcher_projects(&state);
        let rows = switcher_rows(&projects, &state.project_switcher);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, SwitcherTarget::Project(b.clone()));
        // expand shows workspace + pane rows
        handle_switcher_key(&mut state, &key(KeyCode::Right));
        let projects = build_switcher_projects(&state);
        assert_eq!(switcher_rows(&projects, &state.project_switcher).len(), 3);
        handle_switcher_key(&mut state, &key(KeyCode::Left));
        // Enter switches project
        handle_switcher_key(&mut state, &key(KeyCode::Enter));
        assert_eq!(state.active_project_id.as_deref(), Some(b.as_str()));
        assert!(!state.project_switcher.is_open);
        assert!(state.parked.contains_key(&a));
        let mut after: Vec<_> = state.panes.keys().copied().collect();
        after.sort_by_key(|p| p.0);
        assert_eq!(panes_before, after);
    }

    #[test]
    fn enter_on_pane_focuses_it_in_its_project() {
        let mut state = two_projects();
        let (_, b) = ids(&state);
        action_toggle_project_switcher(&mut state);
        state.project_switcher.expanded.insert(b.clone());
        let projects = build_switcher_projects(&state);
        let rows = switcher_rows(&projects, &state.project_switcher);
        let idx = rows
            .iter()
            .position(|r| matches!(r.target, SwitcherTarget::Pane(_, _, PaneId(3))))
            .unwrap();
        state.project_switcher.selected = idx;
        handle_switcher_key(&mut state, &key(KeyCode::Enter));
        assert_eq!(state.active_project_id.as_deref(), Some(b.as_str()));
        assert_eq!(
            state.layout_mgr.active_workspace().focused_pane,
            Some(PaneId(3))
        );
    }

    #[test]
    fn renders_switcher_with_preview() {
        let mut state = two_projects();
        state.panes.insert(
            PaneId(9),
            Box::new(TextPane(PaneId(9), "line one\nPREVIEW-TEXT".into())),
        );
        state
            .layout_mgr
            .active_workspace_mut()
            .minimized_panes
            .push(PaneId(9));
        action_toggle_project_switcher(&mut state);
        let projects = build_switcher_projects(&state);
        let rows = switcher_rows(&projects, &state.project_switcher);
        state.project_switcher.selected = rows
            .iter()
            .position(|r| matches!(r.target, SwitcherTarget::Pane(_, _, PaneId(9))))
            .unwrap();
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| {
            let size = f.area();
            render_switcher_overlay(f, size, &mut state);
        })
        .unwrap();
        let buf = t.backend().buffer();
        let all: String = (0..30)
            .flat_map(|y| (0..120).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(all.contains("PREVIEW-TEXT"));
        assert!(all.contains("txt"));
    }

    #[test]
    fn last_workspace_toggles_across_projects() {
        let mut state = two_projects();
        let (a, b) = ids(&state);
        track_location(&mut state);
        assert!(state.switch_project(&b));
        track_location(&mut state);
        action_last_workspace(&mut state);
        assert_eq!(state.active_project_id.as_deref(), Some(a.as_str()));
        action_last_workspace(&mut state);
        assert_eq!(state.active_project_id.as_deref(), Some(b.as_str()));
        // within-project workspace switching is tracked too
        state.layout_mgr.add_workspace("two");
        track_location(&mut state);
        action_last_workspace(&mut state);
        assert_eq!(state.layout_mgr.active_idx(), 0);
        action_last_workspace(&mut state);
        assert_eq!(state.layout_mgr.active_idx(), 1);
    }

    #[test]
    fn next_attention_cycles_across_projects() {
        let mut state = two_projects();
        let (a, b) = ids(&state);
        state.layout_mgr.active_workspace_mut().focused_pane = Some(PaneId(2));
        // after pane 2 -> pane 3 in parked project b
        action_next_attention(&mut state);
        assert_eq!(state.active_project_id.as_deref(), Some(b.as_str()));
        assert_eq!(
            state.layout_mgr.active_workspace().focused_pane,
            Some(PaneId(3))
        );
        // wraps to pane 1 in project a
        action_next_attention(&mut state);
        assert_eq!(state.active_project_id.as_deref(), Some(a.as_str()));
        assert_eq!(
            state.layout_mgr.active_workspace().focused_pane,
            Some(PaneId(1))
        );
    }

    #[test]
    fn next_attention_toasts_when_none() {
        let (mut state, _rx) = test_state();
        action_next_attention(&mut state);
        assert!(state
            .toasts
            .iter()
            .any(|t| t.message.contains("No agents need input")));
        let _ = Arc::new(Mutex::new(0));
    }

    #[test]
    fn open_folder_registers_and_switches() {
        let (mut state, _rx) = test_state();
        state.ensure_active_project();
        let first = state.active_project_id.clone();
        let dir = std::env::temp_dir().join(format!("tag-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!open_folder_as_project(&mut state, "/definitely/not/here"));
        assert!(open_folder_as_project(&mut state, dir.to_str().unwrap()));
        assert_ne!(state.active_project_id, first);
        for p in state.panes.values_mut() {
            p.kill();
        }
    }
}
