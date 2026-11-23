//! Control-plane dangerous methods: merge, remove worktrees, kill panes (T1.5).

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde_json::{json, Value};
use std::path::Path;
use tag_control::methods::*;
use tag_control::{ClientCtx, ControlError, ControlResult, ReplyHandle, Role};
use tag_core::pane::PaneId;

fn respond<T: serde::Serialize>(v: T) -> ControlResult {
    serde_json::to_value(v).map_err(|e| ControlError::internal(e.to_string()))
}

/// Dispatch dangerous methods. Returns `None` if `method` isn't one of them.
pub(crate) fn handle_dangerous(
    state: &mut AppState,
    client: &ClientCtx,
    method: &str,
    params: Value,
    reply: ReplyHandle,
) -> Option<()> {
    let result: ControlResult = match method {
        "worktree.merge" => match parse_params::<WorktreeMergeParams>(params) {
            Ok(p) => {
                worktree_merge(state, client, p, reply);
                return Some(());
            }
            Err(e) => Err(e),
        },
        "worktree.remove" => match parse_params::<WorktreeRemoveParams>(params) {
            Ok(p) => {
                worktree_remove(state, p, reply);
                return Some(());
            }
            Err(e) => Err(e),
        },
        "pane.kill" => parse_params::<PaneRef>(params).and_then(|p| pane_kill(state, client, p)),
        _ => return None,
    };
    reply.send(result);
    Some(())
}

fn pane_kill(state: &mut AppState, client: &ClientCtx, p: PaneRef) -> ControlResult {
    let pid = PaneId(p.pane);
    if !state.panes.contains_key(&pid) {
        return Err(ControlError::not_found(format!("no pane {}", p.pane)));
    }
    if client.role == Role::Conductor
        && state.pane_meta.get(&pid).and_then(|m| m.run.as_deref()) != client.run.as_deref()
    {
        return Err(ControlError::forbidden("pane belongs to another run"));
    }
    close_pane(state, pid);
    state.pane_meta.remove(&pid);
    state.pending_prompts.remove(&pid);
    state.is_dirty = true;
    respond(Empty {})
}

fn worktree_remove(state: &mut AppState, p: WorktreeRemoveParams, reply: ReplyHandle) {
    let repo = match repo_for(state, None) {
        Ok(r) => r,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    reply_in_background(reply, move || {
        let wt = find_worktree(&repo, &p.worktree)?;
        if wt.is_main {
            return Err(ControlError::policy_blocked(
                "refusing to remove the main worktree",
            ));
        }
        let path = wt.path.display().to_string();
        let mut args = vec!["worktree", "remove"];
        if p.force {
            args.push("--force");
        }
        args.push(&path);
        git(&repo, &args)?;
        respond(Empty {})
    });
}

/// Policy gate for a merge: the last `tests.run` in the worktree passed at the branch HEAD.
pub(crate) fn check_tests_passed(
    record: Option<&TestRecord>,
    head: &str,
) -> Result<(), ControlError> {
    match record {
        None => Err(ControlError::policy_blocked(
            "no passing tests.run for this worktree; run tests first (or require_tests=false)",
        )),
        Some(r) if r.exit_code != 0 => Err(ControlError::policy_blocked(format!(
            "last tests.run failed (exit {}) at {}",
            r.exit_code,
            short(&r.commit)
        ))),
        Some(r) if r.commit != head => Err(ControlError::policy_blocked(format!(
            "tests passed at {} but the branch is now at {}; run tests again",
            short(&r.commit),
            short(head)
        ))
        .with_data(json!({"tested": r.commit, "head": head}))),
        Some(r) if r.dirty => Err(ControlError::policy_blocked(
            "tests ran with uncommitted changes; commit and run tests again",
        )),
        Some(_) => Ok(()),
    }
}

fn short(c: &str) -> &str {
    &c[..c.len().min(8)]
}

/// Resolve the worktree to merge from `{worktree}` or `{run, task}`.
fn merge_target(state: &AppState, p: &WorktreeMergeParams) -> Result<String, ControlError> {
    if let Some(w) = &p.worktree {
        return Ok(w.clone());
    }
    let (Some(run), Some(task)) = (&p.run, &p.task) else {
        return Err(ControlError::invalid_params(
            "give worktree, or run and task",
        ));
    };
    let (_, run) = load_run(state, run)?;
    let t = run
        .task(task)
        .ok_or_else(|| ControlError::not_found(format!("no task {task}")))?;
    t.worktree
        .clone()
        .ok_or_else(|| ControlError::not_found(format!("task {task} has no worktree")))
}

fn worktree_merge(
    state: &mut AppState,
    client: &ClientCtx,
    p: WorktreeMergeParams,
    reply: ReplyHandle,
) {
    let prepared = (|| {
        let repo = repo_for(state, None)?;
        let target = merge_target(state, &p)?;
        let wt = find_worktree(&repo, &target)?;
        if wt.is_main {
            return Err(ControlError::invalid_params(
                "cannot merge the main worktree into itself",
            ));
        }
        let record = state.test_results.get(&canonical(&wt.path)).cloned();
        Ok((repo, wt, record))
    })();
    let (repo, wt, record) = match prepared {
        Ok(x) => x,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    if client.role == Role::Conductor {
        if let Some(queued) = conductor_merge_gate(state, client, &p, &wt.path) {
            reply.send(queued);
            return;
        }
    }
    let into = p.into.clone();
    let require_tests = p.require_tests;
    let tx = state.event_tx.clone();
    let wt_path = wt.path.clone();
    reply_in_background(reply, move || {
        let branch = wt
            .branch
            .clone()
            .ok_or_else(|| ControlError::invalid_params("worktree has a detached HEAD"))?;
        let head = git(&repo, &["rev-parse", &branch])?.trim().to_string();
        if require_tests {
            check_tests_passed(record.as_ref(), &head)?;
        }
        let commit = merge_branch(&repo, &branch, into.as_deref())?;
        let (r, _rx) = ReplyHandle::new();
        continue_in_loop(
            &tx,
            crate::app::internal_ctx(),
            "_internal.merged",
            json!({ "worktree": wt_path, "commit": commit }),
            r,
        );
        respond(MergeResult {
            merged_commit: commit,
            queued: false,
        })
    });
}

/// Hook for run-scoped merge policy (filled in by the orchestrator wiring). `Some` means the
/// request was answered (blocked or queued for the human).
pub(crate) fn conductor_merge_gate(
    state: &mut AppState,
    client: &ClientCtx,
    p: &WorktreeMergeParams,
    worktree: &Path,
) -> Option<ControlResult> {
    run_merge_gate(state, client, p, worktree)
}

/// Merge `branch` into the main worktree (on `into`, default its current branch).
/// On conflict the merge is aborted and `Conflict` lists the files.
pub(crate) fn merge_branch(
    repo: &Path,
    branch: &str,
    into: Option<&str>,
) -> Result<String, ControlError> {
    let current = main_branch(repo)?;
    if let Some(into) = into {
        if into != current {
            return Err(ControlError::policy_blocked(format!(
                "main worktree is on '{current}', not '{into}'; check out '{into}' first"
            )));
        }
    }
    let dirty = git(repo, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        return Err(ControlError::policy_blocked(
            "main worktree has uncommitted changes; commit or stash them first",
        ));
    }
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=TAG",
            "-c",
            "user.email=tag@localhost",
            "merge",
            "--no-ff",
            "--no-edit",
            branch,
        ])
        .current_dir(repo)
        .env("GIT_MERGE_AUTOEDIT", "no")
        .output()
        .map_err(|e| ControlError::internal(format!("git merge: {e}")))?;
    if !out.status.success() {
        let files: Vec<String> = git(repo, &["diff", "--name-only", "--diff-filter=U"])
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        let _ = git(repo, &["merge", "--abort"]);
        if files.is_empty() {
            return Err(ControlError::internal(format!(
                "git merge failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        return Err(ControlError::conflict(format!(
            "merging {branch} conflicts in {} file(s); merge aborted",
            files.len()
        ))
        .with_data(json!({ "files": files })));
    }
    Ok(git(repo, &["rev-parse", "HEAD"])?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(commit: &str, code: i32, dirty: bool) -> TestRecord {
        TestRecord {
            commit: commit.into(),
            exit_code: code,
            dirty,
            at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_gate() {
        assert!(check_tests_passed(None, "a").is_err());
        assert!(check_tests_passed(Some(&rec("a", 1, false)), "a").is_err());
        assert!(check_tests_passed(Some(&rec("a", 0, false)), "b").is_err());
        assert!(check_tests_passed(Some(&rec("a", 0, true)), "a").is_err());
        assert!(check_tests_passed(Some(&rec("a", 0, false)), "a").is_ok());
        let e = check_tests_passed(Some(&rec("a", 0, false)), "b").unwrap_err();
        assert_eq!(e.code, tag_control::codes::POLICY_BLOCKED);
    }
}
