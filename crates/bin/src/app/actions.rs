#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use std::path::PathBuf;
use tag_agents::{AgentPane, AgentProfile};
use tag_clipboard::{copy_to_clipboard, get_clipboard_text, ImageBridge, SnippetBank};
use tag_core::action::{Action, SplitDirection};
use tag_core::event::AppEvent;
use tag_core::pane::PaneKind;
use tag_core::theme::Theme;
use tag_keys::KeyStateMachine;
use tag_pty::PtyPane;
use tag_ui::{RenameModalState, Toast, ToastLevel};
use tag_vault::VaultMode;

pub(crate) fn execute_action(state: &mut AppState, action: Action) -> bool {
    match action {
        Action::Quit => {
            // The session is saved once when the main loop exits
            return true;
        }
        Action::Redraw => {
            state.needs_clear = true;
            state.is_dirty = true;
            return false;
        }
        Action::CheckUpdate => {
            state.toasts.push(Toast {
                title: "Checking for updates...".into(),
                message: "Querying GitHub Releases for new TAG versions...".into(),
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 3,
            });
            let tx = state.event_tx.clone();
            let current_ver = TAG_VERSION.to_string();
            std::thread::spawn(move || {
                if let Some(newer) = crate::update::check_for_github_update(&current_ver) {
                    let _ = tx.send(AppEvent::UpdateAvailable(newer));
                }
            });
            return false;
        }
        Action::Update => {
            let msg = if let Some(ref ver) = state.available_update {
                format!(
                    "New TAG v{} ready! Exit TAG and run 'tag update' in your shell to upgrade.",
                    ver
                )
            } else {
                "Run 'tag update' in your shell to check and update to the latest release.".into()
            };
            state.toasts.push(Toast {
                title: "Update TAG".into(),
                message: msg,
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 6,
            });
            return false;
        }
        Action::ToggleTabTray => {
            state.tab_tray.is_open = !state.tab_tray.is_open;
            state.needs_clear = true;
        }
        Action::ToggleCommandPalette => {
            state.palette.is_open = !state.palette.is_open;
            if state.palette.is_open {
                refresh_tmux_commands(&mut state.command_registry);
            }
            state.palette.query.clear();
            state.palette.selected_index = 0;
            state.palette.scroll_offset = 0;
        }
        Action::ToggleHelp => {
            state.help_open = !state.help_open;
            state.help_scroll = 0;
        }
        Action::ToggleWhichKey => {}
        Action::ToggleAttentionQueue => {
            state.attention_state.is_open = !state.attention_state.is_open;
        }
        Action::ToggleUsageOverlay => {
            state.usage_state.is_open = !state.usage_state.is_open;
        }
        Action::ToggleGitOverlay => {
            state.git_state.is_open = !state.git_state.is_open;
            if state.git_state.is_open {
                request_git_refresh(state);
            } else {
                state.git_cache = None;
                state.git_state.prompt = None;
            }
        }
        Action::ToggleSnippetOverlay => {
            state.snippet_state.is_open = !state.snippet_state.is_open;
            state.snippet_state.fill = None;
            if state.snippet_state.is_open {
                let snippets_path = dirs_or_home()
                    .join(".config")
                    .join("tag")
                    .join("snippets.toml");
                state.snippets = SnippetBank::load_from_file_or_default(&snippets_path);
                if state.snippet_state.selected_index >= state.snippets.snippets.len() {
                    state.snippet_state.selected_index =
                        state.snippets.snippets.len().saturating_sub(1);
                }
            }
        }
        Action::ToggleVaultOverlay => {
            state.vault_state.is_open = !state.vault_state.is_open;
            if state.vault_state.is_open {
                if state.vault.is_unlocked() {
                    state.vault_state.mode = VaultMode::Viewing;
                } else {
                    state.vault_state.mode = VaultMode::Locked;
                }
                state.vault_state.clear_input();
                state.vault_state.clear_pending_passphrase();
                state.vault_state.pending_key.clear();
                state.vault_state.status_message = None;
            }
        }
        Action::ToggleBrainOverlay => {
            if state.brain_state.is_open {
                state.brain_state.close();
                state.brain = None;
            } else {
                open_brain_overlay(state);
            }
        }
        Action::ToggleQuickSwitcher => {
            state.palette.is_open = true;
            refresh_tmux_commands(&mut state.command_registry);
            state.palette.query.clear();
            state.palette.selected_index = 0;
            state.palette.scroll_offset = 0;
        }
        Action::TogglePassthrough => {
            state.key_state_machine.toggle_passthrough();
        }
        Action::ToggleDnd => {
            state.dnd_active = !state.dnd_active;
        }
        Action::ToggleZoom => {
            state.layout_mgr.active_workspace_mut().toggle_zoom();
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::UndoLayout => {
            state.layout_mgr.active_workspace_mut().undo_layout();
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::ToggleWorkers => action_toggle_workers(state),
        Action::Peek => action_peek(state),
        Action::ToggleRunView => {
            action_toggle_run_view(state);
        }
        Action::Orchestrate => {
            state.goal_prompt = Some(String::new());
        }
        Action::RenamePane(target_pid) => {
            let pid = target_pid.or(state.layout_mgr.active_workspace().focused_pane);
            if let Some(pid) = pid {
                let current_title = state.panes.get(&pid).map(|p| p.title()).unwrap_or_default();
                state.rename_prompt = Some(RenameModalState::new(pid, current_title));
            }
        }
        Action::Split(direction) => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Split Disabled".into(),
                    message: "Pane is maximized. Restore (Alt+Enter, Leader+m, or click [-]) before splitting.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let new_id = state.allocate_pane_id();
            let env = vault_env(state);
            let (w, h) = initial_pane_size(state);
            // New splits start where the focused pane currently is
            let split_cwd = focused_pane_cwd(state).or_else(|| Some(dirs_or_home()));

            if let Ok(new_pane) = PtyPane::spawn(
                new_id,
                format!("Shell-{}", new_id.0),
                PaneKind::Shell,
                None,
                &[],
                &env,
                split_cwd,
                w,
                h,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                state.panes.insert(new_id, Box::new(new_pane));
                let _ = state
                    .layout_mgr
                    .active_workspace_mut()
                    .split_current(new_id, direction);
                save_session_to_disk(state);
                state.needs_clear = true;
            }
        }
        Action::ClosePane => {
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                close_pane(state, focused);
            }
        }
        Action::SwitchWorkspaceIndex(idx) => {
            if state.layout_mgr.switch_workspace_index(idx) {
                state.needs_clear = true;
            }
        }
        Action::SendLeaderKey => {
            let leader = state.key_state_machine.keymap().leader.clone();
            let ev = crossterm::event::KeyEvent::new(leader.code, leader.modifiers);
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused) {
                    let bytes = key_event_to_bytes(ev, pane.application_cursor());
                    pane.send_input(&bytes);
                }
            }
        }
        Action::SwitchWorkspace(ws_id) => {
            state.layout_mgr.switch_workspace(ws_id);
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::NextWorkspace => {
            state.layout_mgr.next_workspace();
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::PrevWorkspace => {
            state.layout_mgr.prev_workspace();
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::NewWorkspace => {
            // Capture the focused pane's directory before the new workspace becomes active
            let ws_cwd_hint = focused_pane_cwd(state);
            let _new_id = state
                .layout_mgr
                .add_workspace(format!("WS-{}", state.layout_mgr.workspaces().len() + 1));
            let ws_cwd = ws_cwd_hint.or_else(|| Some(dirs_or_home()));
            let new_pid = state.allocate_pane_id();
            let (w, h) = initial_pane_size(state);
            let env = vault_env(state);

            if let Ok(new_pane) = PtyPane::spawn(
                new_pid,
                "Shell".into(),
                PaneKind::Shell,
                None,
                &[],
                &env,
                ws_cwd,
                w,
                h,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                state.panes.insert(new_pid, Box::new(new_pane));
                state
                    .layout_mgr
                    .active_workspace_mut()
                    .add_initial_pane(new_pid);
                save_session_to_disk(state);
                state.needs_clear = true;
            }
        }
        Action::CloseWorkspace(ws_id) => {
            if state.layout_mgr.workspaces().len() <= 1 {
                state.toasts.push(Toast {
                    title: "Workspace".into(),
                    message: "Cannot close the only remaining workspace.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let (closed, pids) = state.layout_mgr.close_workspace(ws_id);
            if closed {
                for pid in &pids {
                    // Workers of an active run keep running, hidden (§5.4).
                    let in_run = state
                        .pane_meta
                        .get(pid)
                        .and_then(|m| m.run.as_ref())
                        .is_some_and(|r| state.runs.contains_key(r));
                    if in_run {
                        place_pane(state, *pid, tag_orchestrator::Visibility::Hidden, None);
                        continue;
                    }
                    if let Some(mut pane) = state.panes.remove(pid) {
                        pane.kill();
                    }
                    state.attention_queue.clear_by_pane(*pid);
                }
                save_session_to_disk(state);
                state.needs_clear = true;
                state.toasts.push(Toast {
                    title: "Workspace Closed".into(),
                    message: format!("Closed workspace and terminated {} pane(s).", pids.len()),
                    level: ToastLevel::Info,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
            }
        }
        Action::CloseActiveWorkspace => {
            let active_id = state.layout_mgr.active_workspace_id();
            return execute_action(state, Action::CloseWorkspace(active_id));
        }
        Action::Focus(dir) => {
            // Move focus to the pane that is actually in that direction on screen
            let area = content_area(state);
            let ws = state.layout_mgr.active_workspace();
            if let Some(curr) = ws.focused_pane {
                if let Some(next) = ws.neighbor_in_direction(area, curr, dir) {
                    state.layout_mgr.active_workspace_mut().focused_pane = Some(next);
                }
            }
        }
        Action::FocusPane(pid) => {
            focus_pane_anywhere(state, pid);
        }
        Action::SpawnShell => {
            execute_action(state, Action::Split(SplitDirection::Vertical));
        }
        Action::SpawnAgent { profile, worktree } => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Spawn Disabled".into(),
                    message: "Pane is maximized. Restore (Alt+Enter, Leader+m, or click [-]) before spawning a new pane.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let pid = state.allocate_pane_id();
            if let Some(prof_cfg) = state.config.agents.get(&profile).cloned() {
                let mut prof = AgentProfile::from_config(&prof_cfg);
                // Let TAG choose the conversation id so the pane can resume after a restart.
                let session_id = (!prof_cfg.session_id_args.is_empty()).then(new_session_uuid);
                let (command, args) = prof_cfg.build_argv(&tag_core::config::LaunchOpts {
                    new_session_id: session_id.clone(),
                    ..Default::default()
                });
                prof.command = command;
                prof.args = args;
                let (w, h) = if state.term_width > 0 && state.term_height > 0 {
                    (
                        state.term_width.saturating_sub(2),
                        state.term_height.saturating_sub(4),
                    )
                } else {
                    (80, 24)
                };
                match AgentPane::spawn(
                    pid,
                    &prof,
                    worktree,
                    w,
                    h,
                    state.config.scrollback_lines,
                    state.event_tx.clone(),
                ) {
                    Ok(agent_pane) => {
                        state.panes.insert(pid, Box::new(agent_pane));
                        state.pane_meta.insert(
                            pid,
                            PaneMeta {
                                profile: Some(profile.clone()),
                                session_id,
                                ..Default::default()
                            },
                        );
                        let _ = state
                            .layout_mgr
                            .active_workspace_mut()
                            .split_current(pid, SplitDirection::Vertical);
                        save_session_to_disk(state);
                        state.needs_clear = true;
                    }
                    Err(e) => {
                        state.toasts.push(Toast {
                            title: "Spawn Error".into(),
                            message: format!("Failed to spawn agent '{}': {}", prof.command, e),
                            level: ToastLevel::Error,
                            created_at: chrono::Utc::now(),
                            duration_secs: 4,
                        });
                    }
                }
            } else {
                state.toasts.push(Toast {
                    title: "Unknown Agent".into(),
                    message: format!("Agent profile '{}' not found in config.", profile),
                    level: ToastLevel::Error,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
            }
        }
        Action::AttachTmux { session } => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Attach Disabled".into(),
                    message: "Pane is maximized. Restore (Alt+Enter, Leader+m, or click [-]) before attaching tmux.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let pid = state.allocate_pane_id();
            let (w, h) = if state.term_width > 0 && state.term_height > 0 {
                (
                    state.term_width.saturating_sub(2),
                    state.term_height.saturating_sub(4),
                )
            } else {
                (80, 24)
            };
            match AgentPane::spawn_tmux(
                pid,
                &session,
                w,
                h,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                Ok(tmux_pane) => {
                    state.panes.insert(pid, Box::new(tmux_pane));
                    let _ = state
                        .layout_mgr
                        .active_workspace_mut()
                        .split_current(pid, SplitDirection::Vertical);
                    save_session_to_disk(state);
                    state.needs_clear = true;
                }
                Err(e) => {
                    state.toasts.push(Toast {
                        title: "Tmux Error".into(),
                        message: format!("Failed to attach tmux session '{}': {}", session, e),
                        level: ToastLevel::Error,
                        created_at: chrono::Utc::now(),
                        duration_secs: 4,
                    });
                }
            }
        }
        Action::PasteImageBridge => {
            // Save next to the focused pane's project so the agent there can read it
            let cwd = focused_pane_cwd(state)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            match ImageBridge::save_clipboard_image(&cwd) {
                Ok(path) => {
                    let text = path.to_string_lossy().to_string();
                    paste_into_focused(state, &text);
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
                    state.toasts.push(Toast {
                        title: "Image Bridge".into(),
                        message: format!("Pasted {}", filename),
                        level: ToastLevel::Info,
                        created_at: chrono::Utc::now(),
                        duration_secs: 3,
                    });
                }
                Err(e) => {
                    state.toasts.push(Toast {
                        title: "Image Bridge".into(),
                        message: format!("Image paste failed: {}", e),
                        level: ToastLevel::Error,
                        created_at: chrono::Utc::now(),
                        duration_secs: 6,
                    });
                }
            }
        }
        Action::StartTour => {
            state.tour.is_active = true;
            state.tour.step = 0;
        }
        Action::ReloadConfig => {
            let (config, error) = load_app_config(&state.paths.config_path);
            if let Some(err) = error {
                // Keep the current settings rather than silently falling back to defaults
                state.config_error = Some(err.clone());
                push_toast(
                    state,
                    "Config Error",
                    format!("Not reloaded: {}", err),
                    ToastLevel::Error,
                    8,
                );
            } else {
                state.config_error = None;
                state.config = config;
                state.theme = Theme::from_config(state.config.theme.clone());
                state.key_state_machine =
                    KeyStateMachine::from_config(&state.config, &state.command_registry);
                push_toast(
                    state,
                    "Config",
                    "Configuration reloaded.",
                    ToastLevel::Info,
                    3,
                );
            }
        }
        Action::CopySelection => {
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused) {
                    if let Some(text) = pane.copy_selection() {
                        if !text.is_empty() {
                            copy_to_clipboard(&text);
                            pane.clear_selection();
                            state.toasts.push(Toast {
                                title: "Clipboard".into(),
                                message: "Copied selection to clipboard.".into(),
                                level: ToastLevel::Info,
                                created_at: chrono::Utc::now(),
                                duration_secs: 2,
                            });
                        }
                    }
                }
            }
        }
        Action::PasteFromClipboard => {
            if let Some(text) = get_clipboard_text() {
                route_paste(state, &text);
            } else {
                push_toast(
                    state,
                    "Clipboard Empty",
                    "No text found in system or TAG clipboard.",
                    ToastLevel::Warning,
                    3,
                );
            }
        }
        Action::SpawnApplet { name } => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Applet Disabled".into(),
                    message: "Pane is maximized. Restore before opening an applet.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let pid = state.allocate_pane_id();
            let Some(applet_pane) = make_applet(&name, pid) else {
                push_toast(
                    state,
                    "Unknown Applet",
                    format!("No applet named '{}' (available: chat, poker).", name),
                    ToastLevel::Error,
                    3,
                );
                return false;
            };
            state.panes.insert(pid, applet_pane);
            let _ = state
                .layout_mgr
                .active_workspace_mut()
                .split_current(pid, SplitDirection::Vertical);
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::MinimizePane(target_pid) => {
            let pid = target_pid.or(state.layout_mgr.active_workspace().focused_pane);
            if let Some(pid) = pid {
                state.layout_mgr.active_workspace_mut().minimize_pane(pid);
                save_session_to_disk(state);
                state.needs_clear = true;
                state.toasts.push(Toast {
                    title: "Pane Minimized".into(),
                    message: "Pane minimized. Press Ctrl+Space b or open Tab Tray to restore."
                        .into(),
                    level: ToastLevel::Info,
                    created_at: chrono::Utc::now(),
                    duration_secs: 2,
                });
            }
        }
        Action::RestorePane(target_pid) => {
            state
                .layout_mgr
                .active_workspace_mut()
                .restore_pane(target_pid, None);
            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
            save_session_to_disk(state);
            state.needs_clear = true;
        }
        Action::Detach => {
            // Detaching keeps TAG (and every pane) running in its background tmux session
            if in_tag_background_session() {
                save_session_to_disk(state);
                run_tmux_silent(&["detach-client"]);
            } else {
                push_toast(
                    state,
                    "Detach Unavailable",
                    "TAG is running in the foreground. Start it with 'tag --bg' (or enable background mode) to detach and re-attach later.",
                    ToastLevel::Warning,
                    5,
                );
            }
            return false;
        }
        Action::ToggleBackgroundMode => {
            if let Some(err) = state.config_error.clone() {
                push_toast(
                    state,
                    "Config Error",
                    format!("Fix config.toml before changing settings ({})", err),
                    ToastLevel::Error,
                    6,
                );
                return false;
            }
            state.config.run_in_background = !state.config.run_in_background;
            if let Err(e) = save_app_config(&state.paths.config_path, &state.config) {
                push_toast(
                    state,
                    "Config",
                    format!("Could not save config: {}", e),
                    ToastLevel::Error,
                    5,
                );
            }
            let status = if state.config.run_in_background {
                "enabled (TAG will persist in background)"
            } else {
                "disabled (TAG will run in foreground)"
            };
            state.toasts.push(Toast {
                title: "Background Mode".into(),
                message: format!("Background mode is now {}", status),
                level: ToastLevel::Success,
                created_at: chrono::Utc::now(),
                duration_secs: 3,
            });
        }
        _ => {}
    }
    false
}
