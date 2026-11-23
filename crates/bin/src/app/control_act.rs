//! Control-plane act methods: spawning and driving panes, worktrees and tests (T1.4).

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tag_agents::{AgentPane, AgentProfile};
use tag_clipboard::wrap_bracketed_paste;
use tag_control::methods::*;
use tag_control::{ClientCtx, ControlEnvelope, ControlError, ControlResult, ReplyHandle, Role};
use tag_core::action::SplitDirection;
use tag_core::config::{LaunchOpts, PromptDelivery};
use tag_core::event::AppEvent;
use tag_core::pane::{PaneId, PaneKind};
use tag_orchestrator::Visibility;
use tag_pty::PtyPane;

/// Size of panes that aren't in any layout (§5.4).
pub(crate) const HIDDEN_COLS: u16 = 120;
pub(crate) const HIDDEN_ROWS: u16 = 40;

/// How long a typed prompt waits for an agent with no output before it is pasted anyway.
const PROMPT_QUIET: Duration = Duration::from_secs(3);

/// A prompt waiting to be typed into an agent once it is ready (`prompt_delivery = "type"`).
#[derive(Debug, Clone)]
pub(crate) struct PendingPrompt {
    pub(crate) text: String,
    pub(crate) since: Instant,
}

/// Result of the last `tests.run` in a worktree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TestRecord {
    pub(crate) commit: String,
    pub(crate) exit_code: i32,
    pub(crate) dirty: bool,
    pub(crate) at: chrono::DateTime<chrono::Utc>,
    /// Last lines of output.
    #[serde(default)]
    pub(crate) tail: String,
}

/// Extra launch settings used by the orchestrator (token env, branch naming).
#[derive(Debug, Clone, Default)]
pub(crate) struct SpawnExtras {
    pub(crate) env: HashMap<String, String>,
    pub(crate) mcp_config_path: Option<String>,
}

fn respond<T: serde::Serialize>(v: T) -> ControlResult {
    serde_json::to_value(v).map_err(|e| ControlError::internal(e.to_string()))
}

/// Re-enter the app loop with an internal continuation that answers `reply`.
pub(crate) fn continue_in_loop(
    event_tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
    client: ClientCtx,
    method: &str,
    params: Value,
    reply: ReplyHandle,
) {
    let env = ControlEnvelope {
        client,
        method: method.to_string(),
        params,
        reply: reply.clone(),
    };
    if event_tx.send(AppEvent::Control(env)).is_err() {
        reply.send(Err(ControlError::internal("TAG is shutting down")));
    }
}

/// Dispatch act methods. Returns `None` if `method` isn't one of them.
pub(crate) fn handle_act(
    state: &mut AppState,
    client: &ClientCtx,
    method: &str,
    params: Value,
    reply: ReplyHandle,
) -> Option<()> {
    let result: ControlResult = match method {
        "pane.spawn" => match parse_params::<SpawnParams>(params) {
            Ok(p) => {
                pane_spawn(state, client, p, reply);
                return Some(());
            }
            Err(e) => Err(e),
        },
        "_internal.spawn_in_worktree" => {
            let spawn = parse_params::<SpawnParams>(params["spawn"].clone());
            let wt: Option<WorktreeInfo> = serde_json::from_value(params["worktree"].clone()).ok();
            spawn.and_then(|p| {
                spawn_pane_now(state, client, p, wt, SpawnExtras::default()).and_then(respond)
            })
        }
        "pane.send" => parse_params(params).and_then(|p| pane_send(state, client, p)),
        "pane.set_visibility" => {
            parse_params(params).and_then(|p| pane_set_visibility(state, client, p))
        }
        "worktree.create" => match parse_params::<WorktreeCreateParams>(params) {
            Ok(p) => {
                worktree_create(state, p, reply);
                return Some(());
            }
            Err(e) => Err(e),
        },
        "tests.run" => match parse_params::<TestsRunParams>(params) {
            Ok(p) => {
                tests_run(state, client, p, reply);
                return Some(());
            }
            Err(e) => Err(e),
        },
        "_internal.test_result" => {
            if let (Some(path), Ok(rec)) = (
                params["worktree"].as_str(),
                serde_json::from_value::<TestRecord>(params["record"].clone()),
            ) {
                record_task_test(state, Path::new(path), &rec);
                state.test_results.insert(canonical(Path::new(path)), rec);
            }
            Ok(Value::Null)
        }
        _ => return None,
    };
    reply.send(result);
    Some(())
}

pub(crate) fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Conductors may only drive panes of their own run.
fn check_pane_scope(state: &AppState, client: &ClientCtx, pid: PaneId) -> Result<(), ControlError> {
    if client.role == Role::Conductor {
        let run = state.pane_meta.get(&pid).and_then(|m| m.run.as_deref());
        if run != client.run.as_deref() {
            return Err(ControlError::forbidden("pane belongs to another run"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// pane.spawn
// ---------------------------------------------------------------------------------------------

fn pane_spawn(state: &mut AppState, client: &ClientCtx, mut p: SpawnParams, reply: ReplyHandle) {
    if client.role == Role::Conductor {
        // Conductors spawn into their own run only.
        p.run = client.run.clone();
    }
    match p.worktree.clone() {
        Some(spec) if spec.create => {
            let repo = match repo_for(state, p.project.as_deref()) {
                Ok(r) => r,
                Err(e) => {
                    reply.send(Err(e));
                    return;
                }
            };
            let tx = state.event_tx.clone();
            let client = client.clone();
            std::thread::spawn(move || {
                match ensure_worktree(&repo, &spec.name, &spec.name, spec.base.as_deref()) {
                    Ok(wt) => {
                        let mut p = p;
                        p.worktree = None;
                        p.cwd = Some(wt.path.clone());
                        continue_in_loop(
                            &tx,
                            client,
                            "_internal.spawn_in_worktree",
                            json!({ "spawn": p, "worktree": wt }),
                            reply,
                        );
                    }
                    Err(e) => {
                        reply.send(Err(e));
                    }
                }
            });
        }
        Some(spec) => {
            // Existing worktree: resolve it, then spawn.
            let res = repo_for(state, p.project.as_deref())
                .and_then(|repo| find_worktree(&repo, &spec.name))
                .and_then(|wt| {
                    let info = WorktreeInfo {
                        name: wt.name,
                        path: wt.path.display().to_string(),
                        branch: wt.branch,
                        is_main: wt.is_main,
                        task: None,
                    };
                    p.cwd = Some(info.path.clone());
                    p.worktree = None;
                    spawn_pane_now(state, client, p, Some(info), SpawnExtras::default())
                });
            reply.send(res.and_then(respond));
        }
        None => {
            let res = spawn_pane_now(state, client, p, None, SpawnExtras::default());
            reply.send(res.and_then(respond));
        }
    }
}

/// Spawn a pane synchronously (worktree, if any, already exists).
pub(crate) fn spawn_pane_now(
    state: &mut AppState,
    _client: &ClientCtx,
    p: SpawnParams,
    worktree: Option<WorktreeInfo>,
    extras: SpawnExtras,
) -> Result<SpawnResult, ControlError> {
    let mut env = extras.env.clone();
    if !p.secrets.is_empty() {
        if !state.vault.is_unlocked() {
            return Err(ControlError::policy_blocked(
                "the vault is locked; unlock it in TAG to inject secrets",
            ));
        }
        for name in &p.secrets {
            let value = state
                .vault
                .get_secret_value(name)
                .ok_or_else(|| ControlError::not_found(format!("no vault secret {name}")))?;
            env.insert(name.clone(), value);
        }
    }
    let worktree_path = worktree.as_ref().map(|w| PathBuf::from(&w.path));
    let cwd: Option<PathBuf> = match (&worktree_path, &p.cwd) {
        (Some(w), _) => Some(w.clone()),
        (None, Some(c)) => Some(PathBuf::from(c)),
        (None, None) => repo_for(state, p.project.as_deref()).ok(),
    };
    let (w, h) = if p.visibility == Visibility::Pane {
        initial_pane_size(state)
    } else {
        (HIDDEN_COLS, HIDDEN_ROWS)
    };
    let pid = state.allocate_pane_id();
    let mut typed_prompt = None;
    let mut session_id: Option<String> = None;

    let pane: Box<dyn tag_core::pane::Pane> = if p.profile == "shell" {
        let mut shell_env = vault_env(state);
        shell_env.extend(env);
        let pane = PtyPane::spawn(
            pid,
            p.title.clone().unwrap_or_else(|| "shell".into()),
            PaneKind::Shell,
            None,
            &[],
            &shell_env,
            cwd,
            w,
            h,
            state.config.scrollback_lines,
            state.event_tx.clone(),
        )
        .map_err(|e| ControlError::internal(format!("spawn failed: {e}")))?;
        typed_prompt = p.prompt.clone();
        Box::new(pane)
    } else {
        let cfg = state
            .config
            .agents
            .get(&p.profile)
            .cloned()
            .ok_or_else(|| {
                let mut names: Vec<_> = state.config.agents.keys().cloned().collect();
                names.sort();
                ControlError::not_found(format!(
                    "no agent profile '{}' (have: shell, {})",
                    p.profile,
                    names.join(", ")
                ))
            })?;
        let mut opts = LaunchOpts {
            mcp_config_path: extras.mcp_config_path.clone(),
            ..Default::default()
        };
        if !cfg.session_id_args.is_empty() {
            let id = new_session_uuid();
            opts.new_session_id = Some(id.clone());
            session_id = Some(id);
        }
        match cfg.prompt_delivery {
            PromptDelivery::Arg => opts.prompt = p.prompt.clone(),
            PromptDelivery::Type | PromptDelivery::Stdin => typed_prompt = p.prompt.clone(),
        }
        let (command, args) = cfg.build_argv(&opts);
        let mut prof = AgentProfile::from_config(&cfg);
        prof.command = command;
        prof.args = args;
        prof.env.extend(env);
        prof.default_cwd = cwd;
        if let Some(t) = &p.title {
            prof.name = t.clone();
        }
        let pane = AgentPane::spawn(
            pid,
            &prof,
            worktree_path.clone(),
            w,
            h,
            state.config.scrollback_lines,
            state.event_tx.clone(),
        )
        .map_err(|e| ControlError::internal(format!("spawn failed: {e}")))?;
        if cfg.prompt_delivery == PromptDelivery::Stdin {
            if let Some(text) = typed_prompt.take() {
                let mut pane = pane;
                pane_write(&mut pane, &text, true);
                state.panes.insert(pid, Box::new(pane));
                return finish_spawn(state, pid, p, worktree, worktree_path, session_id);
            }
        }
        Box::new(pane)
    };
    state.panes.insert(pid, pane);
    if let Some(text) = typed_prompt {
        state.pending_prompts.insert(
            pid,
            PendingPrompt {
                text,
                since: Instant::now(),
            },
        );
    }
    finish_spawn(state, pid, p, worktree, worktree_path, session_id)
}

fn finish_spawn(
    state: &mut AppState,
    pid: PaneId,
    p: SpawnParams,
    worktree: Option<WorktreeInfo>,
    worktree_path: Option<PathBuf>,
    session_id: Option<String>,
) -> Result<SpawnResult, ControlError> {
    state.pane_meta.insert(
        pid,
        PaneMeta {
            profile: Some(p.profile.clone()),
            run: p.run.clone(),
            task: p.task.clone(),
            worktree: worktree_path,
            visibility: Some(p.visibility),
            session_id,
        },
    );
    place_pane(state, pid, p.visibility, p.workspace);
    save_session_to_disk(state);
    state.is_dirty = true;
    Ok(SpawnResult {
        pane: pid.0,
        worktree,
    })
}

/// A random RFC 4122 version-4 UUID string.
pub(crate) fn new_session_uuid() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Put a pane into (or take it out of) the layout according to `visibility`.
pub(crate) fn place_pane(
    state: &mut AppState,
    pid: PaneId,
    visibility: Visibility,
    workspace: Option<usize>,
) {
    for ws in state.layout_mgr.workspaces_mut() {
        if ws.all_panes().contains(&pid) {
            ws.remove_pane(pid);
        }
        ws.minimized_panes.retain(|p| *p != pid);
    }
    let target = workspace
        .map(tag_core::pane::WorkspaceId)
        .filter(|id| state.layout_mgr.workspaces().iter().any(|w| w.id == *id))
        .unwrap_or_else(|| state.layout_mgr.active_workspace_id());
    match visibility {
        Visibility::Pane => {
            if let Some(ws) = state.layout_mgr.find_workspace_mut(target) {
                ws.zoomed_pane = None;
                if ws.root.is_none() {
                    ws.add_initial_pane(pid);
                } else {
                    let _ = ws.split_current(pid, SplitDirection::Vertical);
                }
            }
            state.needs_clear = true;
        }
        Visibility::Card => {
            if let Some(ws) = state.layout_mgr.find_workspace_mut(target) {
                ws.minimized_panes.push(pid);
            }
            resize_hidden(state, pid);
        }
        Visibility::Hidden => resize_hidden(state, pid),
    }
    if let Some(m) = state.pane_meta.get_mut(&pid) {
        m.visibility = Some(visibility);
    }
}

fn resize_hidden(state: &mut AppState, pid: PaneId) {
    if let Some(pane) = state.panes.get_mut(&pid) {
        pane.on_resize(HIDDEN_COLS, HIDDEN_ROWS);
    }
}

/// Paste `text` into a pane (bracketed only if the program enabled it), then Enter.
pub(crate) fn pane_write<P: tag_core::pane::Pane + ?Sized>(pane: &mut P, text: &str, submit: bool) {
    let bytes = if pane.bracketed_paste() {
        wrap_bracketed_paste(text)
    } else {
        text.as_bytes().to_vec()
    };
    pane.send_input(&bytes);
    if submit {
        pane.send_input(b"\r");
    }
}

/// Deliver queued prompts to agents that are ready (idle/awaiting input, or quiet for 3 s).
pub(crate) fn deliver_pending_prompts(state: &mut AppState) {
    if state.pending_prompts.is_empty() {
        return;
    }
    let now = Instant::now();
    let ready: Vec<PaneId> = state
        .pending_prompts
        .iter()
        .filter(|(pid, pp)| {
            let Some(pane) = state.panes.get(pid) else {
                return true; // pane gone: drop the prompt
            };
            let status_ready = matches!(pane.agent_status(), Some("idle") | Some("awaiting-input"));
            let last = state.last_output.get(pid).copied().unwrap_or(pp.since);
            status_ready || now.duration_since(last.max(pp.since)) >= PROMPT_QUIET
        })
        .map(|(pid, _)| *pid)
        .collect();
    for pid in ready {
        if let Some(pp) = state.pending_prompts.remove(&pid) {
            if let Some(pane) = state.panes.get_mut(&pid) {
                pane_write(pane.as_mut(), &pp.text, true);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// pane.send / pane.set_visibility
// ---------------------------------------------------------------------------------------------

fn pane_send(state: &mut AppState, client: &ClientCtx, p: PaneSendParams) -> ControlResult {
    let pid = PaneId(p.pane);
    check_pane_scope(state, client, pid)?;
    let pane = state
        .panes
        .get_mut(&pid)
        .ok_or_else(|| ControlError::not_found(format!("no pane {}", p.pane)))?;
    pane_write(pane.as_mut(), &p.text, p.submit);
    respond(Empty {})
}

fn pane_set_visibility(
    state: &mut AppState,
    client: &ClientCtx,
    p: SetVisibilityParams,
) -> ControlResult {
    let pid = PaneId(p.pane);
    if !state.panes.contains_key(&pid) {
        return Err(ControlError::not_found(format!("no pane {}", p.pane)));
    }
    check_pane_scope(state, client, pid)?;
    state.pane_meta.entry(pid).or_default();
    place_pane(state, pid, p.visibility, None);
    save_session_to_disk(state);
    state.is_dirty = true;
    respond(Empty {})
}

// ---------------------------------------------------------------------------------------------
// Worktrees
// ---------------------------------------------------------------------------------------------

/// Create `<repo>/.worktrees/<name>` on `branch` (new from `base`, default HEAD), or return it
/// if it already exists. `.worktrees/` is added to the repo's git exclude.
pub(crate) fn ensure_worktree(
    repo: &Path,
    name: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<WorktreeInfo, ControlError> {
    if name.is_empty() || name.contains("..") || name.starts_with('/') {
        return Err(ControlError::invalid_params(format!(
            "bad worktree name {name:?}"
        )));
    }
    let path = repo.join(".worktrees").join(name);
    exclude_in_repo(repo, ".worktrees/");
    if !path.exists() {
        let branch_exists = git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        let path_s = path.display().to_string();
        if branch_exists {
            git(repo, &["worktree", "add", &path_s, branch])?;
        } else {
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    &path_s,
                    base.unwrap_or("HEAD"),
                ],
            )?;
        }
    }
    let wt = find_worktree(repo, &path.display().to_string())?;
    Ok(WorktreeInfo {
        name: wt.name,
        path: wt.path.display().to_string(),
        branch: wt.branch,
        is_main: wt.is_main,
        task: None,
    })
}

/// Append `pattern` to `<repo>/.git/info/exclude` if missing.
pub(crate) fn exclude_in_repo(repo: &Path, pattern: &str) {
    let Ok(git_dir) = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) else {
        return;
    };
    let exclude = PathBuf::from(git_dir.trim()).join("info").join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|l| l.trim() == pattern) {
        return;
    }
    if let Some(parent) = exclude.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(pattern);
    text.push('\n');
    let _ = std::fs::write(&exclude, text);
}

fn worktree_create(state: &mut AppState, p: WorktreeCreateParams, reply: ReplyHandle) {
    let repo = match repo_for(state, p.project.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    reply_in_background(reply, move || {
        let wt = ensure_worktree(&repo, &p.name, &p.name, p.base.as_deref())?;
        respond(WorktreeCreateResult {
            branch: wt.branch.unwrap_or_else(|| p.name.clone()),
            path: wt.path,
        })
    });
}

// ---------------------------------------------------------------------------------------------
// tests.run
// ---------------------------------------------------------------------------------------------

/// Strip ANSI escape sequences (CSI, OSC, two-byte ESC) from `s`.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Last `max_lines` lines / `max_bytes` bytes of `s`.
pub(crate) fn tail_lines(s: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let joined = lines[start..].join("\n");
    tail_bytes(&joined, max_bytes).0
}

fn tests_run(state: &mut AppState, client: &ClientCtx, p: TestsRunParams, reply: ReplyHandle) {
    let repo = match repo_for(state, None) {
        Ok(r) => r,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    let target = match resolve_task_worktree(
        state,
        p.worktree.as_deref(),
        p.run.as_deref(),
        p.task.as_deref(),
    ) {
        Ok(t) => t,
        Err(e) => {
            reply.send(Err(e));
            return;
        }
    };
    let own = own_worktree(state, client);
    let tx = state.event_tx.clone();
    let client = client.clone();
    std::thread::spawn(move || {
        let res = (|| {
            let wt = find_worktree(&repo, &target)?;
            check_worktree_scope(&own, &wt.path)?;
            let command = match p.command.clone() {
                Some(c) => c,
                None => tag_orchestrator::ProjectConfig::load(&repo)
                    .ok()
                    .flatten()
                    .and_then(|c| c.test_command)
                    .ok_or_else(|| {
                        ControlError::invalid_params(
                            "no command given and no test_command in .tag/project.toml",
                        )
                    })?,
            };
            let commit = git(&wt.path, &["rev-parse", "HEAD"])?.trim().to_string();
            let dirty = !git(&wt.path, &["status", "--porcelain"])?.trim().is_empty();
            let (exit_code, duration, output) =
                run_with_timeout(&command, &wt.path, Duration::from_secs(p.timeout_s))?;
            let record = TestRecord {
                commit: commit.clone(),
                exit_code,
                dirty,
                at: chrono::Utc::now(),
                tail: tail_lines(&strip_ansi(&output), 60, 8192),
            };
            let (record_reply, _rx) = ReplyHandle::new();
            continue_in_loop(
                &tx,
                client.clone(),
                "_internal.test_result",
                json!({ "worktree": wt.path, "record": record }),
                record_reply,
            );
            respond(TestRunResult {
                exit_code,
                duration_s: duration.as_secs_f64(),
                tail: tail_lines(&strip_ansi(&output), 60, 8192),
                commit,
            })
        })();
        reply.send(res);
    });
}

/// Run `sh -c command` in `dir`, capturing stdout+stderr. Exit code 124 on timeout.
fn run_with_timeout(
    command: &str,
    dir: &Path,
    timeout: Duration,
) -> Result<(i32, Duration, String), ControlError> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let start = Instant::now();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{command} 2>&1"))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ControlError::internal(format!("cannot run tests: {e}")))?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let code = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| ControlError::internal(e.to_string()))?
        {
            break status.code().unwrap_or(-1);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break 124;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = reader.join().unwrap_or_default();
    Ok((
        code,
        start.elapsed(),
        String::from_utf8_lossy(&output).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_and_osc() {
        assert_eq!(
            strip_ansi("\x1b[1;31merror\x1b[0m: x\x1b]9;hi\x07 done\r\n"),
            "error: x done\n"
        );
    }

    #[test]
    fn tail_lines_caps() {
        assert_eq!(tail_lines("a\nb\nc\nd", 2, 100), "c\nd");
    }
}
