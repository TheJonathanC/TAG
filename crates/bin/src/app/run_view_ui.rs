//! Run view overlay (`Leader+O`): app-side data and keys for `tag_ui::run_view`.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::json;
use std::path::Path;
use tag_core::pane::PaneId;
use tag_orchestrator::RunState;
use tag_ui::{RunTaskRow, RunViewData};

/// Active runs, newest first.
fn run_ids(state: &AppState) -> Vec<String> {
    let mut runs: Vec<_> = state.runs.values().map(|a| &a.run).collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    runs.into_iter().map(|r| r.id.clone()).collect()
}

/// The (run id, task id, pane, worktree) under the selection.
fn selected(state: &AppState) -> Option<(String, String, Option<PaneId>, Option<String>)> {
    let ids = run_ids(state);
    let run_id = ids.get(state.run_view.run_index.min(ids.len().saturating_sub(1)))?;
    let run = &state.runs.get(run_id)?.run;
    let t = run.tasks.get(state.run_view.selected)?;
    Some((
        run_id.clone(),
        t.id.clone(),
        t.pane.map(|p| PaneId(p as u32)),
        t.worktree.clone(),
    ))
}

fn elapsed(since: chrono::DateTime<chrono::Utc>) -> String {
    let s = (chrono::Utc::now() - since).num_seconds().max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Build what the run view shows this frame.
pub(crate) fn build_run_view_data(state: &mut AppState) -> RunViewData {
    let ids = run_ids(state);
    if ids.is_empty() {
        return RunViewData {
            header: "No active runs (Leader+o starts one)".into(),
            ..Default::default()
        };
    }
    state.run_view.run_index = state.run_view.run_index.min(ids.len() - 1);
    let run = state.runs[&ids[state.run_view.run_index]].run.clone();
    state.run_view.selected = state
        .run_view
        .selected
        .min(run.tasks.len().saturating_sub(1));
    let budget = if run.budget_usd > 0.0 {
        format!("${:.2} / ${:.2}", run.spent_usd, run.budget_usd)
    } else {
        format!("${:.2}", run.spent_usd)
    };
    let header = format!(
        "Run {}: {} [{}] {} ({}/{})",
        run.short_id(),
        truncate_goal(&run.goal),
        serde_json::to_value(run.state)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
        budget,
        state.run_view.run_index + 1,
        ids.len()
    );
    let tasks: Vec<RunTaskRow> = run
        .tasks
        .iter()
        .map(|t| RunTaskRow {
            id: t.id.clone(),
            title: t.title.clone(),
            profile: t.profile.clone().unwrap_or_default(),
            state: serde_json::to_value(t.state)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            elapsed: t
                .pane
                .and_then(|p| state.status_since.get(&PaneId(p as u32)).copied())
                .map(elapsed)
                .unwrap_or_default(),
            has_pane: t
                .pane
                .is_some_and(|p| state.panes.contains_key(&PaneId(p as u32))),
        })
        .collect();
    let mut data = RunViewData {
        header,
        tasks,
        ..Default::default()
    };
    if let Some((_, _, pane, wt)) = selected(state) {
        data.tail = pane
            .and_then(|p| state.panes.get_mut(&p))
            .and_then(|p| p.read_text(60))
            .unwrap_or_else(|| "(no live pane)".into());
        data.bottom = match (&wt, state.run_view.show_tests) {
            (None, _) => "(no worktree yet)".into(),
            (Some(w), true) => match state.test_results.get(&canonical(Path::new(w))) {
                Some(r) => format!(
                    "exit {} at {}{}\n{}",
                    r.exit_code,
                    &r.commit[..r.commit.len().min(8)],
                    if r.dirty { " (dirty tree)" } else { "" },
                    r.tail
                ),
                None => "(no test run yet; press t)".into(),
            },
            (Some(w), false) => state
                .diffstat_cache
                .get(w)
                .cloned()
                .unwrap_or_else(|| "(press d to load the diff stat)".into()),
        };
    } else if run.state == RunState::Planning || run.state == RunState::AwaitingApproval {
        data.tail =
            "The conductor is planning. Press [a] to approve the plan once tasks appear.".into();
    }
    data
}

fn truncate_goal(g: &str) -> String {
    if g.chars().count() > 40 {
        g.chars().take(39).collect::<String>() + "…"
    } else {
        g.to_string()
    }
}

/// Load the diff stat of a worktree in the background into `diffstat_cache`.
fn request_diffstat(state: &mut AppState, worktree: String) {
    let Ok(repo) = repo_for(state, None) else {
        return;
    };
    let tx = state.event_tx.clone();
    std::thread::spawn(move || {
        let stat = diff_against_base(&repo, Path::new(&worktree), 0, true)
            .map(|(s, _, _)| {
                if s.trim().is_empty() {
                    "(no changes)".into()
                } else {
                    s
                }
            })
            .unwrap_or_else(|e| format!("(diff failed: {})", e.message));
        let (r, _rx) = tag_control::ReplyHandle::new();
        continue_in_loop(
            &tx,
            internal_ctx(),
            "_internal.diffstat",
            json!({"worktree": worktree, "stat": stat}),
            r,
        );
    });
}

pub(crate) fn action_toggle_run_view(state: &mut AppState) {
    state.run_view.is_open = !state.run_view.is_open;
    state.run_view_confirm_kill = false;
    if state.run_view.is_open {
        if let Some((_, _, _, Some(w))) = selected(state) {
            if !state.diffstat_cache.contains_key(&w) {
                request_diffstat(state, w);
            }
        }
    }
}

/// Handle a key while the run view is open.
pub(crate) fn handle_run_view_key(state: &mut AppState, key: KeyEvent) {
    if state.run_view_confirm_kill {
        state.run_view_confirm_kill = false;
        if key.code == KeyCode::Char('y') {
            if let Some((_, _, Some(p), _)) = selected(state) {
                if state.panes.contains_key(&p) {
                    close_pane(state, p);
                }
            }
        }
        return;
    }
    let sel = selected(state);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => state.run_view.is_open = false,
        KeyCode::Up => state.run_view.selected = state.run_view.selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => state.run_view.selected += 1,
        KeyCode::Char('[') => state.run_view.run_index = state.run_view.run_index.saturating_sub(1),
        KeyCode::Char(']') => state.run_view.run_index += 1,
        KeyCode::Tab => state.run_view.show_tests = !state.run_view.show_tests,
        KeyCode::Char('d') => {
            if let Some((_, _, _, Some(w))) = sel {
                state.diffstat_cache.remove(&w);
                request_diffstat(state, w);
            }
        }
        KeyCode::Char('t') => {
            if let Some((run, task, _, Some(_))) = sel {
                state.run_view.show_tests = true;
                ui_call(
                    state,
                    "tests.run",
                    json!({"run": run, "task": task}),
                    &format!("Tests for {task}"),
                );
            }
        }
        KeyCode::Char('m') => {
            if let Some((run, task, _, _)) = sel {
                ui_call(
                    state,
                    "worktree.merge",
                    json!({"run": run, "task": task}),
                    &format!("Merge {task}"),
                );
            }
        }
        KeyCode::Char('a') => {
            if let Some(run) = run_ids(state).get(state.run_view.run_index).cloned() {
                ui_call(
                    state,
                    "run.approve_plan",
                    json!({"run": run}),
                    &format!("Approve {run}"),
                );
            }
        }
        KeyCode::Char('r') => {
            if let Some((_, _, Some(p), _)) = sel {
                state.reply_prompt = Some((p, String::new()));
            }
        }
        KeyCode::Char('e') => {
            if let Some((_, _, Some(p), _)) = sel {
                open_peek(state, p);
            }
        }
        KeyCode::Enter => {
            if let Some((_, _, Some(p), _)) = sel {
                promote_pane(state, p);
                state.run_view.is_open = false;
            }
        }
        KeyCode::Char('k') => {
            if matches!(sel, Some((_, _, Some(_), _))) {
                state.run_view_confirm_kill = true;
                push_toast(
                    state,
                    "Kill worker?",
                    "Press y to kill the selected task's pane",
                    tag_ui::ToastLevel::Warning,
                    3,
                );
            }
        }
        _ => {}
    }
    // Load the diff stat for a newly selected task.
    if state.run_view.is_open && !state.run_view.show_tests {
        if let Some((_, _, _, Some(w))) = selected(state) {
            if !state.diffstat_cache.contains_key(&w) {
                state.diffstat_cache.insert(w.clone(), "(loading…)".into());
                request_diffstat(state, w);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::tests::{test_state, MockAgentPane};
    use tag_orchestrator::{OrchestrationSettings, Run, RunStore, Task, TaskState};

    fn add_run(state: &mut AppState, id: &str) {
        let mut run = Run::new(id, "p", "add oauth", chrono::Utc::now());
        run.state = RunState::Running;
        let mut t1 = Task::new("t1", "api", "x");
        t1.state = TaskState::Working;
        t1.pane = Some(9);
        run.tasks.push(t1);
        run.tasks.push(Task::new("t2", "ui", "y"));
        state.runs.insert(
            id.into(),
            ActiveRun {
                store: RunStore::new(state.paths.state_dir.join("runs")),
                run,
                repo: state.paths.state_dir.clone(),
                settings: OrchestrationSettings::builtin(),
                announced_all_review: false,
            },
        );
    }

    #[test]
    fn data_and_keys() {
        let (mut state, _rx) = test_state();
        let empty = build_run_view_data(&mut state);
        assert!(empty.header.contains("No active runs"));
        add_run(&mut state, "r_20261123_0915_ab12");
        state.panes.insert(
            PaneId(9),
            Box::new(MockAgentPane {
                id: PaneId(9),
                status: "running",
            }),
        );
        action_toggle_run_view(&mut state);
        assert!(state.run_view.is_open);
        let d = build_run_view_data(&mut state);
        assert!(d.header.contains("r_ab12"), "{}", d.header);
        assert_eq!(d.tasks.len(), 2);
        assert!(d.tasks[0].has_pane);
        assert_eq!(d.tasks[0].state, "working");
        assert!(d.bottom.contains("no worktree"));

        let key = |c| KeyEvent::new(c, crossterm::event::KeyModifiers::NONE);
        handle_run_view_key(&mut state, key(KeyCode::Down));
        handle_run_view_key(&mut state, key(KeyCode::Down));
        build_run_view_data(&mut state);
        assert_eq!(state.run_view.selected, 1, "selection is clamped");
        handle_run_view_key(&mut state, key(KeyCode::Up));
        handle_run_view_key(&mut state, key(KeyCode::Char('k')));
        assert!(state.run_view_confirm_kill);
        handle_run_view_key(&mut state, key(KeyCode::Char('n')));
        assert!(state.panes.contains_key(&PaneId(9)), "n cancels");
        handle_run_view_key(&mut state, key(KeyCode::Tab));
        assert!(state.run_view.show_tests);
        handle_run_view_key(&mut state, key(KeyCode::Esc));
        assert!(!state.run_view.is_open);
    }
}
