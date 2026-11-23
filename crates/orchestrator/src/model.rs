//! Run and task models plus the state machine.
//!
//! # Run state transitions
//!
//! | from              | allowed next                                   |
//! |-------------------|------------------------------------------------|
//! | `planning`        | `awaiting_approval`, `running`, `failed`, `cancelled` |
//! | `awaiting_approval` | `running`, `planning`, `failed`, `cancelled` |
//! | `running`         | `reviewing`, `merging`, `done`, `paused`, `failed`, `cancelled` |
//! | `reviewing`       | `running`, `merging`, `done`, `paused`, `failed`, `cancelled` |
//! | `merging`         | `running`, `reviewing`, `done`, `paused`, `failed`, `cancelled` |
//! | `paused`          | `running`, the state it was paused from, `failed`, `cancelled` |
//! | `done`, `failed`, `cancelled` | nothing (terminal)              |
//!
//! [`RunState::can_transition_to`] is the static table (for `paused` it
//! allows `running`, `reviewing` and `merging`); [`Run::transition`]
//! additionally restricts `paused` to `running` or the recorded previous
//! state.
//!
//! # Task state transitions
//!
//! | from               | allowed next                          |
//! |--------------------|---------------------------------------|
//! | `planned`          | `queued`, `skipped`, `failed`         |
//! | `queued`           | `working`, `skipped`, `failed`        |
//! | `working`          | `needs_input`, `review`, `failed`, `skipped` |
//! | `needs_input`      | `working`, `failed`, `skipped`        |
//! | `review`           | `approved`, `changes_requested`, `failed`, `skipped` |
//! | `changes_requested`| `working`, `failed`, `skipped`        |
//! | `approved`         | `merged`, `changes_requested`, `failed`, `skipped` |
//! | `failed`           | `queued` (retry), `skipped`           |
//! | `merged`, `skipped`| nothing (terminal)                    |

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::error::OrchestratorError;
use crate::Result;

/// Lifecycle state of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// The conductor is drafting the plan.
    Planning,
    /// The plan awaits human approval.
    AwaitingApproval,
    /// Workers are being scheduled and are working.
    Running,
    /// Work is being reviewed.
    Reviewing,
    /// Approved work is being merged.
    Merging,
    /// All work finished.
    Done,
    /// Paused (budget hit or user request).
    Paused,
    /// The run failed.
    Failed,
    /// The run was cancelled.
    Cancelled,
}

impl RunState {
    /// All variants, for exhaustive iteration.
    pub const ALL: [RunState; 9] = [
        RunState::Planning,
        RunState::AwaitingApproval,
        RunState::Running,
        RunState::Reviewing,
        RunState::Merging,
        RunState::Done,
        RunState::Paused,
        RunState::Failed,
        RunState::Cancelled,
    ];

    /// True for `done`, `failed` and `cancelled`.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Done | RunState::Failed | RunState::Cancelled
        )
    }

    /// Static transition table; see the module docs.
    pub fn can_transition_to(&self, next: RunState) -> bool {
        use RunState::*;
        if self.is_terminal() {
            return false;
        }
        if next == *self {
            return false;
        }
        if matches!(next, Failed | Cancelled) {
            return true;
        }
        match self {
            Planning => matches!(next, AwaitingApproval | Running),
            AwaitingApproval => matches!(next, Running | Planning),
            Running => matches!(next, Reviewing | Merging | Done | Paused),
            Reviewing => matches!(next, Running | Merging | Done | Paused),
            Merging => matches!(next, Running | Reviewing | Done | Paused),
            Paused => matches!(next, Running | Reviewing | Merging),
            Done | Failed | Cancelled => false,
        }
    }
}

/// Lifecycle state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Defined but not yet approved for scheduling.
    Planned,
    /// Waiting for dependencies and capacity.
    Queued,
    /// A worker is running it.
    Working,
    /// The worker is blocked on input.
    NeedsInput,
    /// Awaiting review.
    Review,
    /// Review asked for changes.
    ChangesRequested,
    /// Approved, awaiting merge.
    Approved,
    /// Merged.
    Merged,
    /// Failed.
    Failed,
    /// Skipped.
    Skipped,
}

impl TaskState {
    /// All variants, for exhaustive iteration.
    pub const ALL: [TaskState; 10] = [
        TaskState::Planned,
        TaskState::Queued,
        TaskState::Working,
        TaskState::NeedsInput,
        TaskState::Review,
        TaskState::ChangesRequested,
        TaskState::Approved,
        TaskState::Merged,
        TaskState::Failed,
        TaskState::Skipped,
    ];

    /// True for `merged` and `skipped`.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Merged | TaskState::Skipped)
    }

    /// True for states that count as "finished" for run completion
    /// (`merged`, `skipped`, `failed`).
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            TaskState::Merged | TaskState::Skipped | TaskState::Failed
        )
    }

    /// True while a worker occupies a slot (`working`, `needs_input`).
    pub fn is_active(self) -> bool {
        matches!(self, TaskState::Working | TaskState::NeedsInput)
    }

    /// Static transition table; see the module docs.
    pub fn can_transition_to(&self, next: TaskState) -> bool {
        use TaskState::*;
        if self.is_terminal() || next == *self {
            return false;
        }
        if matches!(next, Failed | Skipped) {
            return true;
        }
        match self {
            Planned => matches!(next, Queued),
            Queued => matches!(next, Working),
            Working => matches!(next, NeedsInput | Review),
            NeedsInput => matches!(next, Working),
            Review => matches!(next, Approved | ChangesRequested),
            ChangesRequested => matches!(next, Working),
            Approved => matches!(next, Merged | ChangesRequested),
            Failed => matches!(next, Queued),
            Merged | Skipped => false,
        }
    }
}

/// How finished work is merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergePolicy {
    /// Ask the human before merging.
    Ask,
    /// Merge automatically.
    Auto,
}

/// How visible a worker is in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// Not shown.
    Hidden,
    /// Shown as a card.
    Card,
    /// Shown as a full pane.
    Pane,
}

/// Reference to the conductor agent of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConductorRef {
    /// Agent profile name.
    pub profile: String,
    /// Pane id, once spawned.
    #[serde(default)]
    pub pane: Option<u64>,
}

/// Result of the last test run for a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestResult {
    /// Commit that was tested.
    pub commit: String,
    /// Test command exit code.
    pub exit_code: i32,
    /// When the test ran.
    pub at: DateTime<Utc>,
}

/// Outcome claimed in a worker report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// Work completed.
    Done,
    /// Worker is blocked.
    Blocked,
}

/// A worker's final report for a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskReport {
    /// Summary of the work.
    pub summary: String,
    /// Done or blocked.
    pub status: ReportStatus,
    /// Optional extra notes.
    #[serde(default)]
    pub notes: Option<String>,
    /// When reported.
    pub at: DateTime<Utc>,
}

/// One unit of work in a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Task id, unique within the run (e.g. `t1`).
    pub id: String,
    /// Short title.
    pub title: String,
    /// Full worker instructions.
    #[serde(default)]
    pub prompt: String,
    /// Acceptance criteria.
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// Ids of tasks that must finish first.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Dependents may start once this task is merely approved.
    #[serde(default)]
    pub parallel_ok: bool,
    /// Worker profile override.
    #[serde(default)]
    pub profile: Option<String>,
    /// Current state.
    pub state: TaskState,
    /// Worktree path relative to the repo.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Branch name.
    #[serde(default)]
    pub branch: Option<String>,
    /// Worker pane id.
    #[serde(default)]
    pub pane: Option<u64>,
    /// Number of times a worker was started.
    #[serde(default)]
    pub attempts: u32,
    /// Last test result.
    #[serde(default)]
    pub last_test: Option<TestResult>,
    /// Worker report.
    #[serde(default)]
    pub report: Option<TaskReport>,
    /// Why the task failed.
    #[serde(default)]
    pub failure_reason: Option<String>,
}

impl Task {
    /// Creates a `planned` task with empty optional fields.
    pub fn new(id: impl Into<String>, title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Task {
            id: id.into(),
            title: title.into(),
            prompt: prompt.into(),
            acceptance: Vec::new(),
            depends_on: Vec::new(),
            parallel_ok: false,
            profile: None,
            state: TaskState::Planned,
            worktree: None,
            branch: None,
            pane: None,
            attempts: 0,
            last_test: None,
            report: None,
            failure_reason: None,
        }
    }
}

fn default_max_workers() -> usize {
    4
}

fn default_true() -> bool {
    true
}

fn default_merge_policy() -> MergePolicy {
    MergePolicy::Ask
}

fn default_planning() -> RunState {
    RunState::Planning
}

/// An orchestration run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Run id, see [`new_run_id`].
    pub id: String,
    /// Project id.
    pub project: String,
    /// The user's goal.
    pub goal: String,
    /// Current state.
    #[serde(default = "default_planning")]
    pub state: RunState,
    /// Conductor agent.
    #[serde(default)]
    pub conductor: Option<ConductorRef>,
    /// Merge policy.
    #[serde(default = "default_merge_policy")]
    pub merge_policy: MergePolicy,
    /// Budget in USD; 0 means unlimited.
    #[serde(default)]
    pub budget_usd: f64,
    /// Spend so far in USD.
    #[serde(default)]
    pub spent_usd: f64,
    /// Whether the plan needs human approval.
    #[serde(default = "default_true")]
    pub require_plan_approval: bool,
    /// Maximum concurrent workers.
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    /// Worker profiles to use.
    #[serde(default)]
    pub worker_profiles: Vec<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last modification time.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
    /// Tasks in plan order.
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Why the run is paused.
    #[serde(default)]
    pub pause_reason: Option<String>,
    /// Why the run failed.
    #[serde(default)]
    pub failure_reason: Option<String>,
    /// State the run was in before pausing.
    #[serde(default)]
    pub paused_from: Option<RunState>,
}

/// Generates a run id like `r_20261123_0915_ab12`.
pub fn new_run_id(now: DateTime<Utc>) -> String {
    let n: u16 = rand::thread_rng().gen();
    format!("r_{}_{:04x}", now.format("%Y%m%d_%H%M"), n)
}

/// Worktree directory name for a task: `.worktrees/<short>-<task>`.
pub fn task_worktree_name(run: &Run, task: &str) -> String {
    format!(".worktrees/{}-{}", run.short_id(), task)
}

/// Branch name for a task: `tag/<short>/<task>`.
pub fn task_branch(run: &Run, task: &str) -> String {
    format!("tag/{}/{}", run.short_id(), task)
}

impl Run {
    /// Creates a new run in `planning` with defaults.
    pub fn new(
        id: impl Into<String>,
        project: impl Into<String>,
        goal: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Run {
            id: id.into(),
            project: project.into(),
            goal: goal.into(),
            state: RunState::Planning,
            conductor: None,
            merge_policy: MergePolicy::Ask,
            budget_usd: 0.0,
            spent_usd: 0.0,
            require_plan_approval: true,
            max_workers: 4,
            worker_profiles: Vec::new(),
            created_at: now,
            updated_at: now,
            tasks: Vec::new(),
            pause_reason: None,
            failure_reason: None,
            paused_from: None,
        }
    }

    /// Short id `r_xxxx` (`r_` plus the last 4 characters of the id).
    pub fn short_id(&self) -> String {
        let chars: Vec<char> = self.id.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
        format!("r_{tail}")
    }

    /// Looks up a task by id.
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Looks up a task mutably by id.
    pub fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Transitions the run, updating `updated_at` and returning the event.
    pub fn transition(&mut self, next: RunState, reason: Option<String>) -> Result<RunEvent> {
        let from = self.state;
        let ok = from.can_transition_to(next)
            && !(from == RunState::Paused
                && matches!(next, RunState::Reviewing | RunState::Merging)
                && self.paused_from != Some(next));
        if !ok {
            return Err(OrchestratorError::InvalidRunTransition { from, to: next });
        }
        let now = Utc::now();
        self.state = next;
        self.updated_at = now;
        if next == RunState::Paused {
            self.paused_from = Some(from);
            self.pause_reason = reason.clone();
        } else if from == RunState::Paused {
            self.paused_from = None;
            self.pause_reason = None;
        }
        if next == RunState::Failed {
            self.failure_reason = reason.clone();
        }
        Ok(RunEvent {
            at: now,
            kind: RunEventKind::RunState {
                from,
                to: next,
                reason,
            },
        })
    }

    /// Transitions a task, updating `updated_at` and returning the event.
    pub fn transition_task(
        &mut self,
        task_id: &str,
        next: TaskState,
        reason: Option<String>,
    ) -> Result<RunEvent> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| OrchestratorError::UnknownTask(task_id.to_string()))?;
        let from = task.state;
        if !from.can_transition_to(next) {
            return Err(OrchestratorError::InvalidTaskTransition {
                task: task_id.to_string(),
                from,
                to: next,
            });
        }
        task.state = next;
        if next == TaskState::Working && from == TaskState::Queued {
            task.attempts += 1;
        }
        if next == TaskState::Failed {
            task.failure_reason = reason.clone();
        } else if next == TaskState::Queued {
            task.failure_reason = None;
        }
        let now = Utc::now();
        self.updated_at = now;
        Ok(RunEvent {
            at: now,
            kind: RunEventKind::TaskState {
                task: task_id.to_string(),
                from,
                to: next,
                reason,
            },
        })
    }

    /// Adds a task. Allowed in `planning`, `awaiting_approval` and
    /// `running`; rejects duplicate ids, unknown dependencies and cycles.
    pub fn add_task(&mut self, task: Task) -> Result<RunEvent> {
        if !matches!(
            self.state,
            RunState::Planning | RunState::AwaitingApproval | RunState::Running
        ) {
            return Err(OrchestratorError::RunNotEditable(self.state));
        }
        if self.task(&task.id).is_some() {
            return Err(OrchestratorError::DuplicateTask(task.id));
        }
        for dep in &task.depends_on {
            if *dep != task.id && self.task(dep).is_none() {
                return Err(OrchestratorError::UnknownDependency {
                    task: task.id.clone(),
                    dependency: dep.clone(),
                });
            }
        }
        // Build the graph including the new task and look for a cycle
        // reachable from it.
        let mut graph: HashMap<&str, &[String]> = self
            .tasks
            .iter()
            .map(|t| (t.id.as_str(), t.depends_on.as_slice()))
            .collect();
        graph.insert(task.id.as_str(), task.depends_on.as_slice());
        let mut visiting = HashSet::new();
        let mut done = HashSet::new();
        if has_cycle(task.id.as_str(), &graph, &mut visiting, &mut done) {
            return Err(OrchestratorError::DependencyCycle(task.id));
        }
        let id = task.id.clone();
        self.tasks.push(task);
        let now = Utc::now();
        self.updated_at = now;
        Ok(RunEvent {
            at: now,
            kind: RunEventKind::TaskAdded { task: id },
        })
    }

    /// Moves every `planned` task to `queued` (used on plan approval) and
    /// returns the events.
    pub fn queue_planned(&mut self) -> Vec<RunEvent> {
        let now = Utc::now();
        let mut events = Vec::new();
        for t in &mut self.tasks {
            if t.state == TaskState::Planned {
                t.state = TaskState::Queued;
                events.push(RunEvent {
                    at: now,
                    kind: RunEventKind::TaskState {
                        task: t.id.clone(),
                        from: TaskState::Planned,
                        to: TaskState::Queued,
                        reason: Some("plan approved".into()),
                    },
                });
            }
        }
        if !events.is_empty() {
            self.updated_at = now;
        }
        events
    }

    /// Adds spend. Returns true when this call newly pushed spend to or
    /// past a non-zero budget.
    pub fn add_cost(&mut self, usd: f64) -> bool {
        let was_over = self.budget_usd > 0.0 && self.spent_usd >= self.budget_usd;
        self.spent_usd += usd;
        self.updated_at = Utc::now();
        !was_over && self.budget_usd > 0.0 && self.spent_usd >= self.budget_usd
    }
}

fn has_cycle<'a>(
    node: &'a str,
    graph: &HashMap<&'a str, &'a [String]>,
    visiting: &mut HashSet<&'a str>,
    done: &mut HashSet<&'a str>,
) -> bool {
    if done.contains(node) {
        return false;
    }
    if !visiting.insert(node) {
        return true;
    }
    if let Some(deps) = graph.get(node) {
        for d in deps.iter() {
            if has_cycle(d.as_str(), graph, visiting, done) {
                return true;
            }
        }
    }
    visiting.remove(node);
    done.insert(node);
    false
}

/// A recorded state change, appended to `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    /// When it happened.
    pub at: DateTime<Utc>,
    /// What happened.
    #[serde(flatten)]
    pub kind: RunEventKind,
}

/// The payload of a [`RunEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventKind {
    /// Run state changed.
    RunState {
        /// Previous state.
        from: RunState,
        /// New state.
        to: RunState,
        /// Optional reason.
        #[serde(default)]
        reason: Option<String>,
    },
    /// Task state changed.
    TaskState {
        /// Task id.
        task: String,
        /// Previous state.
        from: TaskState,
        /// New state.
        to: TaskState,
        /// Optional reason.
        #[serde(default)]
        reason: Option<String>,
    },
    /// A task was added.
    TaskAdded {
        /// Task id.
        task: String,
    },
    /// Free-form note.
    Note {
        /// Note text.
        text: String,
    },
}
