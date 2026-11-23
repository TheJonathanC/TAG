//! Per-project sessions: project switching, session discovery and legacy migration.
//!
//! The active project's workspaces live in `AppState::layout_mgr`; every other project's
//! layouts are parked in `AppState::parked`. Panes of all projects stay in `AppState::panes`
//! so their processes keep running while another project is shown.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use std::path::{Path, PathBuf};
use tag_core::pane::WorkspaceId;
use tag_layout::{LayoutManager, SavedSession, Workspace};

/// A project that is not currently shown: its layouts, kept out of `layout_mgr`.
pub(crate) struct ParkedProject {
    pub(crate) layout: LayoutManager,
}

/// `<state_dir>/projects/<project_id>/session.json`
pub(crate) fn project_session_path(state_dir: &Path, project_id: &str) -> PathBuf {
    state_dir
        .join("projects")
        .join(project_id)
        .join("session.json")
}

/// `<state_dir>/sessions.json`: remembers which project was active.
pub(crate) fn sessions_index_path(state_dir: &Path) -> PathBuf {
    state_dir.join("sessions.json")
}

/// Marker written once the legacy global session has been imported.
pub(crate) fn migration_marker_path(state_dir: &Path) -> PathBuf {
    state_dir.join("migrated-v1")
}

pub(crate) fn read_active_project_index(state_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(sessions_index_path(state_dir)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("active_project")?.as_str().map(str::to_string)
}

pub(crate) fn write_active_project_index(state_dir: &Path, project_id: &str) {
    let path = sessions_index_path(state_dir);
    let _ = std::fs::create_dir_all(state_dir);
    let json = serde_json::json!({ "version": 1, "active_project": project_id });
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Every parsable `<state_dir>/projects/*/session.json`, sorted by project id.
pub(crate) fn read_project_sessions(state_dir: &Path) -> Vec<(String, SavedSession)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(state_dir.join("projects")) else {
        return out;
    };
    for entry in rd.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path().join("session.json");
        if path.is_file() {
            if let Ok(saved) = SavedSession::load_from_file(&path) {
                out.push((id, saved));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// True when any per-project session file exists (even an unparsable one).
pub(crate) fn any_project_session_exists(state_dir: &Path) -> bool {
    std::fs::read_dir(state_dir.join("projects"))
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().join("session.json").is_file())
        })
        .unwrap_or(false)
}

#[allow(dead_code)] // used by switch_project; UI wiring comes with T3.4/T3.5
/// A layout with one empty workspace named after the project.
pub(crate) fn fresh_layout(name: &str) -> LayoutManager {
    LayoutManager::from_workspaces(vec![Workspace::new(WorkspaceId(1), name)], WorkspaceId(1))
}

impl AppState {
    #[allow(dead_code)]
    /// Display name for a project id (registry name, else the id).
    pub(crate) fn project_name(&self, project_id: &str) -> String {
        self.projects
            .get(project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| project_id.to_string())
    }

    fn persist_projects(&self) {
        if self.projects_load_error.is_none() {
            let _ = self.projects.save();
        }
    }

    /// Make sure there is an active project: the repo of the process cwd, else `default`.
    pub(crate) fn ensure_active_project(&mut self) -> String {
        if let Some(id) = &self.active_project_id {
            return id.clone();
        }
        let project = match std::env::current_dir()
            .ok()
            .as_deref()
            .and_then(tag_projects::find_repo_root)
        {
            Some(root) => self.projects.touch(&root).clone(),
            None => self.projects.default_project(&dirs_or_home()),
        };
        self.projects.set_active(&project.id);
        self.persist_projects();
        self.active_project_name = Some(project.name.clone());
        self.active_project_root = Some(project.root.clone());
        self.active_project_id = Some(project.id.clone());
        project.id
    }

    #[allow(dead_code)] // UI wiring comes with T3.4/T3.5
    /// Show `project_id`: park the current layouts under the current project id and bring in
    /// the target's parked layouts (or a fresh single empty workspace named after the project).
    /// Panes are untouched, so processes keep running. Returns false when the id is unknown
    /// (neither parked nor in the registry) or already active.
    pub(crate) fn switch_project(&mut self, project_id: &str) -> bool {
        let current = self.ensure_active_project();
        if current == project_id {
            return false;
        }
        let known = self.parked.contains_key(project_id) || self.projects.get(project_id).is_some();
        if !known {
            return false;
        }
        // Persist the outgoing project's latest layout before it is parked.
        save_session_to_disk(self);
        let outgoing = std::mem::take(&mut self.layout_mgr);
        self.parked
            .insert(current, ParkedProject { layout: outgoing });
        self.layout_mgr = match self.parked.remove(project_id) {
            Some(p) => p.layout,
            None => fresh_layout(&self.project_name(project_id)),
        };
        self.active_project_id = Some(project_id.to_string());
        self.projects.set_active(project_id);
        if let Some(p) = self.projects.get(project_id).cloned() {
            self.active_project_name = Some(p.name);
            self.active_project_root = Some(p.root);
        }
        self.persist_projects();
        write_active_project_index(&self.paths.state_dir, project_id);
        self.needs_clear = true;
        self.is_dirty = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::tests::{test_state, MockAgentPane};
    use tag_core::pane::{PaneId, PaneKind};
    use tag_layout::{SavedPane, SavedWorkspace};

    fn sleep_pane(id: u32) -> SavedPane {
        SavedPane {
            id: PaneId(id),
            title: format!("p{id}"),
            kind: PaneKind::Shell,
            cwd: Some(std::env::temp_dir()),
            command: Some("sleep".into()),
            args: vec!["30".into()],
            visibility: None,
            profile: None,
            resume: None,
        }
    }

    /// One workspace whose layout is a vertical chain of the given panes.
    fn session_with(ids: &[u32]) -> SavedSession {
        let mut ws = Workspace::new(WorkspaceId(1), "main");
        ws.add_initial_pane(PaneId(ids[0]));
        for &id in &ids[1..] {
            let _ = ws.split_current(PaneId(id), tag_core::action::SplitDirection::Vertical);
        }
        let mut s = SavedSession::new(WorkspaceId(1));
        s.workspaces.push(SavedWorkspace::from(&ws));
        for &id in ids {
            s.panes.insert(PaneId(id), sleep_pane(id));
        }
        s
    }

    fn kill_all(state: &mut AppState) {
        for pane in state.panes.values_mut() {
            pane.kill();
        }
    }

    fn layout_panes(layout: &LayoutManager) -> Vec<PaneId> {
        let mut v: Vec<PaneId> = layout
            .workspaces()
            .iter()
            .flat_map(|w| w.all_panes())
            .collect();
        v.sort_by_key(|p| p.0);
        v
    }

    #[test]
    fn switch_and_back_keeps_panes_and_swaps_workspaces() {
        let (mut state, _rx) = test_state();
        let dir = state.paths.state_dir.parent().unwrap().to_path_buf();
        let root_a = dir.join("proj-a");
        let root_b = dir.join("proj-b");
        let a = state.projects.touch(&root_a).clone();
        let b = state.projects.touch(&root_b).clone();
        state.active_project_id = Some(a.id.clone());

        for id in [1u32, 2] {
            state.panes.insert(
                PaneId(id),
                Box::new(MockAgentPane {
                    id: PaneId(id),
                    status: "running",
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

        assert!(state.switch_project(&b.id));
        assert_eq!(state.active_project_id.as_deref(), Some(b.id.as_str()));
        assert_eq!(state.layout_mgr.workspaces().len(), 1);
        assert_eq!(state.layout_mgr.active_workspace().name, "proj-b");
        assert!(layout_panes(&state.layout_mgr).is_empty());
        assert!(state.parked.contains_key(&a.id));
        // Processes (panes) keep living while parked.
        assert!(state.panes.contains_key(&PaneId(1)) && state.panes.contains_key(&PaneId(2)));
        assert!(state.needs_clear && state.is_dirty);
        assert_eq!(state.projects.last_active.as_deref(), Some(b.id.as_str()));

        assert!(state.switch_project(&a.id));
        assert_eq!(layout_panes(&state.layout_mgr), vec![PaneId(1), PaneId(2)]);
        assert!(state.panes.contains_key(&PaneId(1)) && state.panes.contains_key(&PaneId(2)));
        assert!(state.parked.contains_key(&b.id));
        assert!(!state.switch_project(&a.id), "already active");
        assert!(!state.switch_project("nope"), "unknown project");
    }

    #[test]
    fn save_writes_one_file_per_project_with_only_its_panes() {
        let (mut state, _rx) = test_state();
        let dir = state.paths.state_dir.parent().unwrap().to_path_buf();
        let a = state.projects.touch(&dir.join("proj-a")).clone();
        let b = state.projects.touch(&dir.join("proj-b")).clone();
        state.active_project_id = Some(a.id.clone());
        for id in [1u32, 2] {
            state.panes.insert(
                PaneId(id),
                Box::new(MockAgentPane {
                    id: PaneId(id),
                    status: "running",
                }),
            );
        }
        state
            .layout_mgr
            .active_workspace_mut()
            .add_initial_pane(PaneId(1));
        assert!(state.switch_project(&b.id));
        state
            .layout_mgr
            .active_workspace_mut()
            .add_initial_pane(PaneId(2));
        save_session_to_disk(&state);

        let sd = &state.paths.state_dir;
        let sa = SavedSession::load_from_file(&project_session_path(sd, &a.id)).unwrap();
        let sb = SavedSession::load_from_file(&project_session_path(sd, &b.id)).unwrap();
        assert_eq!(sa.version, tag_layout::SESSION_VERSION);
        assert_eq!(
            sa.panes.keys().copied().collect::<Vec<_>>(),
            vec![PaneId(1)]
        );
        assert_eq!(
            sb.panes.keys().copied().collect::<Vec<_>>(),
            vec![PaneId(2)]
        );
        assert_eq!(
            read_active_project_index(sd).as_deref(),
            Some(b.id.as_str())
        );
    }

    #[test]
    fn migrates_v1_session_once_into_default_project() {
        let (mut state, _rx) = test_state();
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../layout/tests/fixtures/session_v1.json"
        );
        // test_state paths are isolated: session_path lives in the temp state dir.
        assert!(state.paths.session_path.starts_with(&state.paths.state_dir));
        std::fs::create_dir_all(&state.paths.state_dir).unwrap();
        std::fs::copy(fixture, &state.paths.session_path).unwrap();

        restore_session(&mut state, 100, 40);
        let default_id = state.projects.get("default").unwrap().id.clone();
        assert_eq!(
            state.active_project_id.as_deref(),
            Some(default_id.as_str())
        );
        assert_eq!(layout_panes(&state.layout_mgr), vec![PaneId(1), PaneId(2)]);
        assert_eq!(state.panes.len(), 2);
        let sd = state.paths.state_dir.clone();
        assert!(migration_marker_path(&sd).exists());
        assert!(state.paths.session_path.exists(), "legacy file is kept");
        let migrated =
            SavedSession::load_from_file(&project_session_path(&sd, &default_id)).unwrap();
        assert_eq!(migrated.version, tag_layout::SESSION_VERSION);
        assert_eq!(migrated.panes.len(), 2);

        // A second instance does not import the legacy file again.
        std::fs::remove_file(project_session_path(&sd, &default_id)).unwrap();
        kill_all(&mut state);
        let (tx, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let (_utx, urx) = std::sync::mpsc::channel();
        let mut second =
            AppState::new(state.config.clone(), None, state.paths.clone(), tx, urx).unwrap();
        restore_session(&mut second, 100, 40);
        assert!(second.panes.is_empty());
    }

    #[test]
    fn restore_remaps_colliding_pane_ids_across_projects() {
        let (mut state, _rx) = test_state();
        let sd = state.paths.state_dir.clone();
        session_with(&[1, 2])
            .save_to_file(&project_session_path(&sd, "aaaa"))
            .unwrap();
        session_with(&[1, 2])
            .save_to_file(&project_session_path(&sd, "bbbb"))
            .unwrap();
        write_active_project_index(&sd, "bbbb");

        restore_session(&mut state, 100, 40);
        assert_eq!(state.panes.len(), 4);
        assert_eq!(state.active_project_id.as_deref(), Some("bbbb"));
        let active = layout_panes(&state.layout_mgr);
        let parked = layout_panes(&state.parked["aaaa"].layout);
        assert_eq!(active.len(), 2);
        assert_eq!(parked.len(), 2);
        // aaaa restores first and keeps its ids; bbbb gets fresh ones.
        assert_eq!(parked, vec![PaneId(1), PaneId(2)]);
        assert!(active.iter().all(|p| !parked.contains(p)));
        assert!(active
            .iter()
            .chain(&parked)
            .all(|p| state.panes.contains_key(p)));
        assert!(state.next_pane_id > active.iter().map(|p| p.0).max().unwrap());
        kill_all(&mut state);
    }
}
