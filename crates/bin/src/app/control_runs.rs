//! Orchestration runs over the control plane (run/task methods, merge policy). Filled in by T2.5.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde_json::Value;
use std::path::Path;
use tag_control::methods::WorktreeMergeParams;
use tag_control::{ClientCtx, ControlError, ControlResult, ReplyHandle};

/// Dispatch run/task methods. Returns `None` if `method` isn't one of them.
pub(crate) fn handle_runs(
    _state: &mut AppState,
    _client: &ClientCtx,
    _method: &str,
    _params: Value,
    _reply: ReplyHandle,
) -> Option<()> {
    None
}

/// Merge policy for conductor tokens: until runs are wired, conductors can't merge.
pub(crate) fn run_merge_gate(
    _state: &mut AppState,
    _client: &ClientCtx,
    _p: &WorktreeMergeParams,
    _worktree: &Path,
) -> Option<ControlResult> {
    Some(Err(ControlError::policy_blocked(
        "merges by a conductor need the run's merge policy",
    )))
}
