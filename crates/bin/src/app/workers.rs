//! Orchestration UI glue: card strip, peek overlay and the workers table.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::time::{Duration, Instant};
use tag_core::pane::PaneId;
use tag_orchestrator::Visibility;
use tag_ui::{
    format_elapsed, inside_workers, visible_rows, worker_row_at, CardItem, ToastLevel, WorkerRow,
};

/// Peek overlay state: which pane is shown and whether keys are forwarded to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PeekState {
    pub(crate) pane: PaneId,
    pub(crate) input: bool,
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

fn since_status(state: &AppState, pid: PaneId) -> String {
    state
        .status_since
        .get(&pid)
        .map(|t| format_elapsed((chrono::Utc::now() - *t).num_seconds().max(0) as u64))
        .unwrap_or_default()
}

fn short(s: &str, n: usize) -> String {
    let mut o: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        o.pop();
        o.push('…');
    }
    o
}

/// Cards of the active workspace (its minimized panes).
pub(crate) fn build_card_items(state: &AppState) -> Vec<CardItem> {
    state
        .layout_mgr
        .active_workspace()
        .minimized_panes
        .iter()
        .filter_map(|pid| {
            let pane = state.panes.get(pid)?;
            let label = state
                .pane_meta
                .get(pid)
                .and_then(|m| m.task.clone())
                .unwrap_or_else(|| short(&pane.title(), 12));
            Some(CardItem {
                pane_id: *pid,
                status: pane.agent_status().unwrap_or("").to_string(),
                label,
                profile: pane.agent_profile(),
                elapsed: since_status(state, *pid),
            })
        })
        .collect()
}

/// Rows reserved for the card strip (0 or 1).
pub(crate) fn card_strip_height(state: &AppState) -> u16 {
    let has = state
        .layout_mgr
        .active_workspace()
        .minimized_panes
        .iter()
        .any(|p| state.panes.contains_key(p));
    u16::from(has && state.term_height > 6)
}

/// Screen rectangle of the card strip (just above the status bar).
pub(crate) fn card_strip_area(state: &AppState) -> Option<Rect> {
    (card_strip_height(state) > 0)
        .then(|| Rect::new(0, state.term_height.saturating_sub(2), state.term_width, 1))
}

fn task_state_label(state: &AppState, pid: PaneId) -> Option<String> {
    let meta = state.pane_meta.get(&pid)?;
    let run = state.runs.get(meta.run.as_ref()?)?;
    let task = run.run.tasks.iter().find(|t| {
        meta.task.as_deref() == Some(t.id.as_str()) || t.pane == Some(u64::from(pid.0))
    })?;
    serde_json::to_value(task.state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

/// One row per agent pane or run-bound pane. `with_output` reads each pane's last line.
pub(crate) fn build_worker_rows(state: &mut AppState, with_output: bool) -> Vec<WorkerRow> {
    let mut ids: Vec<PaneId> = state
        .panes
        .iter()
        .filter(|(pid, p)| {
            p.agent_status().is_some() || state.pane_meta.get(pid).is_some_and(|m| m.run.is_some())
        })
        .map(|(pid, _)| *pid)
        .collect();
    ids.sort_by_key(|p| p.0);
    let mut rows = Vec::new();
    for pid in ids {
        let task_state = task_state_label(state, pid);
        let elapsed = since_status(state, pid);
        let meta = state.pane_meta.get(&pid);
        let run_task = match meta {
            Some(m) => match (&m.task, &m.run) {
                (Some(t), Some(r)) => format!("{t}@{r}"),
                (Some(t), None) => t.clone(),
                (None, Some(r)) => r.clone(),
                _ => String::new(),
            },
            None => String::new(),
        };
        let Some(pane) = state.panes.get_mut(&pid) else {
            continue;
        };
        let status = pane.agent_status().unwrap_or("").to_string();
        let last_line = if with_output {
            pane.read_text(4)
                .and_then(|t| {
                    t.lines()
                        .rev()
                        .map(str::trim)
                        .find(|l| !l.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        rows.push(WorkerRow {
            pane_id: pid,
            name: pane.title(),
            profile: pane.agent_profile().unwrap_or_default(),
            run_task,
            state: task_state.unwrap_or_else(|| status.clone()),
            status,
            elapsed,
            last_line,
        });
    }
    rows
}

/// Move a pane into the active workspace's layout, focus it and close the overlays.
pub(crate) fn promote_pane(state: &mut AppState, pid: PaneId) {
    if !state.panes.contains_key(&pid) {
        return;
    }
    if pane_visibility(state, pid) != Visibility::Pane {
        place_pane(state, pid, Visibility::Pane, None);
    }
    state.peek = None;
    state.workers.is_open = false;
    focus_pane_anywhere(state, pid);
    state.needs_clear = true;
    save_session_to_disk(state);
}

pub(crate) fn open_peek(state: &mut AppState, pid: PaneId) {
    if state.panes.contains_key(&pid) {
        state.peek = Some(PeekState {
            pane: pid,
            input: false,
        });
        state.needs_clear = true;
    }
}

/// Close the peek overlay, shrinking a non-tiled pane back to its hidden size.
pub(crate) fn close_peek(state: &mut AppState) {
    if let Some(pk) = state.peek.take() {
        if pane_visibility(state, pk.pane) != Visibility::Pane {
            if let Some(p) = state.panes.get_mut(&pk.pane) {
                p.on_resize(HIDDEN_COLS, HIDDEN_ROWS);
            }
        }
        state.needs_clear = true;
    }
}

/// Pane to peek: first card, else first non-tiled agent needing input, else first hidden worker.
pub(crate) fn peek_target(state: &AppState) -> Option<PaneId> {
    if let Some(p) = state
        .layout_mgr
        .active_workspace()
        .minimized_panes
        .iter()
        .find(|p| state.panes.contains_key(p))
    {
        return Some(*p);
    }
    let mut ids: Vec<PaneId> = state.panes.keys().copied().collect();
    ids.sort_by_key(|p| p.0);
    let non_tiled = |p: &PaneId| pane_visibility(state, *p) != Visibility::Pane;
    ids.iter()
        .copied()
        .find(|p| {
            non_tiled(p) && state.panes[p].agent_status().map(normalize_status_str) == Some(true)
        })
        .or_else(|| {
            ids.iter().copied().find(|p| {
                pane_visibility(state, *p) == Visibility::Hidden
                    && state.panes[p].agent_status().is_some()
            })
        })
}

fn normalize_status_str(s: &str) -> bool {
    s == "awaiting-input" || s == "awaiting_input"
}

pub(crate) fn action_peek(state: &mut AppState) {
    match peek_target(state) {
        Some(pid) => open_peek(state, pid),
        None => push_toast(
            state,
            "Peek",
            "No card or worker to peek",
            ToastLevel::Info,
            3,
        ),
    }
}

pub(crate) fn action_toggle_workers(state: &mut AppState) {
    if state.workers.is_open {
        state.workers.is_open = false;
    } else {
        state.workers.open();
    }
    state.needs_clear = true;
}

/// Key handling while the peek overlay is open. Returns true if the peek consumed the key.
pub(crate) fn handle_peek_key(state: &mut AppState, key: &KeyEvent) -> bool {
    let Some(pk) = state.peek else {
        return false;
    };
    if key.code == KeyCode::Esc {
        close_peek(state);
        return true;
    }
    if pk.input {
        if key.code == KeyCode::Char('i') && key.modifiers.contains(KeyModifiers::ALT) {
            state.peek = Some(PeekState { input: false, ..pk });
        } else if let Some(pane) = state.panes.get_mut(&pk.pane) {
            let bytes = key_event_to_bytes(*key, pane.application_cursor());
            if !bytes.is_empty() {
                pane.send_input(&bytes);
            }
        }
        return true;
    }
    match key.code {
        KeyCode::Char('i') => state.peek = Some(PeekState { input: true, ..pk }),
        KeyCode::Enter => promote_pane(state, pk.pane),
        _ => {}
    }
    true
}

/// Key handling for the workers table (caller checks `state.workers.is_open`).
pub(crate) fn handle_workers_key(state: &mut AppState, key: &KeyEvent) {
    // Inline prompts first.
    if let Some((pid, name)) = state.workers.confirm_kill.clone() {
        state.workers.confirm_kill = None;
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            close_pane(state, pid);
            state.pane_meta.remove(&pid);
            state.pending_prompts.remove(&pid);
            state.last_output.remove(&pid);
            push_toast(
                state,
                "Workers",
                format!("Killed {name}"),
                ToastLevel::Info,
                3,
            );
        }
        return;
    }
    if let Some((pid, name, mut text)) = state.workers.reply.take() {
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                if let Some(pane) = state.panes.get_mut(&pid) {
                    pane_write(pane.as_mut(), &text, true);
                    push_toast(
                        state,
                        "Workers",
                        format!("Sent reply to {name}"),
                        ToastLevel::Info,
                        3,
                    );
                }
            }
            KeyCode::Backspace => {
                text.pop();
                state.workers.reply = Some((pid, name, text));
            }
            _ => {
                if let Some(c) = text_input_char(key) {
                    text.push(c);
                }
                state.workers.reply = Some((pid, name, text));
            }
        }
        return;
    }
    if state.workers.is_filtering {
        match key.code {
            KeyCode::Esc => {
                state.workers.is_filtering = false;
                state.workers.filter_text.clear();
            }
            KeyCode::Enter => state.workers.is_filtering = false,
            KeyCode::Backspace => {
                state.workers.filter_text.pop();
            }
            _ => {
                if let Some(c) = text_input_char(key) {
                    state.workers.filter_text.push(c);
                }
            }
        }
        return;
    }

    let rows = build_worker_rows(state, false);
    let vis = visible_rows(&rows, state.workers.filter, &state.workers.filter_text);
    state.workers.sync(&vis);
    let sel = state
        .workers
        .selected_row(&vis)
        .map(|r| (r.pane_id, r.name.clone()));
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => state.workers.is_open = false,
        KeyCode::Up => state.workers.move_by(-1, &vis),
        KeyCode::Down | KeyCode::Char('j') => state.workers.move_by(1, &vis),
        KeyCode::Char('/') => state.workers.is_filtering = true,
        KeyCode::Char('f') => {
            state.workers.filter = state.workers.filter.next();
            state.workers.selected = 0;
            state.workers.selected_pane = None;
        }
        KeyCode::Enter => {
            if let Some((pid, _)) = sel {
                promote_pane(state, pid);
            }
        }
        KeyCode::Char('e') => {
            if let Some((pid, _)) = sel {
                open_peek(state, pid);
            }
        }
        KeyCode::Char('h') => {
            if let Some((pid, _)) = sel {
                place_pane(state, pid, Visibility::Hidden, None);
                state.needs_clear = true;
                save_session_to_disk(state);
            }
        }
        KeyCode::Char('k') => state.workers.confirm_kill = sel,
        KeyCode::Char('r') => {
            state.workers.reply = sel.map(|(p, n)| (p, n, String::new()));
        }
        _ => {}
    }
}

/// Mouse handling while the workers table or peek overlay is open (always consumes the event).
pub(crate) fn handle_overlay_mouse(state: &mut AppState, mouse: &MouseEvent) {
    if state.peek.is_some() || !state.workers.is_open {
        return;
    }
    let screen = Rect::new(0, 0, state.term_width, state.term_height);
    let rows = build_worker_rows(state, false);
    let vis = visible_rows(&rows, state.workers.filter, &state.workers.filter_text);
    state.workers.sync(&vis);
    match mouse.kind {
        MouseEventKind::ScrollDown => state.workers.move_by(1, &vis),
        MouseEventKind::ScrollUp => state.workers.move_by(-1, &vis),
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(idx) =
                worker_row_at(screen, &state.workers, vis.len(), mouse.column, mouse.row)
            {
                let pid = vis[idx].pane_id;
                let double = state
                    .workers
                    .last_click
                    .is_some_and(|(t, p)| p == pid && t.elapsed() < DOUBLE_CLICK);
                state.workers.select_index(idx, &vis);
                if double {
                    state.workers.last_click = None;
                    promote_pane(state, pid);
                } else {
                    state.workers.last_click = Some((Instant::now(), pid));
                }
            } else if !inside_workers(screen, mouse.column, mouse.row) {
                state.workers.is_open = false;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::tests::{test_state, MockAgentPane};
    use crossterm::event::KeyEventKind;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(c, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn add(state: &mut AppState, id: u32, status: &'static str) -> PaneId {
        let pid = PaneId(id);
        state
            .panes
            .insert(pid, Box::new(MockAgentPane { id: pid, status }));
        pid
    }

    #[test]
    fn filter_cycle_selection_and_kill_confirm() {
        let (mut state, _rx) = test_state();
        let a = add(&mut state, 1, "running");
        let b = add(&mut state, 2, "awaiting-input");
        place_pane(&mut state, a, Visibility::Hidden, None);
        place_pane(&mut state, b, Visibility::Hidden, None);
        state.workers.open();

        // needs-input sorts first
        handle_workers_key(&mut state, &key(KeyCode::Down));
        assert_eq!(state.workers.selected_pane, Some(a));
        for _ in 0..5 {
            handle_workers_key(&mut state, &key(KeyCode::Down));
        }
        assert_eq!(state.workers.selected, 1);
        handle_workers_key(&mut state, &key(KeyCode::Up));
        handle_workers_key(&mut state, &key(KeyCode::Up));
        handle_workers_key(&mut state, &key(KeyCode::Up));
        assert_eq!(state.workers.selected_pane, Some(b));

        // filter cycling
        handle_workers_key(&mut state, &key(KeyCode::Char('f')));
        assert_eq!(state.workers.filter, tag_ui::WorkersFilter::NeedsMe);
        for _ in 0..3 {
            handle_workers_key(&mut state, &key(KeyCode::Char('f')));
        }
        assert_eq!(state.workers.filter, tag_ui::WorkersFilter::All);

        // kill: n cancels, y kills
        handle_workers_key(&mut state, &key(KeyCode::Char('k')));
        assert_eq!(state.workers.confirm_kill.as_ref().map(|c| c.0), Some(b));
        handle_workers_key(&mut state, &key(KeyCode::Char('n')));
        assert!(state.panes.contains_key(&b));
        handle_workers_key(&mut state, &key(KeyCode::Char('k')));
        handle_workers_key(&mut state, &key(KeyCode::Char('y')));
        assert!(!state.panes.contains_key(&b));
        assert!(state.panes.contains_key(&a));
    }

    #[test]
    fn enter_promotes_and_esc_closes() {
        let (mut state, _rx) = test_state();
        let a = add(&mut state, 1, "running");
        place_pane(&mut state, a, Visibility::Card, None);
        assert_eq!(build_card_items(&state).len(), 1);
        assert_eq!(card_strip_height(&state), 1);
        state.workers.open();
        handle_workers_key(&mut state, &key(KeyCode::Enter));
        assert!(!state.workers.is_open);
        assert_eq!(pane_visibility(&state, a), Visibility::Pane);
        assert_eq!(card_strip_height(&state), 0);

        open_peek(&mut state, a);
        assert!(handle_peek_key(&mut state, &key(KeyCode::Char('i'))));
        assert!(state.peek.unwrap().input);
        assert!(handle_peek_key(&mut state, &key(KeyCode::Esc)));
        assert!(state.peek.is_none());
    }
}
