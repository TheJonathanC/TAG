#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::{Event as CrossEvent, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use tag_clipboard::{copy_to_clipboard, set_internal_clipboard, wrap_bracketed_paste};
use tag_core::action::{Action, SplitDirection};
use tag_core::event::{AppEvent, HookEvent, StateEvent};
use tag_notify::{Notifier, SystemNotifier};
use tag_ui::{
    filter_tray_items, get_matched_commands, get_selected_command, hit_test_palette,
    hit_test_status_bar, tray_item_at_row, PaletteClick, StatusBarClickTarget, Toast, ToastLevel,
};
use tag_vault::VaultMode;
use tracing::info;

pub(crate) fn handle_app_event(state: &mut AppState, event: AppEvent) -> bool {
    let was_overlay_open = is_any_overlay_open(state);
    let should_quit = handle_app_event_inner(state, event);
    state.publish_status_changes();
    let is_overlay_open = is_any_overlay_open(state);
    if was_overlay_open != is_overlay_open {
        state.needs_clear = true;
        state.is_dirty = true;
    }
    should_quit
}

pub(crate) fn handle_app_event_inner(state: &mut AppState, event: AppEvent) -> bool {
    match event {
        AppEvent::Action(action) => {
            state.is_dirty = true;
            execute_action(state, action)
        }
        AppEvent::PtyOutput { pane_id, data } => {
            state.is_dirty = true;
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let _ = pane.handle_event(&AppEvent::PtyOutput { pane_id, data });
            }
            false
        }
        AppEvent::PtyExited { pane_id, exit_code } => {
            state.is_dirty = true;
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let _ = pane.handle_event(&AppEvent::PtyExited { pane_id, exit_code });
            }
            // Flush any final status change before the pane disappears
            state.publish_status_changes();
            for ws in state.layout_mgr.workspaces_mut() {
                ws.remove_pane(pane_id);
            }
            state.panes.remove(&pane_id);
            state.last_status.remove(&pane_id);
            state.publish(StateEvent::PaneExited {
                pane: pane_id,
                exit_code,
                at: chrono::Utc::now(),
            });
            state.attention_queue.clear_by_pane(pane_id);
            state.needs_clear = true;
            save_session_to_disk(state);
            false
        }
        AppEvent::AgentHook(payload) => {
            state.is_dirty = true;
            if let HookEvent::PasteImage = payload.event {
                let target_pid = payload
                    .pane_id
                    .or(state.layout_mgr.active_workspace().focused_pane);
                if let Some(pid) = target_pid {
                    if let Some(pane) = state.panes.get_mut(&pid) {
                        let bytes = if pane.bracketed_paste() {
                            wrap_bracketed_paste(&payload.message)
                        } else {
                            payload.message.as_bytes().to_vec()
                        };
                        pane.send_input(&bytes);
                    }
                }
                let filename = std::path::Path::new(&payload.message)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&payload.message);
                state.toasts.push(Toast {
                    title: "Image Bridge".into(),
                    message: format!("Pasted {}", filename),
                    level: ToastLevel::Info,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }

            info!("Agent hook event received: {:?}", payload.event);
            let item_id = state.attention_queue.push(
                payload.pane_id,
                payload.agent.clone(),
                payload.event.clone(),
                payload.message.clone(),
                payload.worktree.clone(),
            );
            state.publish(StateEvent::AttentionAdded {
                item: item_id,
                pane: payload.pane_id,
                agent: payload.agent.clone(),
                event: payload.event.clone(),
                message: payload.message.clone(),
                at: chrono::Utc::now(),
            });

            // Toast notification (when notifications enabled)
            if state.config.notifications.enabled {
                state.toasts.push(Toast {
                    title: format!("{} Alert", payload.agent),
                    message: payload.message.clone(),
                    level: match payload.event {
                        HookEvent::Error => ToastLevel::Error,
                        HookEvent::TaskDone => ToastLevel::Success,
                        _ => ToastLevel::Warning,
                    },
                    created_at: chrono::Utc::now(),
                    duration_secs: 5,
                });
            }

            // Out-of-app notification & bell if not DND, enabled, focus policy satisfied, and debounced
            let debounce_ok = state
                .last_notification_time
                .map(|t| {
                    t.elapsed()
                        >= std::time::Duration::from_millis(state.config.notifications.debounce_ms)
                })
                .unwrap_or(true);
            let focus_ok =
                !state.config.notifications.only_when_unfocused || !state.terminal_focused;

            if !state.dnd_active && state.config.notifications.enabled && focus_ok && debounce_ok {
                state.last_notification_time = Some(std::time::Instant::now());
                if state.config.notifications.desktop_notifications {
                    // D-Bus/notification daemons can block for a long time; never on the UI thread
                    let summary = format!("TAG - {}", payload.agent);
                    let body = payload.message.clone();
                    let _ = std::thread::Builder::new()
                        .name("desktop-notify".into())
                        .spawn(move || SystemNotifier.notify(&summary, &body));
                }
                if state.config.notifications.sound {
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(b"\x07");
                    let _ = std::io::stdout().flush();
                }
            }

            // Route to target pane
            if let Some(pid) = payload.pane_id {
                if let Some(pane) = state.panes.get_mut(&pid) {
                    let _ = pane.handle_event(&AppEvent::AgentHook(payload));
                }
            }
            state.update_usage_summary();
            false
        }
        AppEvent::TerminalInput(CrossEvent::Key(key)) => {
            state.is_dirty = true;
            // Check rename modal prompt first
            if let Some(ref mut rename) = state.rename_prompt {
                match key.code {
                    KeyCode::Enter => {
                        let new_title = rename.input.trim().to_string();
                        let pid = rename.pane_id;
                        if !new_title.is_empty() {
                            if let Some(pane) = state.panes.get_mut(&pid) {
                                pane.set_title(new_title.clone());
                                state.toasts.push(Toast {
                                    title: "Pane Renamed".into(),
                                    message: format!("Title updated to '{}'", new_title),
                                    level: ToastLevel::Success,
                                    created_at: chrono::Utc::now(),
                                    duration_secs: 2,
                                });
                            }
                        }
                        state.rename_prompt = None;
                        save_session_to_disk(state);
                    }
                    KeyCode::Esc => {
                        state.rename_prompt = None;
                    }
                    KeyCode::Backspace => {
                        rename.input.pop();
                    }
                    _ => {
                        if let Some(c) = text_input_char(&key) {
                            rename.input.push(c);
                        }
                    }
                }
                return false;
            }

            // Check modal overlays first
            if state.palette.is_open {
                let matched_count =
                    get_matched_commands(&state.command_registry, &state.palette.query).len();
                match key.code {
                    KeyCode::Esc => {
                        state.palette.is_open = false;
                    }
                    KeyCode::Up => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if matched_count > 0 && state.palette.selected_index + 1 < matched_count {
                            state.palette.selected_index += 1;
                        }
                    }
                    KeyCode::PageUp => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(8);
                    }
                    KeyCode::PageDown => {
                        if matched_count > 0 {
                            state.palette.selected_index =
                                (state.palette.selected_index + 8).min(matched_count - 1);
                        }
                    }
                    KeyCode::Home => {
                        state.palette.selected_index = 0;
                    }
                    KeyCode::End => {
                        if matched_count > 0 {
                            state.palette.selected_index = matched_count - 1;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(cmd) =
                            get_selected_command(&state.command_registry, &state.palette)
                        {
                            let act = cmd.action.clone();
                            state.palette.is_open = false;
                            return execute_action(state, act);
                        }
                    }
                    KeyCode::Backspace => {
                        state.palette.query.pop();
                        state.palette.selected_index = 0;
                        state.palette.scroll_offset = 0;
                    }
                    _ => {
                        if let Some(c) = text_input_char(&key) {
                            state.palette.query.push(c);
                            state.palette.selected_index = 0;
                            state.palette.scroll_offset = 0;
                        }
                    }
                }
                return false;
            }

            if state.help_open {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        state.help_open = false;
                        state.help_scroll = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.help_scroll = state.help_scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        state.help_scroll += 1;
                    }
                    KeyCode::PageUp => {
                        state.help_scroll = state.help_scroll.saturating_sub(10);
                    }
                    KeyCode::PageDown => {
                        state.help_scroll += 10;
                    }
                    _ => {}
                }
                return false;
            }

            if state.tour.is_active {
                match key.code {
                    KeyCode::Esc => {
                        state.tour.is_active = false;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        state.tour.step += 1;
                        if state.tour.step >= tag_ui::TOUR_STEPS.len() {
                            state.tour.is_active = false;
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.attention_state.is_open {
                match key.code {
                    KeyCode::Esc => state.attention_state.is_open = false,
                    KeyCode::Up => {
                        state.attention_state.selected_index =
                            state.attention_state.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        let count = state.attention_queue.count();
                        if count > 0 {
                            state.attention_state.selected_index =
                                (state.attention_state.selected_index + 1).min(count - 1);
                        }
                    }
                    KeyCode::Enter => {
                        let item = state
                            .attention_queue
                            .items()
                            .get(state.attention_state.selected_index)
                            .cloned();
                        if let Some(item) = item {
                            match item.pane_id {
                                Some(pid) => {
                                    if !focus_pane_anywhere(state, pid) {
                                        push_toast(
                                            state,
                                            "Attention Queue",
                                            "That pane no longer exists.",
                                            ToastLevel::Warning,
                                            3,
                                        );
                                    }
                                    state.attention_queue.clear_by_pane(pid);
                                }
                                None => {
                                    state.attention_queue.clear_item(item.id);
                                }
                            }
                        }
                        state.attention_state.selected_index = 0;
                        state.attention_state.is_open = false;
                    }
                    KeyCode::Char('x') | KeyCode::Char('d') | KeyCode::Delete => {
                        // Dismiss the selected alert without jumping to it
                        let id = state
                            .attention_queue
                            .items()
                            .get(state.attention_state.selected_index)
                            .map(|i| i.id);
                        if let Some(id) = id {
                            state.attention_queue.clear_item(id);
                        }
                        let count = state.attention_queue.count();
                        state.attention_state.selected_index = state
                            .attention_state
                            .selected_index
                            .min(count.saturating_sub(1));
                    }
                    _ => {}
                }
                return false;
            }

            if state.usage_state.is_open {
                if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                    state.usage_state.is_open = false;
                }
                return false;
            }

            if state.git_state.is_open {
                handle_git_overlay_key(state, key);
                return false;
            }

            if state.snippet_state.is_open {
                if let Some(ref mut fill) = state.snippet_state.fill {
                    match key.code {
                        KeyCode::Esc => state.snippet_state.fill = None,
                        KeyCode::Backspace => {
                            fill.input.pop();
                        }
                        KeyCode::Enter => {
                            if let Some(text) = state.snippet_state.submit_fill_value() {
                                state.snippet_state.is_open = false;
                                paste_into_focused(state, &text);
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                fill.input.push(c);
                            }
                        }
                    }
                    return false;
                }
                match key.code {
                    KeyCode::Esc => state.snippet_state.is_open = false,
                    KeyCode::Up => {
                        state.snippet_state.selected_index =
                            state.snippet_state.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if !state.snippets.snippets.is_empty() {
                            state.snippet_state.selected_index =
                                (state.snippet_state.selected_index + 1)
                                    .min(state.snippets.snippets.len() - 1);
                        }
                    }
                    KeyCode::Enter => {
                        let template = state
                            .snippets
                            .snippets
                            .get(state.snippet_state.selected_index)
                            .map(|snip| snip.template.clone());
                        if let Some(template) = template {
                            // Templates with {{placeholders}} prompt for values first
                            if let Some(text) = state.snippet_state.begin_fill(&template) {
                                state.snippet_state.is_open = false;
                                paste_into_focused(state, &text);
                            }
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.vault_state.is_open {
                // If not unlocked, handle passphrase input
                if !state.vault.is_unlocked() {
                    let creating = !state.vault.exists();
                    match key.code {
                        KeyCode::Esc => {
                            state.vault_state.clear_input();
                            if state.vault_state.pending_new_passphrase.is_some() {
                                // Start over choosing the new passphrase
                                state.vault_state.clear_pending_passphrase();
                                state.vault_state.status_message = None;
                            } else {
                                state.vault_state.is_open = false;
                            }
                        }
                        KeyCode::Backspace => {
                            state.vault_state.input_buffer.pop();
                        }
                        KeyCode::Enter if creating && state.vault_state.input_buffer.is_empty() => {
                            state.vault_state.status_message =
                                Some("Passphrase cannot be empty".into());
                        }
                        KeyCode::Enter
                            if creating && state.vault_state.pending_new_passphrase.is_none() =>
                        {
                            // New vault: ask for the passphrase twice before creating it
                            state.vault_state.pending_new_passphrase =
                                Some(std::mem::take(&mut state.vault_state.input_buffer));
                            state.vault_state.status_message = None;
                        }
                        KeyCode::Enter
                            if creating
                                && state.vault_state.pending_new_passphrase.as_deref()
                                    != Some(state.vault_state.input_buffer.as_str()) =>
                        {
                            state.vault_state.clear_input();
                            state.vault_state.clear_pending_passphrase();
                            state.vault_state.status_message = Some(
                                "Passphrases did not match. Choose a passphrase again.".into(),
                            );
                        }
                        KeyCode::Enter => {
                            let res = state.vault.unlock(&state.vault_state.input_buffer);
                            state.vault_state.clear_input();
                            state.vault_state.clear_pending_passphrase();
                            match res {
                                Ok(_) => {
                                    state.vault_state.mode = VaultMode::Viewing;
                                    state.vault_state.status_message =
                                        Some("Unlocked successfully".into());
                                }
                                Err(e) => {
                                    state.vault_state.status_message =
                                        Some(format!("Error: {}", e));
                                }
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.vault_state.input_buffer.push(c);
                            }
                        }
                    }
                    return false;
                }

                // If unlocked, handle based on vault mode
                match state.vault_state.mode {
                    VaultMode::Locked => {
                        state.vault_state.mode = VaultMode::Viewing;
                    }
                    VaultMode::Viewing => {
                        let keys = state.vault.list_keys();
                        match key.code {
                            KeyCode::Esc => {
                                state.vault_state.clear_input();
                                state.vault_state.is_open = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                                state.vault_state.selected_index =
                                    state.vault_state.selected_index.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                                if !keys.is_empty() {
                                    state.vault_state.selected_index =
                                        (state.vault_state.selected_index + 1).min(keys.len() - 1);
                                }
                            }
                            KeyCode::Char('a') | KeyCode::Char('A') if key.modifiers.is_empty() => {
                                state.vault_state.mode = VaultMode::AddingKey;
                                state.vault_state.clear_input();
                                state.vault_state.pending_key.clear();
                                state.vault_state.status_message = None;
                            }
                            KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete
                                if key.modifiers.is_empty() || key.code == KeyCode::Delete =>
                            {
                                if let Some(key_to_delete) =
                                    keys.get(state.vault_state.selected_index)
                                {
                                    let key_name = key_to_delete.clone();
                                    state.vault.delete_secret(&key_name);
                                    let _ = state.vault.save_current();
                                    state.vault_state.status_message =
                                        Some(format!("Deleted secret '{}'", key_name));
                                    let remaining_keys = state.vault.list_keys();
                                    if state.vault_state.selected_index >= remaining_keys.len() {
                                        state.vault_state.selected_index =
                                            remaining_keys.len().saturating_sub(1);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    VaultMode::AddingKey => match key.code {
                        KeyCode::Esc => {
                            state.vault_state.clear_input();
                            state.vault_state.pending_key.clear();
                            state.vault_state.mode = VaultMode::Viewing;
                            state.vault_state.status_message = None;
                        }
                        KeyCode::Backspace => {
                            state.vault_state.input_buffer.pop();
                        }
                        KeyCode::Enter => {
                            let key_name = state.vault_state.input_buffer.trim().to_string();
                            if !key_name.is_empty() {
                                state.vault_state.pending_key = key_name;
                                state.vault_state.clear_input();
                                state.vault_state.mode = VaultMode::AddingValue;
                                state.vault_state.status_message = None;
                            } else {
                                state.vault_state.status_message =
                                    Some("Key name cannot be empty".into());
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.vault_state.input_buffer.push(c);
                            }
                        }
                    },
                    VaultMode::AddingValue => match key.code {
                        KeyCode::Esc => {
                            state.vault_state.clear_input();
                            state.vault_state.pending_key.clear();
                            state.vault_state.mode = VaultMode::Viewing;
                            state.vault_state.status_message = None;
                        }
                        KeyCode::Backspace => {
                            state.vault_state.input_buffer.pop();
                        }
                        KeyCode::Enter => {
                            let secret_val = state.vault_state.input_buffer.clone();
                            let key_name = state.vault_state.pending_key.clone();
                            state.vault.set_secret(key_name.clone(), secret_val);
                            let save_res = state.vault.save_current();
                            state.vault_state.clear_input();
                            state.vault_state.pending_key.clear();
                            state.vault_state.mode = VaultMode::Viewing;
                            match save_res {
                                Ok(_) => {
                                    state.vault_state.status_message =
                                        Some(format!("Secret '{}' saved and encrypted", key_name));
                                }
                                Err(e) => {
                                    state.vault_state.status_message =
                                        Some(format!("Failed to save secret: {}", e));
                                }
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.vault_state.input_buffer.push(c);
                            }
                        }
                    },
                }
                return false;
            }

            if state.tab_tray.is_open {
                let tray_items = build_tray_items(state);
                let filtered = filter_tray_items(&tray_items, &state.tab_tray.search_query);
                let count = filtered.len();

                if state.tab_tray.is_searching {
                    match key.code {
                        KeyCode::Esc => {
                            state.tab_tray.is_searching = false;
                            state.tab_tray.search_query.clear();
                            state.tab_tray.selected_index = 0;
                        }
                        KeyCode::Enter => {
                            state.tab_tray.is_searching = false;
                        }
                        KeyCode::Up => {
                            state.tab_tray.selected_index =
                                state.tab_tray.selected_index.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            if count > 0 {
                                state.tab_tray.selected_index =
                                    (state.tab_tray.selected_index + 1).min(count - 1);
                            }
                        }
                        KeyCode::Backspace => {
                            state.tab_tray.search_query.pop();
                            state.tab_tray.selected_index = 0;
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.tab_tray.search_query.push(c);
                                state.tab_tray.selected_index = 0;
                            }
                        }
                    }
                    return false;
                }

                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        state.tab_tray.is_open = false;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.tab_tray.selected_index =
                            state.tab_tray.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if count > 0 {
                            state.tab_tray.selected_index =
                                (state.tab_tray.selected_index + 1).min(count - 1);
                        }
                    }
                    KeyCode::Char('/') => {
                        state.tab_tray.is_searching = true;
                    }
                    KeyCode::Enter => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            focus_pane_anywhere(state, target_pid);
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            let is_min = item.is_minimized;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            if is_min {
                                if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                    ws.restore_pane(target_pid, Some(SplitDirection::Horizontal));
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                                save_session_to_disk(state);
                            } else {
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                                return execute_action(
                                    state,
                                    Action::Split(SplitDirection::Horizontal),
                                );
                            }
                        }
                    }
                    KeyCode::Char('v') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            let is_min = item.is_minimized;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            if is_min {
                                if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                    ws.restore_pane(target_pid, Some(SplitDirection::Vertical));
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                                save_session_to_disk(state);
                            } else {
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                                return execute_action(
                                    state,
                                    Action::Split(SplitDirection::Vertical),
                                );
                            }
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let pid = item.pane_id;
                            close_pane(state, pid);
                            if state.tab_tray.selected_index > 0
                                && state.tab_tray.selected_index >= count.saturating_sub(1)
                            {
                                state.tab_tray.selected_index =
                                    state.tab_tray.selected_index.saturating_sub(1);
                            }
                        }
                    }
                    KeyCode::Char('r') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            return execute_action(state, Action::RenamePane(Some(pid)));
                        }
                    }
                    KeyCode::Char('m') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            let is_min = item.is_minimized;
                            if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                if is_min {
                                    ws.restore_pane(target_pid, None);
                                } else {
                                    ws.minimize_pane(target_pid);
                                }
                                save_session_to_disk(state);
                            }
                        }
                    }
                    KeyCode::Char('z') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            state.layout_mgr.switch_workspace(target_ws);
                            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
                            state.layout_mgr.active_workspace_mut().toggle_zoom();
                            save_session_to_disk(state);
                        }
                    }
                    _ => {}
                }
                return false;
            }

            // If active workspace is empty, check empty state actions and leader keys
            if state.layout_mgr.active_workspace().all_panes().is_empty() {
                if let Some(action) = state.key_state_machine.process_key(key) {
                    return execute_action(state, action);
                }

                let is_ctrl_c =
                    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                let is_q = key.code == KeyCode::Char('q') && key.modifiers.is_empty();
                if is_ctrl_c || is_q {
                    return confirm_quit_from_empty_workspace(state);
                }
                if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
                    return false;
                }

                match key.code {
                    KeyCode::Char('c') | KeyCode::Enter => {
                        let min_count = state.layout_mgr.active_workspace().minimized_panes.len();
                        if min_count > 0 && key.code == KeyCode::Enter {
                            if let Some(first_min) = state
                                .layout_mgr
                                .active_workspace()
                                .minimized_panes
                                .first()
                                .copied()
                            {
                                state
                                    .layout_mgr
                                    .active_workspace_mut()
                                    .restore_pane(first_min, None);
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(first_min);
                                save_session_to_disk(state);
                                return false;
                            }
                        }
                        return execute_action(state, Action::SpawnShell);
                    }
                    KeyCode::Char('w') => {
                        return execute_action(state, Action::NewWorkspace);
                    }
                    KeyCode::Char('p') => {
                        return execute_action(state, Action::ToggleCommandPalette);
                    }
                    KeyCode::Char('b') => {
                        return execute_action(state, Action::ToggleTabTray);
                    }
                    KeyCode::Char('?') => {
                        return execute_action(state, Action::ToggleHelp);
                    }
                    _ => {}
                }
                return false;
            }

            let was_leader = state.key_state_machine.state() == tag_keys::KeyState::LeaderPending;
            // Process through key state machine
            if let Some(action) = state.key_state_machine.process_key(key) {
                return execute_action(state, action);
            }
            if was_leader {
                // Key was entered while Leader was pending; do not forward unmapped follower to child pane!
                state.is_dirty = true;
                return false;
            }

            // Direct Ctrl+Shift+V or Shift+Insert for paste
            if (key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT)
                && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V')))
                || (key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Insert)
            {
                return execute_action(state, Action::PasteFromClipboard);
            }

            // Check Shift+PageUp / Shift+PageDown for viewport scrollback
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                match key.code {
                    KeyCode::PageUp => {
                        if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                            if let Some(pane) = state.panes.get_mut(&focused_id) {
                                pane.scroll(15);
                            }
                        }
                        return false;
                    }
                    KeyCode::PageDown => {
                        if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                            if let Some(pane) = state.panes.get_mut(&focused_id) {
                                pane.scroll(-15);
                            }
                        }
                        return false;
                    }
                    _ => {}
                }
            }

            // If focused pane has an active selection and Ctrl+C is pressed, copy the selection
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
            {
                let has_sel = state
                    .layout_mgr
                    .active_workspace()
                    .focused_pane
                    .and_then(|pid| state.panes.get(&pid))
                    .map(|p| p.has_selection())
                    .unwrap_or(false);

                if has_sel {
                    return execute_action(state, Action::CopySelection);
                }
            }

            // In passthrough or normal non-chord mode, forward key to active child pane
            if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    let bytes = key_event_to_bytes(key, pane.application_cursor());
                    if !bytes.is_empty() {
                        pane.send_input(&bytes);
                        pane.scroll_to_bottom();
                    }
                }
            }
            false
        }
        AppEvent::TerminalInput(CrossEvent::Mouse(mouse)) => {
            state.is_dirty = true;
            if state.palette.is_open {
                let matched_count =
                    get_matched_commands(&state.command_registry, &state.palette.query).len();
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        if matched_count > 0 && state.palette.selected_index + 1 < matched_count {
                            state.palette.selected_index += 1;
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(1);
                    }
                    MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                        let screen = Rect::new(0, 0, state.term_width, state.term_height);
                        match hit_test_palette(
                            screen,
                            &state.palette,
                            matched_count,
                            mouse.column,
                            mouse.row,
                        ) {
                            PaletteClick::Outside => state.palette.is_open = false,
                            PaletteClick::Inside => {}
                            PaletteClick::Item(idx) => {
                                state.palette.selected_index = idx;
                                if let Some(cmd) =
                                    get_selected_command(&state.command_registry, &state.palette)
                                {
                                    let act = cmd.action.clone();
                                    state.palette.is_open = false;
                                    return execute_action(state, act);
                                }
                            }
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.help_open {
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        state.help_scroll += 3;
                    }
                    MouseEventKind::ScrollUp => {
                        state.help_scroll = state.help_scroll.saturating_sub(3);
                    }
                    _ => {}
                }
                return false;
            }

            let tray_w = tray_width(state);

            if state.tab_tray.is_open
                && mouse.column < tray_w
                && mouse.row < state.term_height.saturating_sub(1)
                && !matches!(mouse.kind, MouseEventKind::Down(_))
            {
                let tray_items = build_tray_items(state);
                let filtered = filter_tray_items(&tray_items, &state.tab_tray.search_query);
                let count = filtered.len();
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        if count > 0 {
                            state.tab_tray.selected_index =
                                (state.tab_tray.selected_index + 1).min(count - 1);
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        state.tab_tray.selected_index =
                            state.tab_tray.selected_index.saturating_sub(1);
                    }
                    _ => {}
                }
                return false;
            }

            let content_area = content_area(state);

            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    let target_id = state
                        .layout_mgr
                        .active_workspace()
                        .hit_test(content_area, mouse.column, mouse.row)
                        .or(state.layout_mgr.active_workspace().focused_pane);
                    if let Some(pid) = target_id {
                        let pane_rects = state
                            .layout_mgr
                            .active_workspace()
                            .compute_rects(content_area);
                        let (col, row) = if let Some(rect) = pane_rects.get(&pid) {
                            let inner_w = rect.width.saturating_sub(2).max(1);
                            let inner_h = rect.height.saturating_sub(2).max(1);
                            let inner_x = rect.x + 1;
                            let inner_y = rect.y + 1;
                            let c = mouse.column.saturating_sub(inner_x) + 1;
                            let r = mouse.row.saturating_sub(inner_y) + 1;
                            let max_r = if inner_h > 1 {
                                inner_h.saturating_sub(1)
                            } else {
                                1
                            };
                            (c.clamp(1, inner_w), r.clamp(1, max_r))
                        } else {
                            (1, 1)
                        };
                        if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.scroll_at(3, col, row);
                        }
                    }
                }
                MouseEventKind::ScrollDown => {
                    let target_id = state
                        .layout_mgr
                        .active_workspace()
                        .hit_test(content_area, mouse.column, mouse.row)
                        .or(state.layout_mgr.active_workspace().focused_pane);
                    if let Some(pid) = target_id {
                        let pane_rects = state
                            .layout_mgr
                            .active_workspace()
                            .compute_rects(content_area);
                        let (col, row) = if let Some(rect) = pane_rects.get(&pid) {
                            let inner_w = rect.width.saturating_sub(2).max(1);
                            let inner_h = rect.height.saturating_sub(2).max(1);
                            let inner_x = rect.x + 1;
                            let inner_y = rect.y + 1;
                            let c = mouse.column.saturating_sub(inner_x) + 1;
                            let r = mouse.row.saturating_sub(inner_y) + 1;
                            let max_r = if inner_h > 1 {
                                inner_h.saturating_sub(1)
                            } else {
                                1
                            };
                            (c.clamp(1, inner_w), r.clamp(1, max_r))
                        } else {
                            (1, 1)
                        };
                        if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.scroll_at(-3, col, row);
                        }
                    }
                }
                MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                    let col = mouse.column;
                    let row = mouse.row;

                    // 1. Check if clicked on status bar
                    if state.term_height > 0 && row >= state.term_height.saturating_sub(1) {
                        if let Some(target) = hit_test_status_bar(
                            state.layout_mgr.workspaces(),
                            state.layout_mgr.active_workspace_id(),
                            col,
                        ) {
                            match target {
                                StatusBarClickTarget::Workspace(idx) => {
                                    state.layout_mgr.switch_workspace_index(idx);
                                }
                                StatusBarClickTarget::CloseWorkspace(idx) => {
                                    if let Some(ws) = state.layout_mgr.workspaces().get(idx) {
                                        let ws_id = ws.id;
                                        return execute_action(
                                            state,
                                            Action::CloseWorkspace(ws_id),
                                        );
                                    }
                                }
                                StatusBarClickTarget::AddWorkspace => {
                                    return execute_action(state, Action::NewWorkspace);
                                }
                            }
                        }
                        return false;
                    }

                    // 2. Check if clicked in Tab Tray
                    if state.tab_tray.is_open && col < tray_w {
                        let tray_area =
                            Rect::new(0, 0, tray_w, state.term_height.saturating_sub(1));
                        let tray_items = build_tray_items(state);
                        let target = tray_item_at_row(tray_area, &tray_items, &state.tab_tray, row)
                            .and_then(|idx| {
                                filter_tray_items(&tray_items, &state.tab_tray.search_query)
                                    .get(idx)
                                    .map(|item| item.pane_id)
                            });
                        if let Some(target_pid) = target {
                            state.tab_tray.is_open = false;
                            focus_pane_anywhere(state, target_pid);
                        }
                        return false;
                    }

                    // 3. Content area
                    if state.layout_mgr.active_workspace().all_panes().is_empty() {
                        let min_count = state.layout_mgr.active_workspace().minimized_panes.len();
                        if min_count > 0 {
                            if let Some(first_min) = state
                                .layout_mgr
                                .active_workspace()
                                .minimized_panes
                                .first()
                                .copied()
                            {
                                state
                                    .layout_mgr
                                    .active_workspace_mut()
                                    .restore_pane(first_min, None);
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(first_min);
                                save_session_to_disk(state);
                                return false;
                            }
                        }
                        return execute_action(state, Action::SpawnShell);
                    } else {
                        let pane_rects = state
                            .layout_mgr
                            .active_workspace()
                            .compute_rects(content_area);
                        for (&pid, &rect) in &pane_rects {
                            if col >= rect.x
                                && col < rect.x + rect.width
                                && row >= rect.y
                                && row < rect.y + rect.height
                            {
                                // Clicked on top border
                                if row == rect.y {
                                    // 1. Clicked on top-right [x] close button
                                    if col >= rect.x + rect.width.saturating_sub(4) {
                                        close_pane(state, pid);
                                        return false;
                                    }
                                    // 2. Clicked on maximize/restore button [^] or [-]
                                    if col >= rect.x + rect.width.saturating_sub(8) {
                                        state.layout_mgr.active_workspace_mut().focused_pane =
                                            Some(pid);
                                        state.layout_mgr.active_workspace_mut().toggle_zoom();
                                        save_session_to_disk(state);
                                        return false;
                                    }
                                    // 3. Clicked on minimize button [_]
                                    if col >= rect.x + rect.width.saturating_sub(12) {
                                        state.layout_mgr.active_workspace_mut().minimize_pane(pid);
                                        save_session_to_disk(state);
                                        return false;
                                    }
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane = Some(pid);
                                // Start text selection inside the pane's content area
                                for p in state.panes.values_mut() {
                                    p.clear_selection();
                                }
                                let rel_col = col
                                    .saturating_sub(rect.x + 1)
                                    .min(rect.width.saturating_sub(3));
                                let rel_row = row
                                    .saturating_sub(rect.y + 1)
                                    .min(rect.height.saturating_sub(3));
                                if let Some(pane) = state.panes.get_mut(&pid) {
                                    pane.start_selection(rel_col, rel_row);
                                }
                                state.selecting_pane = Some((pid, rect, (rel_col, rel_row)));
                                break;
                            }
                        }
                    }
                }
                MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                    if let Some((pid, rect, _)) = state.selecting_pane {
                        let rel_col = mouse
                            .column
                            .saturating_sub(rect.x + 1)
                            .min(rect.width.saturating_sub(3));
                        let rel_row = mouse
                            .row
                            .saturating_sub(rect.y + 1)
                            .min(rect.height.saturating_sub(3));
                        if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.update_selection(rel_col, rel_row);
                        }
                    }
                }
                MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                    if let Some((pid, rect, start_pos)) = state.selecting_pane.take() {
                        let rel_col = mouse
                            .column
                            .saturating_sub(rect.x + 1)
                            .min(rect.width.saturating_sub(3));
                        let rel_row = mouse
                            .row
                            .saturating_sub(rect.y + 1)
                            .min(rect.height.saturating_sub(3));
                        if (rel_col, rel_row) != start_pos {
                            if let Some(pane) = state.panes.get_mut(&pid) {
                                pane.update_selection(rel_col, rel_row);
                                if let Some(text) = pane.copy_selection() {
                                    if !text.is_empty() {
                                        copy_to_clipboard(&text);
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
                        } else if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.clear_selection();
                        }
                    }
                }
                MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                    return execute_action(state, Action::PasteFromClipboard);
                }
                _ => {}
            }
            false
        }
        AppEvent::TerminalInput(CrossEvent::Paste(text)) => {
            set_internal_clipboard(&text);
            route_paste(state, &text);
            false
        }
        AppEvent::FocusGained => {
            state.terminal_focused = true;
            false
        }
        AppEvent::FocusLost => {
            state.terminal_focused = false;
            false
        }
        AppEvent::Tick => {
            let now = chrono::Utc::now();
            let before = state.toasts.len();
            state.toasts.retain(|t| {
                now.signed_duration_since(t.created_at).num_seconds() < t.duration_secs as i64
            });
            if before != state.toasts.len() || state.tour.is_active {
                state.is_dirty = true;
            }
            if state.key_state_machine.should_show_which_key() {
                state.is_dirty = true;
            }
            if state.last_usage_query.elapsed() >= std::time::Duration::from_secs(3) {
                let old_summary = state.cached_usage_summary.clone();
                state.update_usage_summary();
                if old_summary != state.cached_usage_summary {
                    state.is_dirty = true;
                }
            }
            if state.last_project_check.elapsed() >= std::time::Duration::from_secs(3)
                && state.refresh_active_project_name()
            {
                state.is_dirty = true;
            }
            drain_background_results(state);
            if state.git_state.is_open && !state.git_loading {
                let needs_reload = match &state.git_cache {
                    Some(cache) => cache.loaded_at.elapsed() > std::time::Duration::from_secs(3),
                    None => true,
                };
                if needs_reload {
                    request_git_refresh(state);
                }
            }
            // Auto-lock wipes decrypted secrets from memory after the idle timeout
            if state.vault.enforce_idle_lock() {
                state.vault_state.mode = VaultMode::Locked;
                state.vault_state.clear_input();
                state.vault_state.pending_key.clear();
                state.vault_state.status_message = Some("Vault locked after idle timeout".into());
                state.is_dirty = true;
            }
            false
        }
        AppEvent::Resize { width, height } => {
            state.is_dirty = true;
            state.needs_clear = true;
            state.term_width = width;
            state.term_height = height;

            let rects = state.layout_mgr.compute_rects(content_area(state));
            for (pane_id, area) in rects {
                if let Some(pane) = state.panes.get_mut(&pane_id) {
                    let inner_w = area.width.saturating_sub(2);
                    let inner_h = area.height.saturating_sub(2);
                    pane.on_resize(inner_w, inner_h);
                }
            }
            false
        }
        AppEvent::UpdateAvailable(ver) => {
            state.available_update = Some(ver.clone());
            state.toasts.push(Toast {
                title: "Update Available".into(),
                message: format!(
                    "TAG v{} is available! Press Leader+U or run 'tag update' to upgrade.",
                    ver
                ),
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 6,
            });
            state.is_dirty = true;
            false
        }
        AppEvent::Control(envelope) => {
            handle_control(state, envelope);
            false
        }
        _ => false,
    }
}

/// `q` / Ctrl+C on an empty workspace quits immediately only when no panes run anywhere;
/// otherwise the first press asks for confirmation.
pub(crate) fn confirm_quit_from_empty_workspace(state: &mut AppState) -> bool {
    let confirmed = state
        .quit_confirm_at
        .is_some_and(|t| t.elapsed() < QUIT_CONFIRM_WINDOW);
    if state.panes.is_empty() || confirmed {
        return execute_action(state, Action::Quit);
    }
    state.quit_confirm_at = Some(std::time::Instant::now());
    let running = state.panes.len();
    push_toast(
        state,
        "Quit TAG?",
        format!(
            "{} pane(s) are still running in other workspaces. Press again to quit and stop them.",
            running
        ),
        ToastLevel::Warning,
        QUIT_CONFIRM_WINDOW.as_secs(),
    );
    false
}
