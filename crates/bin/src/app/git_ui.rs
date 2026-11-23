#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::KeyCode;
use std::path::PathBuf;
use tag_core::action::SplitDirection;
use tag_core::pane::PaneKind;
use tag_git::{GitBackend, GitPromptMode, GitPromptState, StandardGitBackend};
use tag_pty::PtyPane;
use tag_ui::ToastLevel;

pub(crate) fn git_toast_result(
    state: &mut AppState,
    ok_title: &str,
    err_title: &str,
    res: Result<String, String>,
) {
    match res {
        Ok(msg) => push_toast(state, ok_title, msg, ToastLevel::Success, 4),
        Err(msg) => push_toast(state, err_title, msg, ToastLevel::Error, 6),
    }
}

/// Main worktree path from the cached overlay data, falling back to the active repo.
pub(crate) fn git_main_worktree_path(state: &AppState) -> PathBuf {
    state
        .git_cache
        .as_ref()
        .and_then(|c| {
            c.worktrees
                .iter()
                .find(|w| w.is_main)
                .map(|w| w.path.clone())
        })
        .unwrap_or_else(|| get_active_repo_path(state))
}

pub(crate) fn open_shell_in(state: &mut AppState, title: String, dir: PathBuf) -> bool {
    let new_id = state.allocate_pane_id();
    let env = vault_env(state);
    let (w, h) = initial_pane_size(state);
    match PtyPane::spawn(
        new_id,
        title,
        PaneKind::Shell,
        None,
        &[],
        &env,
        Some(dir),
        w,
        h,
        state.config.scrollback_lines,
        state.event_tx.clone(),
    ) {
        Ok(new_pane) => {
            state.panes.insert(new_id, Box::new(new_pane));
            let ws = state.layout_mgr.active_workspace_mut();
            ws.zoomed_pane = None;
            let _ = ws.split_current(new_id, SplitDirection::Vertical);
            state.needs_clear = true;
            true
        }
        Err(e) => {
            push_toast(
                state,
                "Spawn Error",
                format!("Failed to open shell: {}", e),
                ToastLevel::Error,
                4,
            );
            false
        }
    }
}

pub(crate) fn handle_git_overlay_key(state: &mut AppState, key: crossterm::event::KeyEvent) {
    // A modal prompt inside the git overlay takes all keys
    if let Some(prompt) = state.git_state.prompt.clone() {
        match prompt.mode {
            GitPromptMode::CreateWorktree => match key.code {
                KeyCode::Esc => state.git_state.prompt = None,
                KeyCode::Backspace => {
                    if let Some(ref mut p) = state.git_state.prompt {
                        p.input.pop();
                    }
                }
                KeyCode::Enter => {
                    let name = prompt.input.trim().to_string();
                    state.git_state.prompt = None;
                    if name.is_empty() {
                        return;
                    }
                    let active_path = get_active_repo_path(state);
                    match state
                        .git_backend
                        .create_worktree(&active_path, &name, &name)
                    {
                        Ok(wt_path) => {
                            state.git_state.is_open = false;
                            state.git_cache = None;
                            if open_shell_in(state, format!("wt: {}", name), wt_path) {
                                push_toast(
                                    state,
                                    "Worktree Created",
                                    format!("Created worktree and opened shell in {}", name),
                                    ToastLevel::Success,
                                    3,
                                );
                            }
                        }
                        Err(e) => push_toast(
                            state,
                            "Worktree Creation Failed",
                            e.to_string(),
                            ToastLevel::Error,
                            5,
                        ),
                    }
                }
                _ => {
                    if let Some(c) = text_input_char(&key) {
                        if let Some(ref mut p) = state.git_state.prompt {
                            p.input.push(c);
                        }
                    }
                }
            },
            GitPromptMode::ConfirmMerge {
                branch,
                target_branch,
                ..
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    state.git_state.prompt = None;
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    state.git_state.prompt = None;
                    let main_path = git_main_worktree_path(state);
                    let res = state
                        .git_backend
                        .merge_branch(&main_path, &branch)
                        .map(|msg| {
                            format!(
                                "Branch '{}' merged into '{}': {}",
                                branch, target_branch, msg
                            )
                        })
                        .map_err(|e| format!("{} (press A to abort the merge)", e));
                    request_git_refresh(state);
                    git_toast_result(state, "Worktree Merged", "Merge Conflict / Error", res);
                }
                _ => {}
            },
            GitPromptMode::ConfirmDelete {
                worktree_name,
                worktree_path,
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    state.git_state.prompt = None;
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    state.git_state.prompt = None;
                    let main_path = git_main_worktree_path(state);
                    let res = state
                        .git_backend
                        .delete_worktree(&main_path, &worktree_path, false)
                        .map(|_| format!("Worktree '{}' removed", worktree_name))
                        .map_err(|e| e.to_string());
                    if res.is_ok() {
                        state.git_state.selected_index = 0;
                        state.git_state.selected_worktree = 0;
                    }
                    request_git_refresh(state);
                    git_toast_result(state, "Worktree Removed", "Removal Failed", res);
                }
                _ => {}
            },
        }
        return;
    }

    let selected_worktree = state
        .git_cache
        .as_ref()
        .and_then(|c| c.worktrees.get(state.git_state.selected_worktree).cloned());

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.git_state.is_open = false;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            let section = state.git_state.selected_section;
            state.git_state.selected_section = if key.code == KeyCode::Tab {
                (section + 1) % 3
            } else {
                (section + 2) % 3
            };
            // The worktree list remembers its selection; other lists start at the top
            state.git_state.selected_index = if state.git_state.selected_section == 0 {
                state.git_state.selected_worktree
            } else {
                0
            };
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.git_state.selected_section == 2 {
                state.git_state.diff_scroll = state.git_state.diff_scroll.saturating_sub(1);
            } else {
                state.git_state.selected_index = state.git_state.selected_index.saturating_sub(1);
                if state.git_state.selected_section == 0
                    && state.git_state.selected_worktree != state.git_state.selected_index
                {
                    state.git_state.selected_worktree = state.git_state.selected_index;
                    state.git_state.diff_scroll = 0;
                    request_git_refresh(state);
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.git_state.selected_section == 2 {
                state.git_state.diff_scroll = state.git_state.diff_scroll.saturating_add(1);
            } else {
                let max_idx = match state.git_state.selected_section {
                    0 => state
                        .git_cache
                        .as_ref()
                        .map(|c| c.worktrees.len())
                        .unwrap_or(0),
                    _ => state
                        .git_cache
                        .as_ref()
                        .map(|c| c.repo_status.changed_files.len())
                        .unwrap_or(0),
                };
                if state.git_state.selected_index + 1 < max_idx {
                    state.git_state.selected_index += 1;
                    if state.git_state.selected_section == 0 {
                        state.git_state.selected_worktree = state.git_state.selected_index;
                        state.git_state.diff_scroll = 0;
                        request_git_refresh(state);
                    }
                }
            }
        }
        KeyCode::Char('c') | KeyCode::Char('n') | KeyCode::Char('a')
            if state.git_state.selected_section == 0 =>
        {
            state.git_state.prompt = Some(GitPromptState {
                mode: GitPromptMode::CreateWorktree,
                input: String::new(),
            });
        }
        KeyCode::Char('A') => {
            let main_path = git_main_worktree_path(state);
            let res = state
                .git_backend
                .abort_merge(&main_path)
                .map(|_| "Merge aborted; worktree restored".to_string())
                .map_err(|e| e.to_string());
            request_git_refresh(state);
            git_toast_result(state, "Merge Aborted", "Abort Failed", res);
        }
        KeyCode::Char('m') if state.git_state.selected_section == 0 => match selected_worktree {
            Some(wt) if !wt.is_main => {
                if let Some(branch) = wt.branch {
                    let target_branch = state
                        .git_cache
                        .as_ref()
                        .and_then(|c| c.worktrees.iter().find(|w| w.is_main))
                        .and_then(|w| w.branch.clone())
                        .unwrap_or_else(|| "the main worktree's HEAD".into());
                    state.git_state.prompt = Some(GitPromptState {
                        mode: GitPromptMode::ConfirmMerge {
                            worktree_name: wt.name,
                            branch,
                            target_branch,
                        },
                        input: String::new(),
                    });
                }
            }
            Some(_) => push_toast(
                state,
                "Cannot Merge Main",
                "Select a feature worktree to merge into the main worktree",
                ToastLevel::Warning,
                3,
            ),
            None => {}
        },
        KeyCode::Char('d') | KeyCode::Char('x') if state.git_state.selected_section == 0 => {
            match selected_worktree {
                Some(wt) if !wt.is_main => {
                    state.git_state.prompt = Some(GitPromptState {
                        mode: GitPromptMode::ConfirmDelete {
                            worktree_name: wt.name,
                            worktree_path: wt.path,
                        },
                        input: String::new(),
                    });
                }
                Some(_) => push_toast(
                    state,
                    "Cannot Delete Main",
                    "Cannot remove the primary repository worktree",
                    ToastLevel::Warning,
                    3,
                ),
                None => {}
            }
        }
        KeyCode::Enter if state.git_state.selected_section == 0 => {
            if let Some(wt) = selected_worktree {
                state.git_state.is_open = false;
                if open_shell_in(state, format!("wt: {}", wt.name), wt.path) {
                    save_session_to_disk(state);
                    push_toast(
                        state,
                        "Worktree Opened",
                        format!("Opened shell in {}", wt.name),
                        ToastLevel::Info,
                        3,
                    );
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn candidate_repo_dir(state: &AppState) -> PathBuf {
    if let Some(p) = focused_pane_cwd(state) {
        return p;
    }
    if let Some(ref binding) = state.layout_mgr.active_workspace().worktree_binding {
        if binding.exists() {
            return binding.clone();
        }
    }
    for pane in state.panes.values() {
        let (_, _, cwd) = pane.running_process();
        if let Some(p) = cwd.or_else(|| pane.cwd()) {
            if p.exists() {
                return p;
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Main repository root for `search_path` (shared by all of its worktrees).
pub(crate) fn resolve_repo_root(search_path: &std::path::Path) -> PathBuf {
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(search_path)
        .output()
    {
        if output.status.success() {
            let common_git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let p = PathBuf::from(common_git_dir);
            if let Some(parent) = p.parent() {
                if parent.exists() {
                    return parent.to_path_buf();
                }
            }
        }
    }

    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(search_path)
        .output()
    {
        if output.status.success() {
            let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !toplevel.is_empty() {
                return PathBuf::from(toplevel);
            }
        }
    }

    search_path.to_path_buf()
}

pub(crate) fn get_active_repo_path(state: &AppState) -> PathBuf {
    resolve_repo_root(&candidate_repo_dir(state))
}

/// Recompute the git overlay data on a background thread. Results are applied on the
/// next tick; responses to superseded requests are discarded by generation.
pub(crate) fn request_git_refresh(state: &mut AppState) {
    state.git_generation += 1;
    state.git_loading = true;
    let generation = state.git_generation;
    let search_path = candidate_repo_dir(state);
    let selected = state.git_state.selected_worktree;
    let tx = state.git_tx.clone();
    let _ = std::thread::Builder::new()
        .name("git-refresh".into())
        .spawn(move || {
            let backend = StandardGitBackend;
            let repo = resolve_repo_root(&search_path);
            let worktrees = backend.list_worktrees(&repo).unwrap_or_default();
            let target = worktrees
                .get(selected)
                .map(|w| w.path.clone())
                .unwrap_or_else(|| repo.clone());
            let repo_status = backend.repo_status(&target).unwrap_or_default();
            let diff = backend.get_diff(&target).unwrap_or_default();
            let _ = tx.send((
                generation,
                CachedGitData {
                    worktrees,
                    repo_status,
                    diff,
                    loaded_at: std::time::Instant::now(),
                },
            ));
        });
}

/// Apply finished background work (usage scans, git refreshes) to the UI state.
pub(crate) fn drain_background_results(state: &mut AppState) {
    let mut usage_changed = false;
    while let Ok(events) = state.usage_rx.try_recv() {
        attribute_usage(state, &events);
        if state.usage_store.insert_events_batch(&events).unwrap_or(0) > 0 {
            usage_changed = true;
        }
    }
    if usage_changed {
        state.update_usage_summary();
        state.is_dirty = true;
    }

    while let Ok((generation, data)) = state.git_rx.try_recv() {
        if generation != state.git_generation {
            continue;
        }
        state.git_loading = false;
        if state.git_state.is_open {
            let count = data.worktrees.len();
            if state.git_state.selected_worktree >= count {
                state.git_state.selected_worktree = count.saturating_sub(1);
                if state.git_state.selected_section == 0 {
                    state.git_state.selected_index = state.git_state.selected_worktree;
                }
            }
            state.git_cache = Some(data);
            state.is_dirty = true;
        }
    }
}
