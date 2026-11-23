#[allow(unused_imports)]
use crate::{app::*, cli::*, sockets::*, tmux::*};
use std::path::{Path, PathBuf};
use tag_core::config::Config;
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_layout::{LayoutManager, SavedPane, SavedSession, SavedWorkspace};

pub(crate) fn config_path() -> PathBuf {
    dirs_or_home()
        .join(".config")
        .join("tag")
        .join("config.toml")
}

/// Load the config file. On a parse error the defaults are used and the error is
/// returned alongside so it can be shown (and the broken file is never overwritten).
pub(crate) fn load_app_config(path: &Path) -> (Config, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(content) => match Config::load_from_str(&content) {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                Config::default(),
                Some(format!("{}: {}", path.display(), e)),
            ),
        },
        Err(_) => (Config::default(), None),
    }
}

pub(crate) fn save_app_config(path: &Path, config: &Config) -> Result<(), anyhow::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, config.to_toml_string()?)?;
    Ok(())
}

pub(crate) fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Snapshot of one pane for the session file.
fn saved_pane(state: &AppState, pid: PaneId, pane: &dyn Pane) -> SavedPane {
    let (_, _, cwd) = pane.running_process();
    let tmux_session = pane.tmux_session_name().or_else(|| {
        pane.title()
            .strip_prefix("tmux: ")
            .and_then(|s| s.split([' ', '(']).next())
            .filter(|s| !s.is_empty())
            .map(tag_pty::clean_tmux_session_name)
    });

    let (command, args, kind, title) = if let Some(name) = pane.applet_name() {
        (None, vec![name.to_string()], PaneKind::Applet, pane.title())
    } else if let Some(session) = tmux_session {
        (
            Some("tmux".to_string()),
            vec![
                "new-session".to_string(),
                "-A".to_string(),
                "-s".to_string(),
                session.clone(),
            ],
            if pane.kind() == PaneKind::Agent {
                PaneKind::Agent
            } else if pane.kind() == PaneKind::Donut {
                PaneKind::Donut
            } else {
                PaneKind::Tmux
            },
            format!("tmux: {}", session),
        )
    } else {
        let (cmd, args) = match pane.kind() {
            PaneKind::Shell => (None, Vec::new()),
            _ => {
                let (cmd, args, _) = pane.running_process();
                (cmd, args)
            }
        };
        (cmd, args, pane.kind(), pane.title())
    };
    let visibility = state
        .pane_meta
        .get(&pid)
        .and_then(|m| m.visibility)
        .map(|v| {
            match v {
                tag_orchestrator::Visibility::Hidden => "hidden",
                tag_orchestrator::Visibility::Card => "card",
                tag_orchestrator::Visibility::Pane => "pane",
            }
            .to_string()
        });
    SavedPane {
        id: pid,
        title,
        kind,
        cwd: cwd.or_else(|| pane.cwd()),
        command,
        args,
        visibility,
        profile: state.pane_meta.get(&pid).and_then(|m| m.profile.clone()),
        resume: state.pane_meta.get(&pid).and_then(|m| m.session_id.clone()),
    }
}

/// Build the session for one project from its layouts, keeping only the panes they reference.
pub(crate) fn build_project_session(state: &AppState, layout: &LayoutManager) -> SavedSession {
    let mut saved = SavedSession::new(layout.active_workspace_id());
    for ws in layout.workspaces() {
        let mut clean_ws = ws.clone();
        for pid in clean_ws.all_panes() {
            if !state.panes.contains_key(&pid) {
                clean_ws.remove_pane(pid);
            }
        }
        clean_ws
            .minimized_panes
            .retain(|p| state.panes.contains_key(p));
        for pid in clean_ws
            .all_panes()
            .into_iter()
            .chain(clean_ws.minimized_panes.iter().copied())
        {
            if let Some(pane) = state.panes.get(&pid) {
                saved
                    .panes
                    .insert(pid, saved_pane(state, pid, pane.as_ref()));
            }
        }
        saved.workspaces.push(SavedWorkspace::from(&clean_ws));
    }
    saved
}

/// Write one session file per project (the active one plus every parked one), and the
/// index naming the active project.
pub(crate) fn save_session_to_disk(state: &AppState) {
    let state_dir = &state.paths.state_dir;
    let active_id = state
        .active_project_id
        .clone()
        .unwrap_or_else(|| tag_projects::project_id(&dirs_or_home()));
    let saved = build_project_session(state, &state.layout_mgr);
    let _ = saved.save_to_file(&project_session_path(state_dir, &active_id));
    for (id, parked) in &state.parked {
        let saved = build_project_session(state, &parked.layout);
        let _ = saved.save_to_file(&project_session_path(state_dir, id));
    }
    write_active_project_index(state_dir, &active_id);
}
