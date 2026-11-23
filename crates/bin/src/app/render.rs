#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use ratatui::layout::{Constraint, Direction as LayoutDir, Layout};
use tag_notify::render_attention_overlay;
use tag_ui::{
    render_command_palette, render_empty_state, render_help_overlay, render_rename_modal,
    render_status_bar, render_tab_tray, render_toasts, render_tour, render_which_key,
    StatusBarProps,
};
use tag_usage::render_usage_overlay;
use tag_vault::render_vault_overlay;

pub(crate) fn render_frame(frame: &mut ratatui::Frame, state: &mut AppState) {
    let size = frame.area();
    if size.width < 10 || size.height < 5 {
        return;
    }

    // Layout: top pane area, bottom status bar (height 1)
    let chunks = Layout::default()
        .direction(LayoutDir::Vertical)
        .constraints([
            Constraint::Length(size.height.saturating_sub(1)),
            Constraint::Length(1),
        ])
        .split(size);

    let main_area = chunks[0];
    let status_area = chunks[1];

    // Determine tab tray split if open
    let (tray_area, content_area) = if state.tab_tray.is_open {
        let tray_w = (main_area.width * 25 / 100)
            .clamp(24, 45)
            .min(main_area.width);
        let split = Layout::default()
            .direction(LayoutDir::Horizontal)
            .constraints([
                Constraint::Length(tray_w),
                Constraint::Length(main_area.width.saturating_sub(tray_w)),
            ])
            .split(main_area);
        (Some(split[0]), split[1])
    } else {
        (None, main_area)
    };

    // 1. Render active workspace panes or empty state
    let active_panes = state.layout_mgr.active_workspace().all_panes();
    if active_panes.is_empty() {
        let min_count = state.layout_mgr.active_workspace().minimized_panes.len();
        render_empty_state(frame, content_area, &state.theme, min_count, TAG_VERSION);
    } else {
        let rects = state.layout_mgr.compute_rects(content_area);
        let focused_id = state.layout_mgr.active_workspace().focused_pane;
        let is_zoomed = state.layout_mgr.active_workspace().zoomed_pane.is_some();

        for (pane_id, area) in rects {
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let is_focused = focused_id == Some(pane_id);
                pane.set_zoomed(is_zoomed && is_focused);
                pane.render(frame, area, is_focused, &state.theme);
            }
        }
    }

    // 2. Render Tab Tray if open
    if let Some(t_area) = tray_area {
        let tray_items = build_tray_items(state);
        render_tab_tray(
            frame,
            t_area,
            &tray_items,
            &mut state.tab_tray,
            &state.theme,
        );
    }

    // 3. Render Status Bar
    render_status_bar(
        frame,
        status_area,
        StatusBarProps {
            layout_mgr: &state.layout_mgr,
            key_state: state.key_state_machine.state(),
            dnd_active: state.dnd_active,
            attention_count: state.attention_queue.count(),
            token_usage_summary: state.cached_usage_summary.as_deref(),
            theme: &state.theme,
            version: TAG_VERSION,
            available_update: state.available_update.as_deref(),
            project: state.active_project_name.as_deref(),
        },
    );

    // 4. Render Overlays
    if state.key_state_machine.should_show_which_key() {
        render_which_key(frame, size, state.key_state_machine.keymap(), &state.theme);
    }

    if state.palette.is_open {
        render_command_palette(
            frame,
            size,
            &state.command_registry,
            &mut state.palette,
            &state.theme,
        );
    }

    if state.help_open {
        render_help_overlay(
            frame,
            size,
            state.key_state_machine.keymap(),
            &mut state.help_scroll,
            TAG_VERSION,
            &state.theme,
        );
    }

    if state.attention_state.is_open {
        render_attention_overlay(
            frame,
            size,
            &state.attention_queue,
            &state.attention_state,
            &state.theme,
        );
    }

    if state.usage_state.is_open {
        render_usage_overlay(
            frame,
            size,
            &state.usage_store,
            &state.usage_state,
            &state.theme,
        );
    }

    if state.git_state.is_open {
        if let Some(ref cache) = state.git_cache {
            tag_git::render_git_overlay(
                frame,
                size,
                &cache.worktrees,
                &cache.repo_status,
                &cache.diff,
                &state.git_state,
                &state.theme,
            );
        }
    }

    if state.snippet_state.is_open {
        tag_clipboard::render_snippet_overlay(
            frame,
            size,
            &state.snippets,
            &state.snippet_state,
            &state.theme,
        );
    }

    if state.vault_state.is_open {
        render_vault_overlay(frame, size, &state.vault, &state.vault_state, &state.theme);
    }

    if state.brain_state.is_open {
        tag_ui::render_brain_overlay(frame, size, &state.brain_state, &state.theme);
    }

    if state.tour.is_active {
        render_tour(frame, size, &state.tour, &state.theme);
    }

    if let Some(ref rename_state) = state.rename_prompt {
        render_rename_modal(frame, size, rename_state, &state.theme);
    }
    if let Some(ref goal) = state.goal_prompt {
        tag_ui::render_prompt_modal(
            frame,
            size,
            " Orchestrate ",
            " Goal for the conductor (project defaults apply):",
            goal,
            72,
            &state.theme,
        );
    }

    render_toasts(frame, size, &state.toasts, &state.theme);
}

pub(crate) fn is_any_overlay_open(state: &AppState) -> bool {
    state.palette.is_open
        || state.help_open
        || state.tour.is_active
        || state.attention_state.is_open
        || state.usage_state.is_open
        || state.git_state.is_open
        || state.snippet_state.is_open
        || state.vault_state.is_open
        || state.rename_prompt.is_some()
        || state.goal_prompt.is_some()
        || state.tab_tray.is_open
        || state.brain_state.is_open
}
