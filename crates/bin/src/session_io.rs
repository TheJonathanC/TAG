#[allow(unused_imports)]
use crate::{app::*, cli::*, sockets::*, tmux::*};
use std::path::PathBuf;
use tag_core::config::Config;
use tag_core::pane::PaneKind;
use tag_layout::{SavedPane, SavedSession, SavedWorkspace};

pub(crate) fn config_path() -> PathBuf {
    dirs_or_home()
        .join(".config")
        .join("tag")
        .join("config.toml")
}

/// Load the config file. On a parse error the defaults are used and the error is
/// returned alongside so it can be shown (and the broken file is never overwritten).
pub(crate) fn load_app_config() -> (Config, Option<String>) {
    let path = config_path();
    match std::fs::read_to_string(&path) {
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

pub(crate) fn save_app_config(config: &Config) -> Result<(), anyhow::Error> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, config.to_toml_string()?)?;
    Ok(())
}

pub(crate) fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub(crate) fn save_session_to_disk(state: &AppState) {
    let session_path = dirs_or_home()
        .join(".config")
        .join("tag")
        .join("session.json");
    let mut saved = SavedSession::new(state.layout_mgr.active_workspace_id());
    for ws in state.layout_mgr.workspaces() {
        let mut clean_ws = ws.clone();
        for pid in clean_ws.all_panes() {
            if !state.panes.contains_key(&pid) {
                clean_ws.remove_pane(pid);
            }
        }
        clean_ws
            .minimized_panes
            .retain(|p| state.panes.contains_key(p));
        saved.workspaces.push(SavedWorkspace::from(&clean_ws));
    }
    for (&pid, pane) in &state.panes {
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
        saved.panes.insert(
            pid,
            SavedPane {
                id: pid,
                title,
                kind,
                cwd: cwd.or_else(|| pane.cwd()),
                command,
                args,
            },
        );
    }
    let _ = saved.save_to_file(&session_path);
}
