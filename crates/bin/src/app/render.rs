#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use ratatui::layout::{Constraint, Direction as LayoutDir, Layout};
use tag_notify::render_attention_overlay;
use tag_ui::{
    render_card_strip, render_command_palette, render_empty_state, render_help_overlay,
    render_peek_frame, render_rename_modal, render_status_bar, render_tab_tray, render_toasts,
    render_tour, render_which_key, render_workers_table, StatusBarProps,
};
use tag_usage::render_usage_overlay;
use tag_vault::render_vault_overlay;

pub(crate) fn render_frame(frame: &mut ratatui::Frame, state: &mut AppState) {
    let size = frame.area();
    if size.width < 10 || size.height < 5 {
        return;
    }

    // Layout: top pane area, bottom status bar (height 1)
    let cards = build_card_items(state);
    let strip_h = card_strip_height(state);
    let chunks = Layout::default()
        .direction(LayoutDir::Vertical)
        .constraints([
            Constraint::Length(size.height.saturating_sub(1 + strip_h)),
            Constraint::Length(strip_h),
            Constraint::Length(1),
        ])
        .split(size);

    let main_area = chunks[0];
    let strip_area = chunks[1];
    let status_area = chunks[2];

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

    if strip_h > 0 {
        render_card_strip(frame, strip_area, &cards, &state.theme);
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
        let labels: std::collections::HashMap<u64, String> = state
            .attention_actions
            .iter()
            .map(|(id, a)| (*id, a.label().to_string()))
            .collect();
        render_attention_overlay(
            frame,
            size,
            &state.attention_queue,
            &state.attention_state,
            &state.theme,
            &labels,
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

    if state.workers.is_open {
        let rows = build_worker_rows(state, true);
        render_workers_table(frame, size, &rows, &mut state.workers, &state.theme);
    }

    if let Some(pk) = state.peek {
        if state.panes.contains_key(&pk.pane) {
            let title = state
                .panes
                .get(&pk.pane)
                .map(|p| p.title())
                .unwrap_or_default();
            let inner = render_peek_frame(frame, size, &title, pk.input, &state.theme);
            if let Some(pane) = state.panes.get_mut(&pk.pane) {
                pane.render(frame, inner, true, &state.theme);
            }
        } else {
            state.peek = None;
        }
    }

    if state.tour.is_active {
        render_tour(frame, size, &state.tour, &state.theme);
    }

    if let Some(ref rename_state) = state.rename_prompt {
        render_rename_modal(frame, size, rename_state, &state.theme);
    }
    if state.brain_state.is_open {
        tag_ui::render_brain_overlay(frame, size, &state.brain_state, &state.theme);
    }
    if state.run_view.is_open {
        let data = build_run_view_data(state);
        tag_ui::render_run_view(frame, size, &data, &state.run_view, &state.theme);
    }
    if let Some((pane, ref text)) = state.reply_prompt {
        let title = state
            .panes
            .get(&pane)
            .map(|p| format!(" Reply to {} ", p.title()))
            .unwrap_or_else(|| " Reply ".into());
        tag_ui::render_prompt_modal(
            frame,
            size,
            &title,
            " Text to send (Enter sends and presses Enter):",
            text,
            72,
            &state.theme,
        );
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
        || state.reply_prompt.is_some()
        || state.run_view.is_open
        || state.tab_tray.is_open
        || state.workers.is_open
        || state.peek.is_some()
        || state.brain_state.is_open
}

#[cfg(test)]
mod smoke_tests {
    use super::*;
    use crate::app::state::tests::{test_state, MockAgentPane};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tag_core::pane::PaneId;
    use tag_orchestrator::{
        OrchestrationSettings, Run, RunState, RunStore, Task, TaskState, Visibility,
    };

    /// Render every overlay at several terminal sizes with live panes, a run and attention
    /// items: catches layout-math panics the per-widget tests can't.
    #[test]
    fn full_frame_renders_every_overlay_at_every_size() {
        let (mut state, _rx) = test_state();
        for id in 1..=5u32 {
            state.panes.insert(
                PaneId(id),
                Box::new(MockAgentPane {
                    id: PaneId(id),
                    status: if id == 2 { "awaiting-input" } else { "running" },
                }),
            );
        }
        place_pane(&mut state, PaneId(1), Visibility::Pane, None);
        place_pane(&mut state, PaneId(2), Visibility::Pane, None);
        place_pane(&mut state, PaneId(3), Visibility::Card, None);
        place_pane(&mut state, PaneId(4), Visibility::Card, None);
        place_pane(&mut state, PaneId(5), Visibility::Hidden, None);
        let mut run = Run::new(
            "r_20261123_0915_ab12",
            "p",
            "a goal that is rather long indeed",
            chrono::Utc::now(),
        );
        run.state = RunState::Running;
        for (i, st) in [TaskState::Working, TaskState::Review, TaskState::Merged]
            .iter()
            .enumerate()
        {
            let mut t = Task::new(format!("t{}", i + 1), "some task title", "p");
            t.state = *st;
            t.pane = Some(3 + i as u64);
            run.tasks.push(t);
        }
        state.runs.insert(
            run.id.clone(),
            ActiveRun {
                store: RunStore::new(state.paths.state_dir.join("runs")),
                run,
                repo: state.paths.state_dir.clone(),
                settings: OrchestrationSettings::builtin(),
                announced_all_review: false,
                workspace: None,
            },
        );
        state.attention_queue.push(
            Some(PaneId(2)),
            "run r_20261123_0915_ab12".into(),
            tag_core::event::HookEvent::NeedsPermission,
            "Merge t2?".into(),
            None,
        );

        type Opener = fn(&mut AppState);
        let overlays: Vec<(&str, Opener)> = vec![
            ("none", |_| {}),
            ("workers", |s| s.workers.is_open = true),
            ("peek", |s| {
                s.peek = Some(PeekState {
                    pane: PaneId(3),
                    input: false,
                })
            }),
            ("run_view", |s| s.run_view.is_open = true),
            ("attention", |s| s.attention_state.is_open = true),
            ("brain", |s| s.brain_state.is_open = true),
            ("goal", |s| s.goal_prompt = Some("x".repeat(200))),
            ("reply", |s| {
                s.reply_prompt = Some((PaneId(2), "hello".into()))
            }),
            ("tray", |s| s.tab_tray.is_open = true),
            ("help", |s| s.help_open = true),
        ];
        for (w, h) in [(40u16, 10u16), (80, 24), (200, 60)] {
            for (name, open) in &overlays {
                let (mut s2, _rx2) = (std::mem::replace(&mut state, test_state().0), ());
                s2.term_width = w;
                s2.term_height = h;
                open(&mut s2);
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| render_frame(f, &mut s2))
                    .unwrap_or_else(|e| panic!("{name} at {w}x{h}: {e}"));
                // Reset overlays for the next case.
                s2.workers.is_open = false;
                s2.peek = None;
                s2.run_view.is_open = false;
                s2.attention_state.is_open = false;
                s2.brain_state.is_open = false;
                s2.goal_prompt = None;
                s2.reply_prompt = None;
                s2.tab_tray.is_open = false;
                s2.help_open = false;
                state = s2;
            }
        }
    }
}
