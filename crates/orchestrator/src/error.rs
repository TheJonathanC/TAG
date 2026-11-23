//! Error type for the orchestrator crate.

use crate::model::{RunState, TaskState};

/// Errors produced by run/task mutation, persistence and config loading.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    /// A run state transition that is not in the allowed table.
    #[error("invalid run transition {from:?} -> {to:?}")]
    InvalidRunTransition {
        /// Current state.
        from: RunState,
        /// Requested state.
        to: RunState,
    },
    /// A task state transition that is not in the allowed table.
    #[error("invalid transition for task {task}: {from:?} -> {to:?}")]
    InvalidTaskTransition {
        /// Task id.
        task: String,
        /// Current state.
        from: TaskState,
        /// Requested state.
        to: TaskState,
    },
    /// No task with the given id exists in the run.
    #[error("unknown task {0}")]
    UnknownTask(String),
    /// A task with this id already exists.
    #[error("duplicate task id {0}")]
    DuplicateTask(String),
    /// A task depends on an id that is not in the run.
    #[error("task {task} depends on unknown task {dependency}")]
    UnknownDependency {
        /// The task being added.
        task: String,
        /// The missing dependency.
        dependency: String,
    },
    /// Adding the task would create a dependency cycle.
    #[error("dependency cycle involving task {0}")]
    DependencyCycle(String),
    /// Tasks cannot be added in the run's current state.
    #[error("cannot add tasks while run is {0:?}")]
    RunNotEditable(RunState),
    /// The run id is empty or contains path separators.
    #[error("invalid run id {0:?}")]
    InvalidRunId(String),
    /// The run does not exist on disk.
    #[error("run not found: {0}")]
    RunNotFound(String),
    /// Filesystem error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// TOML parse error.
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
}
