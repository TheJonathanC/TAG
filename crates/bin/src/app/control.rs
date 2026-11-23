//! App-loop side of the control plane: handles [`ControlEnvelope`]s forwarded by the control
//! server. Each handler runs a short critical section and replies; slow work (git, tests,
//! merges) runs on a background thread that replies when done.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tag_control::methods::*;
use tag_control::{ClientCtx, ControlEnvelope, ControlError, ControlResult, ReplyHandle, Role};
use tag_core::pane::{PaneId, PaneKind};
use tag_git::GitBackend;
use tag_orchestrator::Visibility;

/// Control-plane metadata for a pane (set by `pane.spawn` and the orchestrator).
#[derive(Debug, Clone, Default)]
pub(crate) struct PaneMeta {
    pub(crate) profile: Option<String>,
    pub(crate) run: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) worktree: Option<PathBuf>,
    /// Requested visibility for panes kept outside the layout (hidden/card).
    pub(crate) visibility: Option<Visibility>,
}

/// Handle one control request and send its reply (possibly later, from a worker thread).
pub(crate) fn handle_control(state: &mut AppState, env: ControlEnvelope) {
    let ControlEnvelope {
        client,
        method,
        params,
        reply,
    } = env;
    if handle_act(state, &client, &method, params.clone(), reply.clone()).is_some() {
        return;
    }
    let result = match method.as_str() {
        "projects.list" => respond(projects_list(state)),
        "workspaces.list" => respond(workspaces_list(state)),
        "panes.list" => parse_params(params).and_then(|p| respond(panes_list(state, &client, p))),
        "pane.read" => parse_params(params).and_then(|p| pane_read(state, p)),
        "agent.status" => parse_params(params).and_then(|p| agent_status(state, p)),
        "usage.summary" => parse_params(params).and_then(|p| usage_summary(state, p)),
        "worktree.list" => match parse_params::<ProjectFilter>(params) {
            Ok(p) => return worktree_list(state, p, reply),
            Err(e) => Err(e),
        },
        "worktree.diff" => match parse_params::<WorktreeDiffParams>(params) {
            Ok(p) => return worktree_diff(state, &client, p, reply),
            Err(e) => Err(e),
        },
        "run.list" => parse_params(params).and_then(|p| run_list(state, p)),
        "run.get" => parse_params(params).and_then(|p| run_get(state, &client, p)),
        _ => Err(ControlError::new(
            tag_control::codes::METHOD_NOT_FOUND,
            format!("{method} is not implemented yet"),
        )),
    };
    reply.send(result);
}

/// Parse typed params, mapping errors to `InvalidParams`.
pub(crate) fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, ControlError> {
    let params = if params.is_null() {
        Value::Object(Default::default())
    } else {
        params
    };
    serde_json::from_value(params).map_err(|e| ControlError::invalid_params(e.to_string()))
}

fn respond<T: Serialize>(v: T) -> ControlResult {
    serde_json::to_value(v).map_err(|e| ControlError::internal(e.to_string()))
}

/// Run `f` on a background thread and reply with its result.
pub(crate) fn reply_in_background<F>(reply: ReplyHandle, f: F)
where
    F: FnOnce() -> ControlResult + Send + 'static,
{
    std::thread::spawn(move || {
        reply.send(f());
    });
}

fn pane_kind_name(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::Shell => "shell",
        PaneKind::Agent => "agent",
        PaneKind::Tmux => "tmux",
        PaneKind::Applet => "applet",
        PaneKind::Donut => "donut",
    }
}

/// Where a pane currently shows: tiled in a layout, minimized (card), or nowhere (hidden).
pub(crate) fn pane_visibility(state: &AppState, pid: PaneId) -> Visibility {
    for ws in state.layout_mgr.workspaces() {
        if ws.all_panes().contains(&pid) {
            return Visibility::Pane;
        }
        if ws.minimized_panes.contains(&pid) {
            return Visibility::Card;
        }
    }
    state
        .pane_meta
        .get(&pid)
        .and_then(|m| m.visibility)
        .unwrap_or(Visibility::Hidden)
}

fn require_pane(state: &AppState, pane: u32) -> Result<PaneId, ControlError> {
    let pid = PaneId(pane);
    if state.panes.contains_key(&pid) {
        Ok(pid)
    } else {
        Err(ControlError::not_found(format!("no pane {pane}")))
    }
}

// ---------------------------------------------------------------------------------------------
// Projects & workspaces
// ---------------------------------------------------------------------------------------------

fn projects_list(state: &mut AppState) -> Vec<ProjectInfo> {
    let active = state.active_project();
    let agents = state
        .panes
        .values()
        .filter(|p| p.kind() == PaneKind::Agent)
        .count();
    let workspaces = state.layout_mgr.workspaces().len();
    state
        .projects
        .list_recent()
        .into_iter()
        .map(|p| {
            let is_active = active.as_ref().map(|a| a.id == p.id).unwrap_or(false);
            ProjectInfo {
                id: p.id.clone(),
                name: p.name.clone(),
                root: p.root.display().to_string(),
                active: is_active,
                // Until per-project sessions land (T3.2) everything belongs to the active one.
                workspaces: if is_active { workspaces } else { 0 },
                agents: if is_active { agents } else { 0 },
            }
        })
        .collect()
}

fn workspaces_list(state: &AppState) -> Vec<WorkspaceInfo> {
    let active = state.layout_mgr.active_workspace_id();
    state
        .layout_mgr
        .workspaces()
        .iter()
        .map(|ws| {
            let mut panes: Vec<u32> = ws.all_panes().iter().map(|p| p.0).collect();
            panes.extend(ws.minimized_panes.iter().map(|p| p.0));
            WorkspaceInfo {
                id: ws.id.0,
                name: ws.name.clone(),
                panes,
                active: ws.id == active,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Panes & agents
// ---------------------------------------------------------------------------------------------

fn panes_list(state: &AppState, client: &ClientCtx, p: PanesListParams) -> Vec<PaneInfo> {
    let ws_panes: Option<Vec<PaneId>> = p.workspace.map(|id| {
        state
            .layout_mgr
            .workspaces()
            .iter()
            .filter(|ws| ws.id.0 == id)
            .flat_map(|ws| {
                ws.all_panes()
                    .into_iter()
                    .chain(ws.minimized_panes.iter().copied())
            })
            .collect()
    });
    let mut out: Vec<PaneInfo> = state
        .panes
        .iter()
        .filter(|(pid, pane)| {
            if client.role == Role::Worker && client.pane != Some(pid.0) {
                return false;
            }
            if let Some(kind) = &p.kind {
                if pane_kind_name(pane.kind()) != kind {
                    return false;
                }
            }
            ws_panes.as_ref().map(|w| w.contains(pid)).unwrap_or(true)
        })
        .map(|(pid, pane)| {
            let meta = state.pane_meta.get(pid);
            PaneInfo {
                id: pid.0,
                title: pane.title(),
                kind: pane_kind_name(pane.kind()).into(),
                profile: meta
                    .and_then(|m| m.profile.clone())
                    .or_else(|| pane.agent_profile()),
                status: pane.agent_status().map(tag_core::event::normalize_status),
                visibility: pane_visibility(state, *pid),
                cwd: pane.cwd().map(|c| c.display().to_string()),
                worktree: meta
                    .and_then(|m| m.worktree.as_ref())
                    .map(|w| w.display().to_string()),
                run: meta.and_then(|m| m.run.clone()),
                task: meta.and_then(|m| m.task.clone()),
            }
        })
        .collect();
    out.sort_by_key(|p| p.id);
    out
}

/// Keep at most `max_bytes` from the end of `text`, on a char boundary.
pub(crate) fn tail_bytes(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

fn pane_read(state: &mut AppState, p: PaneReadParams) -> ControlResult {
    let pid = require_pane(state, p.pane)?;
    let pane = state.panes.get_mut(&pid).expect("checked");
    let status = pane.agent_status().map(tag_core::event::normalize_status);
    let text = pane.read_text(p.lines).unwrap_or_default();
    let line_count = text.lines().count();
    let (text, mut truncated) = tail_bytes(&text, p.max_bytes);
    // The emulator keeps more history than we return when the tail filled the request.
    truncated |= line_count >= p.lines;
    respond(PaneReadResult {
        text,
        truncated,
        status,
    })
}

fn agent_status(state: &AppState, p: PaneRef) -> ControlResult {
    let pid = require_pane(state, p.pane)?;
    let pane = &state.panes[&pid];
    let last_hook = state
        .attention_queue
        .items()
        .iter()
        .rev()
        .find(|i| i.pane_id == Some(pid))
        .map(|i| format!("{:?}", i.event));
    respond(AgentStatusResult {
        status: pane.agent_status().map(tag_core::event::normalize_status),
        since: state.status_since.get(&pid).map(|t| t.to_rfc3339()),
        last_hook,
        tokens: None,
    })
}

fn usage_summary(state: &AppState, p: UsageSummaryParams) -> ControlResult {
    let since = match p.since.as_deref() {
        None => None,
        Some(s) => Some(parse_since(s).ok_or_else(|| {
            ControlError::invalid_params(format!("since: expected RFC 3339 or YYYY-MM-DD: {s}"))
        })?),
    };
    let agg = match since {
        Some(t) => state.usage_store.query_aggregate_since(t),
        None => state.usage_store.query_today(),
    }
    .map_err(|e| ControlError::internal(e.to_string()))?;
    respond(UsageSummary {
        input: agg.input_tokens,
        output: agg.output_tokens,
        cached: agg.cache_read + agg.cache_write,
        cost_usd: agg.estimated_cost_usd,
        by_profile: Default::default(),
    })
}

fn parse_since(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc())
}

// ---------------------------------------------------------------------------------------------
// Worktrees
// ---------------------------------------------------------------------------------------------

/// Repository root for a request: the named project, else the active project, else the
/// focused pane's repo (or the process cwd).
pub(crate) fn repo_for(
    state: &mut AppState,
    project: Option<&str>,
) -> Result<PathBuf, ControlError> {
    if let Some(p) = project {
        return state
            .projects
            .get(p)
            .map(|p| p.root.clone())
            .ok_or_else(|| ControlError::not_found(format!("no project {p}")));
    }
    if let Some(p) = state.active_project() {
        return Ok(p.root);
    }
    Ok(get_active_repo_path(state))
}

fn worktree_list(state: &mut AppState, p: ProjectFilter, reply: ReplyHandle) {
    let repo = match repo_for(state, p.project.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    let tasks = worktree_tasks(state);
    reply_in_background(reply, move || {
        let list = list_worktrees(&repo)?;
        respond(
            list.into_iter()
                .map(|w| {
                    let task = tasks
                        .iter()
                        .find(|(p, _)| p == &w.path)
                        .map(|(_, t)| t.clone());
                    WorktreeInfo {
                        name: w.name,
                        path: w.path.display().to_string(),
                        branch: w.branch,
                        is_main: w.is_main,
                        task,
                    }
                })
                .collect::<Vec<_>>(),
        )
    });
}

/// (worktree path, "run/task") for panes bound to a task.
fn worktree_tasks(state: &AppState) -> Vec<(PathBuf, String)> {
    state
        .pane_meta
        .values()
        .filter_map(|m| {
            Some((
                m.worktree.clone()?,
                format!("{}/{}", m.run.as_deref()?, m.task.as_deref()?),
            ))
        })
        .collect()
}

pub(crate) fn list_worktrees(repo: &Path) -> Result<Vec<tag_git::WorktreeInfo>, ControlError> {
    tag_git::StandardGitBackend
        .list_worktrees(repo)
        .map_err(|e| ControlError::internal(format!("git: {e}")))
}

/// Find a worktree of `repo` by name or path.
pub(crate) fn find_worktree(
    repo: &Path,
    name_or_path: &str,
) -> Result<tag_git::WorktreeInfo, ControlError> {
    let wanted = Path::new(name_or_path);
    let canon = std::fs::canonicalize(wanted).ok();
    list_worktrees(repo)?
        .into_iter()
        .find(|w| {
            w.name == name_or_path
                || w.path == wanted
                || canon.is_some() && std::fs::canonicalize(&w.path).ok() == canon
                || w.path.file_name().and_then(|n| n.to_str()) == Some(name_or_path)
        })
        .ok_or_else(|| ControlError::not_found(format!("no worktree {name_or_path}")))
}

/// Run git in `dir`, returning stdout.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, ControlError> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| ControlError::internal(format!("git: {e}")))?;
    if !out.status.success() {
        return Err(ControlError::internal(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The branch checked out in the main worktree.
pub(crate) fn main_branch(repo: &Path) -> Result<String, ControlError> {
    Ok(git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string())
}

fn worktree_diff(
    state: &mut AppState,
    client: &ClientCtx,
    p: WorktreeDiffParams,
    reply: ReplyHandle,
) {
    let repo = match repo_for(state, None) {
        Ok(r) => r,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    let own = own_worktree(state, client);
    reply_in_background(reply, move || {
        let wt = find_worktree(&repo, &p.worktree)?;
        check_worktree_scope(&own, &wt.path)?;
        let (stat, diff, truncated) = diff_against_base(&repo, &wt.path, p.max_bytes, p.stat_only)?;
        respond(DiffResult {
            stat,
            diff,
            truncated,
        })
    });
}

/// For a worker token, the worktree it may touch. `Ok(None)` = unrestricted.
pub(crate) fn own_worktree(
    state: &AppState,
    client: &ClientCtx,
) -> Result<Option<PathBuf>, ControlError> {
    if client.role != Role::Worker {
        return Ok(None);
    }
    let wt = client
        .pane
        .and_then(|p| state.pane_meta.get(&PaneId(p)))
        .and_then(|m| m.worktree.clone())
        .ok_or_else(|| ControlError::forbidden("this worker has no worktree"))?;
    Ok(Some(wt))
}

pub(crate) fn check_worktree_scope(
    own: &Result<Option<PathBuf>, ControlError>,
    path: &Path,
) -> Result<(), ControlError> {
    match own {
        Err(e) => Err(e.clone()),
        Ok(None) => Ok(()),
        Ok(Some(own)) => {
            let a = std::fs::canonicalize(own).unwrap_or_else(|_| own.clone());
            let b = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            if a == b {
                Ok(())
            } else {
                Err(ControlError::forbidden(
                    "workers may only touch their own worktree",
                ))
            }
        }
    }
}

/// Diff of a worktree against its merge-base with the main branch, including uncommitted
/// changes. Over `max_bytes`: the full `--stat`, then whole-file sections until the cap.
pub(crate) fn diff_against_base(
    repo: &Path,
    worktree: &Path,
    max_bytes: usize,
    stat_only: bool,
) -> Result<(String, String, bool), ControlError> {
    let base_branch = main_branch(repo)?;
    let base = git(worktree, &["merge-base", "HEAD", &base_branch])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".into());
    let stat = git(worktree, &["diff", "--stat", &base])?;
    if stat_only {
        return Ok((stat, String::new(), false));
    }
    let full = git(worktree, &["diff", &base])?;
    let (diff, truncated) = cap_diff(&full, max_bytes);
    Ok((stat, diff, truncated))
}

/// Keep whole `diff --git` sections while they fit in `max_bytes`.
pub(crate) fn cap_diff(full: &str, max_bytes: usize) -> (String, bool) {
    if full.len() <= max_bytes {
        return (full.to_string(), false);
    }
    let mut out = String::new();
    let mut sections = Vec::new();
    let mut start = 0;
    for (i, _) in full.match_indices("\ndiff --git ") {
        sections.push(&full[start..=i]);
        start = i + 1;
    }
    sections.push(&full[start..]);
    for s in sections {
        if out.len() + s.len() > max_bytes {
            break;
        }
        out.push_str(s);
    }
    (out, true)
}

// ---------------------------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------------------------

/// Run stores for every project under `<state>/projects/*/runs`.
pub(crate) fn run_stores(state: &AppState) -> Vec<(String, tag_orchestrator::RunStore)> {
    let dir = state.paths.state_dir.join("projects");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().join("runs").is_dir())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                tag_orchestrator::RunStore::new(e.path().join("runs")),
            )
        })
        .collect()
}

fn run_list(state: &AppState, p: RunListParams) -> ControlResult {
    let project = p.project.as_deref().map(|p| {
        state
            .projects
            .get(p)
            .map(|x| x.id.clone())
            .unwrap_or(p.to_string())
    });
    let mut runs = Vec::new();
    for (pid, store) in run_stores(state) {
        if project.as_ref().is_some_and(|p| p != &pid) {
            continue;
        }
        if let Ok(list) = store.list() {
            runs.extend(list.runs);
        }
    }
    if p.active_only {
        runs.retain(|r| !r.state.is_terminal());
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    respond(runs)
}

pub(crate) fn load_run(
    state: &AppState,
    id: &str,
) -> Result<(tag_orchestrator::RunStore, tag_orchestrator::Run), ControlError> {
    for (_, store) in run_stores(state) {
        if let Ok(run) = store.load(id) {
            return Ok((store, run));
        }
    }
    Err(ControlError::not_found(format!("no run {id}")))
}

fn run_get(state: &AppState, client: &ClientCtx, p: RunRef) -> ControlResult {
    if client.role != Role::External && client.run.as_deref() != Some(p.run.as_str()) {
        return Err(ControlError::forbidden("token is scoped to another run"));
    }
    respond(load_run(state, &p.run)?.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_bytes_respects_char_boundaries() {
        let (t, cut) = tail_bytes("héllo wörld", 6);
        assert!(cut);
        assert!(t.len() <= 6);
        assert!("héllo wörld".ends_with(&t));
        assert_eq!(tail_bytes("abc", 10), ("abc".into(), false));
    }

    #[test]
    fn cap_diff_keeps_whole_sections() {
        let d = "diff --git a/x b/x\n+1\ndiff --git a/y b/y\n+2222222222\n";
        let (out, cut) = cap_diff(d, 25);
        assert!(cut);
        assert_eq!(out, "diff --git a/x b/x\n+1\n");
    }
}
