use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tag_core::pane::{PaneId, PaneKind, WorkspaceId};

use crate::tree::LayoutNode;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPane {
    pub id: PaneId,
    pub title: String,
    pub kind: PaneKind,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub root: Option<LayoutNode>,
    pub focused_pane: Option<PaneId>,
    pub zoomed_pane: Option<PaneId>,
    pub worktree_binding: Option<PathBuf>,
    #[serde(default)]
    pub minimized_panes: Vec<PaneId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub active_workspace_id: WorkspaceId,
    pub workspaces: Vec<SavedWorkspace>,
    pub panes: HashMap<PaneId, SavedPane>,
}

impl SavedSession {
    pub fn new(active_workspace_id: WorkspaceId) -> Self {
        Self {
            active_workspace_id,
            workspaces: Vec::new(),
            panes: HashMap::new(),
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), anyhow::Error> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        let tmp_path = path.with_extension("json.tmp");
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            file.write_all(data.as_bytes())?;
            file.sync_all()?;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
        }
        #[cfg(not(unix))]
        {
            fs::write(&tmp_path, data)?;
        }
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    pub fn load_from_file(path: &Path) -> Result<Self, anyhow::Error> {
        let content = fs::read_to_string(path)?;
        let session: Self = serde_json::from_str(&content)?;
        Ok(session)
    }
}

impl From<&Workspace> for SavedWorkspace {
    fn from(ws: &Workspace) -> Self {
        Self {
            id: ws.id,
            name: ws.name.clone(),
            root: ws.root.clone(),
            focused_pane: ws.focused_pane,
            zoomed_pane: ws.zoomed_pane,
            worktree_binding: ws.worktree_binding.clone(),
            minimized_panes: ws.minimized_panes.clone(),
        }
    }
}

impl From<SavedWorkspace> for Workspace {
    fn from(saved: SavedWorkspace) -> Self {
        Self {
            id: saved.id,
            name: saved.name,
            root: saved.root,
            focused_pane: saved.focused_pane,
            zoomed_pane: saved.zoomed_pane,
            worktree_binding: saved.worktree_binding,
            undo_stack: Vec::new(),
            minimized_panes: saved.minimized_panes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_roundtrip() {
        let mut session = SavedSession::new(WorkspaceId(1));
        let ws = Workspace::new(WorkspaceId(1), "Main");
        session.workspaces.push(SavedWorkspace::from(&ws));
        session.panes.insert(
            PaneId(1),
            SavedPane {
                id: PaneId(1),
                title: "Shell".into(),
                kind: PaneKind::Shell,
                cwd: Some(PathBuf::from("/tmp")),
                command: Some("htop".into()),
                args: vec!["-d".into(), "10".into()],
            },
        );

        let temp_dir = std::env::temp_dir();
        let session_file = temp_dir.join("tag_test_session.json");
        session.save_to_file(&session_file).unwrap();

        let loaded = SavedSession::load_from_file(&session_file).unwrap();
        assert_eq!(loaded.active_workspace_id, WorkspaceId(1));
        assert_eq!(loaded.workspaces.len(), 1);
        assert_eq!(loaded.panes.len(), 1);
        let pane = loaded.panes.get(&PaneId(1)).unwrap();
        assert_eq!(pane.command.as_deref(), Some("htop"));
        assert_eq!(pane.args, vec!["-d", "10"]);

        let _ = std::fs::remove_file(session_file);
    }
}
