//! Pure scheduler: decides what to do next for a run without side effects.

use std::collections::HashMap;

use crate::model::{Run, RunState, TaskState};

/// Concurrency and budget limits applied by [`next_actions`].
#[derive(Debug, Clone, Default)]
pub struct Limits {
    /// Maximum tasks in `working`/`needs_input` at once.
    pub max_workers: usize,
    /// Per-profile concurrency caps (profiles absent are uncapped).
    pub max_concurrent_by_profile: HashMap<String, usize>,
    /// Budget in USD; 0 means unlimited.
    pub budget_usd: f64,
}

/// An action the caller should carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerAction {
    /// Start a worker for this task (task id).
    Spawn {
        /// Task id to spawn.
        task: String,
    },
    /// Budget exhausted: pause the run.
    PauseForBudget,
    /// Every task is settled: mark the run done.
    MarkRunDone,
    /// Nothing is running and tasks await review: enter `reviewing`.
    EnterReviewing,
}

/// Computes the next scheduler actions for `run` under `limits`.
///
/// Returns nothing unless the run is `running`. A task is spawned when it is
/// `queued`, every dependency is `merged` (or `approved` with `parallel_ok`),
/// and the global and per-profile caps allow it.
pub fn next_actions(run: &Run, limits: &Limits) -> Vec<SchedulerAction> {
    if run.state != RunState::Running {
        return Vec::new();
    }
    if limits.budget_usd > 0.0 && run.spent_usd >= limits.budget_usd {
        return vec![SchedulerAction::PauseForBudget];
    }
    if !run.tasks.is_empty() && run.tasks.iter().all(|t| t.state.is_settled()) {
        return vec![SchedulerAction::MarkRunDone];
    }

    let mut active = 0usize;
    let mut by_profile: HashMap<&str, usize> = HashMap::new();
    for t in run.tasks.iter().filter(|t| t.state.is_active()) {
        active += 1;
        if let Some(p) = t.profile.as_deref() {
            *by_profile.entry(p).or_default() += 1;
        }
    }

    let mut actions = Vec::new();
    for t in run.tasks.iter().filter(|t| t.state == TaskState::Queued) {
        if active >= limits.max_workers {
            break;
        }
        let deps_ready = t.depends_on.iter().all(|d| match run.task(d) {
            Some(dep) => {
                dep.state == TaskState::Merged
                    || (dep.parallel_ok && dep.state == TaskState::Approved)
            }
            None => false,
        });
        if !deps_ready {
            continue;
        }
        if let Some(p) = t.profile.as_deref() {
            if let Some(cap) = limits.max_concurrent_by_profile.get(p) {
                if by_profile.get(p).copied().unwrap_or(0) >= *cap {
                    continue;
                }
            }
            *by_profile.entry(p).or_default() += 1;
        }
        active += 1;
        actions.push(SchedulerAction::Spawn { task: t.id.clone() });
    }

    if actions.is_empty() && active == 0 && run.tasks.iter().any(|t| t.state == TaskState::Review) {
        actions.push(SchedulerAction::EnterReviewing);
    }
    actions
}
