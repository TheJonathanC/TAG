//! `tag-orchestrator`: pure library holding the orchestration run/task
//! state machine, on-disk persistence, the dependency/concurrency scheduler
//! and project configuration resolution. No async runtime and no UI.

pub mod config;
pub mod error;
pub mod model;
pub mod scheduler;
pub mod store;

pub use config::{touches_protected, OrchestrationSettings, ProjectConfig};
pub use error::OrchestratorError;
pub use model::{
    new_run_id, task_branch, task_worktree_name, ConductorRef, MergePolicy, ReportStatus, Run,
    RunEvent, RunEventKind, RunState, Task, TaskReport, TaskState, TestResult, Visibility,
};
pub use scheduler::{next_actions, Limits, SchedulerAction};
pub use store::{ListResult, RunStore};

/// Convenience result alias for this crate.
pub type Result<T> = std::result::Result<T, OrchestratorError>;
