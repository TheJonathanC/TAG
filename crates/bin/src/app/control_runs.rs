//! Orchestration runs over the control plane (T2.5): run/task methods, scoped tokens, the
//! scheduler loop, merge policy, and conductor notifications.
//!
//! TAG supplies tools and bookkeeping; the conductor model does the planning and reviewing
//! (D2). TAG keeps the conductor informed by typing `[TAG] …` lines into its pane.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tag_control::methods::*;
use tag_control::{ClientCtx, ControlError, ControlResult, ReplyHandle, Role, TokenGrant};
use tag_core::event::{HookEvent, StateEvent};
use tag_core::pane::PaneId;
use tag_orchestrator::{
    next_actions, touches_protected, ConductorRef, Limits, MergePolicy, OrchestrationSettings,
    ProjectConfig, ReportStatus, Run, RunEvent, RunEventKind, RunState, RunStore, SchedulerAction,
    Task, TaskReport, TaskState, TestResult, Visibility,
};

/// A run being orchestrated by this TAG instance.
pub(crate) struct ActiveRun {
    pub(crate) store: RunStore,
    pub(crate) run: Run,
    pub(crate) repo: PathBuf,
    pub(crate) settings: OrchestrationSettings,
    /// Whether "all tasks ready for review" was already announced for the current set.
    pub(crate) announced_all_review: bool,
}

fn respond<T: serde::Serialize>(v: T) -> ControlResult {
    serde_json::to_value(v).map_err(|e| ControlError::internal(e.to_string()))
}

fn orch_err(e: tag_orchestrator::OrchestratorError) -> ControlError {
    use tag_orchestrator::OrchestratorError as E;
    match e {
        E::UnknownTask(_) | E::RunNotFound(_) => ControlError::not_found(e.to_string()),
        E::InvalidRunTransition { .. } | E::InvalidTaskTransition { .. } | E::RunNotEditable(_) => {
            ControlError::conflict(e.to_string())
        }
        E::DuplicateTask(_) | E::UnknownDependency { .. } | E::DependencyCycle(_) => {
            ControlError::invalid_params(e.to_string())
        }
        other => ControlError::internal(other.to_string()),
    }
}

fn snake<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// Dispatch run/task methods. Returns `None` if `method` isn't one of them.
pub(crate) fn handle_runs(
    state: &mut AppState,
    client: &ClientCtx,
    method: &str,
    params: Value,
    reply: ReplyHandle,
) -> Option<()> {
    let result: ControlResult = match method {
        "run.create" => parse_params(params).and_then(|p| run_create(state, client, p)),
        "task.add" => parse_params(params).and_then(|p| task_add(state, client, p)),
        "task.update" => parse_params(params).and_then(|p| task_update(state, client, p)),
        "task.report" => parse_params(params).and_then(|p| task_report(state, client, p)),
        "run.approve_plan" => parse_params(params).and_then(|p| approve_plan(state, client, p)),
        "run.cancel" => parse_params(params).and_then(|p| run_cancel(state, client, p)),
        "_internal.merged" => {
            if let Some(path) = params["worktree"].as_str() {
                on_merged(
                    state,
                    Path::new(path),
                    params["commit"].as_str().unwrap_or(""),
                );
            }
            Ok(Value::Null)
        }
        _ => return None,
    };
    reply.send(result);
    Some(())
}

// ---------------------------------------------------------------------------------------------
// Bookkeeping
// ---------------------------------------------------------------------------------------------

/// Store for a project's runs.
pub(crate) fn run_store_for(state: &AppState, project_id: &str) -> RunStore {
    RunStore::new(
        state
            .paths
            .state_dir
            .join("projects")
            .join(project_id)
            .join("runs"),
    )
}

/// Load non-terminal runs from disk at startup. Panes don't survive a restart, so tasks that
/// were working become `failed` with reason `lost_on_restart`.
pub(crate) fn load_active_runs(state: &mut AppState) {
    for (project_id, store) in run_stores(state) {
        let Ok(list) = store.list() else { continue };
        for mut run in list.runs.into_iter().filter(|r| !r.state.is_terminal()) {
            let Some(repo) = state.projects.get(&project_id).map(|p| p.root.clone()) else {
                continue;
            };
            let mut events = Vec::new();
            let stale: Vec<String> = run
                .tasks
                .iter()
                .filter(|t| t.state.is_active())
                .map(|t| t.id.clone())
                .collect();
            for t in stale {
                if let Ok(ev) =
                    run.transition_task(&t, TaskState::Failed, Some("lost_on_restart".into()))
                {
                    events.push(ev);
                }
                if let Some(task) = run.task_mut(&t) {
                    task.pane = None;
                }
            }
            if let Some(c) = run.conductor.as_mut() {
                c.pane = None;
            }
            for ev in &events {
                let _ = store.append_event(&run.id, ev);
            }
            let _ = store.save(&run);
            let settings = settings_for(&repo, &ProjectConfig::default());
            state.runs.insert(
                run.id.clone(),
                ActiveRun {
                    store: RunStore::new(store_dir(&store, &run.id)),
                    run,
                    repo,
                    settings,
                    announced_all_review: false,
                },
            );
        }
    }
}

fn store_dir(store: &RunStore, run_id: &str) -> PathBuf {
    store
        .run_dir(run_id)
        .ok()
        .and_then(|d| d.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default()
}

fn settings_for(repo: &Path, overrides: &ProjectConfig) -> OrchestrationSettings {
    let project = ProjectConfig::load(repo).ok().flatten().unwrap_or_default();
    OrchestrationSettings::resolve(
        &OrchestrationSettings::builtin(),
        &ProjectConfig::default(),
        &project,
        overrides,
    )
}

/// Persist `events` for `run_id`, save the run, and publish them as [`StateEvent`]s.
fn commit_events(state: &mut AppState, run_id: &str, events: Vec<RunEvent>) {
    let Some(active) = state.runs.get(run_id) else {
        return;
    };
    for ev in &events {
        let _ = active.store.append_event(run_id, ev);
    }
    let _ = active.store.save(&active.run);
    for ev in events {
        let se = match ev.kind {
            RunEventKind::RunState { from, to, .. } => StateEvent::RunStateChanged {
                run: run_id.to_string(),
                from: snake(&from),
                to: snake(&to),
                at: ev.at,
            },
            RunEventKind::TaskState { task, from, to, .. } => StateEvent::TaskStateChanged {
                run: run_id.to_string(),
                task,
                from: snake(&from),
                to: snake(&to),
                at: ev.at,
            },
            _ => continue,
        };
        state.publish(se);
    }
    state.is_dirty = true;
}

/// Mutate a run, then persist and publish the resulting events.
fn with_run<T>(
    state: &mut AppState,
    run_id: &str,
    f: impl FnOnce(&mut ActiveRun, &mut Vec<RunEvent>) -> Result<T, ControlError>,
) -> Result<T, ControlError> {
    let mut events = Vec::new();
    let res = {
        let active = state
            .runs
            .get_mut(run_id)
            .ok_or_else(|| ControlError::not_found(format!("no active run {run_id}")))?;
        f(active, &mut events)
    };
    commit_events(state, run_id, events);
    res
}

/// Type a `[TAG] …` line into the run's conductor pane, if it has one.
fn notify_conductor(state: &mut AppState, run_id: &str, text: &str) {
    let pane = state
        .runs
        .get(run_id)
        .and_then(|a| a.run.conductor.as_ref())
        .and_then(|c| c.pane);
    if let Some(pane) = pane.and_then(|p| state.panes.get_mut(&PaneId(p as u32))) {
        pane_write(pane.as_mut(), &format!("[TAG] {text}"), true);
    }
}

/// Raise an attention item for the human.
fn raise_attention(state: &mut AppState, run_id: &str, pane: Option<PaneId>, message: String) {
    let id = state.attention_queue.push(
        pane,
        format!("run {run_id}"),
        HookEvent::NeedsPermission,
        message.clone(),
        None,
    );
    state.publish(StateEvent::AttentionAdded {
        item: id,
        pane,
        agent: format!("run {run_id}"),
        event: HookEvent::NeedsPermission,
        message,
        at: chrono::Utc::now(),
    });
    state.is_dirty = true;
}

fn require_own_run(client: &ClientCtx, run: &str) -> Result<(), ControlError> {
    if client.role != Role::External && client.run.as_deref() != Some(run) {
        return Err(ControlError::forbidden("token is scoped to another run"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------------------------

/// Built-in prompt templates (`assets/prompts/`).
fn builtin_template(name: &str) -> &'static str {
    match name {
        "conductor" => include_str!("../../../../assets/prompts/conductor.md"),
        "conductor_cli" => include_str!("../../../../assets/prompts/conductor_cli.md"),
        "reviewer" => include_str!("../../../../assets/prompts/reviewer.md"),
        _ => include_str!("../../../../assets/prompts/worker.md"),
    }
}

/// Template `name`: the user's copy in `<config>/prompts/<name>.md`, else the built-in.
pub(crate) fn load_template(state: &AppState, name: &str) -> String {
    let user = state
        .paths
        .config_dir
        .join("prompts")
        .join(format!("{name}.md"));
    std::fs::read_to_string(user).unwrap_or_else(|_| builtin_template(name).to_string())
}

/// Install built-in templates into `<config>/prompts/` without overwriting user edits.
pub(crate) fn install_templates(config_dir: &Path) {
    let dir = config_dir.join("prompts");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for name in ["conductor", "conductor_cli", "worker", "reviewer"] {
        let path = dir.join(format!("{name}.md"));
        if !path.exists() {
            let _ = std::fs::write(path, builtin_template(name));
        }
    }
}

/// Replace `{{var}}` placeholders.
pub(crate) fn render(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

/// The project brief (capped at 4 KB with a pointer to the full file), or a placeholder.
pub(crate) fn project_brief(repo: &Path) -> String {
    let path = repo.join(".tag/brain/brief.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let body = match text.strip_prefix("---") {
                Some(rest) => rest.split_once("\n---").map(|(_, b)| b).unwrap_or(rest),
                None => &text,
            };
            let body = body.trim();
            if body.len() > 4096 {
                let mut cut = 4096;
                while !body.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!(
                    "{}\n\n(truncated; full brief: {})",
                    &body[..cut],
                    path.display()
                )
            } else {
                body.to_string()
            }
        }
        Err(_) => "(no brief yet; run `tag brain init` to generate one)".into(),
    }
}

fn base_vars(active: &ActiveRun) -> HashMap<&'static str, String> {
    let run = &active.run;
    let mut v = HashMap::new();
    v.insert("goal", run.goal.clone());
    v.insert("run_id", run.id.clone());
    v.insert(
        "project_name",
        active.settings.name.clone().unwrap_or_else(|| {
            active
                .repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
    );
    v.insert("brief", project_brief(&active.repo));
    v.insert("workers", run.worker_profiles.join(", "));
    v.insert("merge_policy", snake(&run.merge_policy));
    v.insert("max_workers", run.max_workers.to_string());
    v.insert(
        "test_command",
        active
            .settings
            .test_command
            .clone()
            .unwrap_or_else(|| "the project's tests".into()),
    );
    v
}

// ---------------------------------------------------------------------------------------------
// run.create / conductor
// ---------------------------------------------------------------------------------------------

fn run_create(state: &mut AppState, client: &ClientCtx, p: RunCreateParams) -> ControlResult {
    if client.role != Role::External {
        return Err(ControlError::forbidden("agents cannot create runs"));
    }
    let run = create_run(state, p)?;
    respond(RunCreateResult { run })
}

/// Create (and optionally start the conductor of) a run. Shared with `tag orchestrate`.
pub(crate) fn create_run(state: &mut AppState, p: RunCreateParams) -> Result<Run, ControlError> {
    let repo = repo_for(state, p.project.as_deref())?;
    let project = state.projects.touch(&repo).clone();
    let overrides = ProjectConfig {
        merge_policy: p.merge_policy,
        budget_usd: p.budget,
        default_workers: p.worker_profiles.clone(),
        default_conductor: p.conductor_profile.clone(),
        require_plan_approval: p.require_plan_approval,
        ..Default::default()
    };
    let settings = settings_for(&repo, &overrides);
    let now = chrono::Utc::now();
    let mut run = Run::new(
        tag_orchestrator::new_run_id(now),
        project.id.clone(),
        p.goal,
        now,
    );
    run.merge_policy = settings.merge_policy;
    run.budget_usd = settings.budget_usd;
    run.require_plan_approval = settings.require_plan_approval;
    run.max_workers = settings.max_workers;
    run.worker_profiles = if settings.default_workers.is_empty() {
        vec!["claude".into()]
    } else {
        settings.default_workers.clone()
    };
    let conductor_profile = settings
        .default_conductor
        .clone()
        .unwrap_or_else(|| "claude".into());
    run.conductor = Some(ConductorRef {
        profile: conductor_profile.clone(),
        pane: None,
    });
    let store = run_store_for(state, &project.id);
    store
        .save(&run)
        .map_err(|e| ControlError::internal(e.to_string()))?;
    let id = run.id.clone();
    state.runs.insert(
        id.clone(),
        ActiveRun {
            store,
            run: run.clone(),
            repo,
            settings,
            announced_all_review: false,
        },
    );
    if p.spawn_conductor {
        spawn_conductor(state, &id, &conductor_profile)?;
    }
    Ok(state.runs[&id].run.clone())
}

/// `^Space o`: start a run with project defaults and spawn its conductor.
pub(crate) fn start_run_from_ui(state: &mut AppState, goal: String) {
    let params = RunCreateParams {
        goal,
        project: None,
        conductor_profile: None,
        worker_profiles: None,
        merge_policy: None,
        budget: None,
        spawn_conductor: true,
        require_plan_approval: None,
    };
    match create_run(state, params) {
        Ok(run) => push_toast(
            state,
            "Run Started",
            format!(
                "{}: the conductor is planning. Approve the plan from the attention queue (^Space a).",
                run.id
            ),
            tag_ui::ToastLevel::Success,
            6,
        ),
        Err(e) => push_toast(
            state,
            "Orchestrate Failed",
            e.message,
            tag_ui::ToastLevel::Error,
            6,
        ),
    }
}

/// Spawn the conductor agent for a run with a scoped token and the rendered prompt.
pub(crate) fn spawn_conductor(
    state: &mut AppState,
    run_id: &str,
    profile: &str,
) -> Result<u32, ControlError> {
    let (repo, vars) = {
        let active = &state.runs[run_id];
        (active.repo.clone(), base_vars(active))
    };
    let cfg = state.config.agents.get(profile).cloned();
    let token = state.tokens.mint(TokenGrant {
        role: Role::Conductor,
        run: Some(run_id.to_string()),
        task: None,
        pane: None,
    });
    let mut extras = SpawnExtras {
        env: run_env(state, &token, run_id, None),
        ..Default::default()
    };
    // MCP-capable profiles get a per-run MCP config; others use `tag ctl`.
    let use_mcp = cfg.as_ref().is_some_and(|c| !c.mcp_config_args.is_empty());
    if use_mcp {
        let dir = state
            .runs
            .get(run_id)
            .and_then(|a| a.store.run_dir(run_id).ok())
            .unwrap_or_else(|| state.paths.state_dir.clone());
        let path = dir.join("mcp.json");
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "tag".into());
        let mcp = json!({"mcpServers": {"tag": {
            "command": exe,
            "args": ["mcp", "--client", "conductor"],
            "env": extras.env,
        }}});
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&mcp).unwrap_or_default(),
        )
        .map_err(|e| ControlError::internal(format!("writing {}: {e}", path.display())))?;
        extras.mcp_config_path = Some(path.display().to_string());
    }
    let template = load_template(
        state,
        if use_mcp {
            "conductor"
        } else {
            "conductor_cli"
        },
    );
    let prompt = render(&template, &vars);
    let goal = state.runs[run_id].run.goal.clone();
    let params = SpawnParams {
        profile: profile.to_string(),
        project: None,
        workspace: None,
        cwd: Some(repo.display().to_string()),
        worktree: None,
        prompt: Some(prompt),
        visibility: Visibility::Pane,
        secrets: Vec::new(),
        title: Some(format!("conductor: {}", truncate(&goal, 40))),
        run: Some(run_id.to_string()),
        task: None,
    };
    let ctx = internal_ctx();
    let res = spawn_pane_now(state, &ctx, params, None, extras)?;
    state.tokens.set_pane(&token, res.pane);
    if let Some(active) = state.runs.get_mut(run_id) {
        active.run.conductor = Some(ConductorRef {
            profile: profile.to_string(),
            pane: Some(res.pane as u64),
        });
    }
    commit_events(state, run_id, Vec::new());
    Ok(res.pane)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

pub(crate) fn internal_ctx() -> ClientCtx {
    ClientCtx {
        client: "tag".into(),
        role: Role::External,
        tiers: vec![
            tag_control::Tier::Read,
            tag_control::Tier::Act,
            tag_control::Tier::Dangerous,
        ],
        run: None,
        task: None,
        pane: None,
    }
}

/// Environment for agents launched for a run (D6).
fn run_env(
    state: &AppState,
    token: &str,
    run_id: &str,
    task: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TAG_CONTROL_TOKEN".into(), token.to_string());
    env.insert("TAG_RUN_ID".into(), run_id.to_string());
    if let Some(t) = task {
        env.insert("TAG_TASK_ID".into(), t.to_string());
    }
    if let Some(s) = &state.control_server {
        env.insert("TAG_CONTROL_SOCKET".into(), s.path().display().to_string());
    }
    env
}

// ---------------------------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------------------------

fn task_add(state: &mut AppState, client: &ClientCtx, p: TaskAddParams) -> ControlResult {
    require_own_run(client, &p.run)?;
    let task = with_run(state, &p.run, |active, events| {
        let run = &mut active.run;
        let id =
            p.id.clone()
                .unwrap_or_else(|| format!("t{}", run.tasks.len() + 1));
        let mut task = Task::new(id.clone(), p.title.clone(), p.prompt.clone());
        task.acceptance = p.acceptance.clone();
        task.depends_on = p.depends_on.clone();
        task.parallel_ok = p.parallel_ok;
        task.profile = p
            .profile
            .clone()
            .or_else(|| run.worker_profiles.first().cloned());
        events.push(run.add_task(task).map_err(orch_err)?);
        if !run.require_plan_approval {
            if run.state == RunState::Planning {
                events.push(run.transition(RunState::Running, None).map_err(orch_err)?);
            }
            events.extend(run.queue_planned());
        } else if run.state == RunState::Running {
            // Approved plan extended mid-run: start the new task right away.
            events.extend(run.queue_planned());
        }
        active.announced_all_review = false;
        Ok(run.task(&id).cloned().expect("just added"))
    })?;
    let awaiting = {
        let run = &state.runs[&p.run].run;
        run.require_plan_approval && run.state == RunState::Planning
    };
    if awaiting {
        // First task of a plan that needs approval: ask the human once.
        with_run(state, &p.run, |a, ev| {
            ev.push(
                a.run
                    .transition(RunState::AwaitingApproval, None)
                    .map_err(orch_err)?,
            );
            Ok(())
        })?;
        let pane = state.runs[&p.run]
            .run
            .conductor
            .as_ref()
            .and_then(|c| c.pane)
            .map(|p| PaneId(p as u32));
        let goal = state.runs[&p.run].run.goal.clone();
        raise_attention(
            state,
            &p.run,
            pane,
            format!(
                "Plan for \"{}\" is waiting for approval (`tag ctl call run.approve_plan '{{\"run\":\"{}\"}}'`)",
                truncate(&goal, 60),
                p.run
            ),
        );
    }
    respond(task)
}

fn task_update(state: &mut AppState, client: &ClientCtx, p: TaskUpdateParams) -> ControlResult {
    require_own_run(client, &p.run)?;
    let (task, kill_pane) = with_run(state, &p.run, |active, events| {
        let run = &mut active.run;
        let t = run
            .task_mut(&p.task)
            .ok_or_else(|| ControlError::not_found(format!("no task {}", p.task)))?;
        let editable = matches!(t.state, TaskState::Planned | TaskState::Queued);
        if p.title.is_some() || p.prompt.is_some() || p.acceptance.is_some() || p.profile.is_some()
        {
            if !editable && p.prompt.is_some() {
                return Err(ControlError::conflict(
                    "the prompt can only change before the task starts; send feedback with pane.send",
                ));
            }
            if let Some(v) = &p.title {
                t.title = v.clone();
            }
            if let Some(v) = &p.prompt {
                t.prompt = v.clone();
            }
            if let Some(v) = &p.acceptance {
                t.acceptance = v.clone();
            }
            if let Some(v) = &p.profile {
                t.profile = Some(v.clone());
            }
        }
        let mut kill = None;
        if let Some(next) = p.state {
            let current = run.task(&p.task).map(|t| t.state).expect("exists");
            match next {
                TaskState::ChangesRequested => {
                    events.push(
                        run.transition_task(&p.task, next, p.reason.clone())
                            .map_err(orch_err)?,
                    );
                    // The worker keeps going in the same pane.
                    if run.task(&p.task).and_then(|t| t.pane).is_some() {
                        events.push(
                            run.transition_task(&p.task, TaskState::Working, None)
                                .map_err(orch_err)?,
                        );
                    }
                }
                TaskState::Queued if current == TaskState::Failed => {
                    kill = run.task(&p.task).and_then(|t| t.pane);
                    events.push(
                        run.transition_task(&p.task, next, p.reason.clone())
                            .map_err(orch_err)?,
                    );
                    if let Some(t) = run.task_mut(&p.task) {
                        t.pane = None;
                    }
                }
                TaskState::Skipped | TaskState::Failed => {
                    kill = run.task(&p.task).and_then(|t| t.pane);
                    events.push(
                        run.transition_task(&p.task, next, p.reason.clone())
                            .map_err(orch_err)?,
                    );
                }
                _ => {
                    events.push(
                        run.transition_task(&p.task, next, p.reason.clone())
                            .map_err(orch_err)?,
                    );
                }
            }
        }
        Ok((run.task(&p.task).cloned().expect("exists"), kill))
    })?;
    if let Some(pane) = kill_pane {
        close_pane(state, PaneId(pane as u32));
    }
    respond(task)
}

fn task_report(state: &mut AppState, client: &ClientCtx, p: TaskReportParams) -> ControlResult {
    let (run_id, task_id) = match client.role {
        Role::Worker => (
            client.run.clone().unwrap_or_default(),
            client.task.clone().unwrap_or_default(),
        ),
        _ => (
            p.run
                .clone()
                .ok_or_else(|| ControlError::invalid_params("run is required"))?,
            p.task
                .clone()
                .ok_or_else(|| ControlError::invalid_params("task is required"))?,
        ),
    };
    let (title, pane) = with_run(state, &run_id, |active, events| {
        let run = &mut active.run;
        let t = run
            .task_mut(&task_id)
            .ok_or_else(|| ControlError::not_found(format!("no task {task_id}")))?;
        t.report = Some(TaskReport {
            summary: p.summary.clone(),
            status: p.status,
            notes: p.notes.clone(),
            at: chrono::Utc::now(),
        });
        let (title, pane, current) = (t.title.clone(), t.pane, t.state);
        let next = match p.status {
            ReportStatus::Done => TaskState::Review,
            ReportStatus::Blocked => TaskState::NeedsInput,
        };
        if current != next && current.can_transition_to(next) {
            events.push(
                run.transition_task(&task_id, next, None)
                    .map_err(orch_err)?,
            );
        }
        Ok((title, pane))
    })?;
    match p.status {
        ReportStatus::Done => {
            notify_conductor(
                state,
                &run_id,
                &format!(
                    "task {task_id} ({title}) is ready for review: {}",
                    one_line(&p.summary)
                ),
            );
            announce_all_review(state, &run_id);
        }
        ReportStatus::Blocked => {
            notify_conductor(
                state,
                &run_id,
                &format!(
                    "task {task_id} ({title}) is blocked: {}",
                    one_line(&p.summary)
                ),
            );
            raise_attention(
                state,
                &run_id,
                pane.map(|p| PaneId(p as u32)),
                format!("Task {task_id} is blocked: {}", one_line(&p.summary)),
            );
        }
    }
    respond(Empty {})
}

fn one_line(s: &str) -> String {
    truncate(&s.split_whitespace().collect::<Vec<_>>().join(" "), 300)
}

/// Tell the conductor once when every unsettled task is waiting for review.
fn announce_all_review(state: &mut AppState, run_id: &str) {
    let ready = state.runs.get(run_id).is_some_and(|a| {
        let open: Vec<_> = a
            .run
            .tasks
            .iter()
            .filter(|t| !t.state.is_settled())
            .collect();
        !a.announced_all_review
            && !open.is_empty()
            && open
                .iter()
                .all(|t| matches!(t.state, TaskState::Review | TaskState::Approved))
    });
    if ready {
        if let Some(a) = state.runs.get_mut(run_id) {
            a.announced_all_review = true;
        }
        notify_conductor(
            state,
            run_id,
            &format!("run {run_id}: all tasks are ready for review"),
        );
    }
}

fn approve_plan(state: &mut AppState, client: &ClientCtx, p: RunRef) -> ControlResult {
    if client.role == Role::Conductor {
        let needs = state
            .runs
            .get(&p.run)
            .is_some_and(|a| a.run.require_plan_approval);
        if needs {
            return Err(ControlError::forbidden(
                "the plan must be approved by the human",
            ));
        }
    }
    require_own_run(client, &p.run)?;
    let run = with_run(state, &p.run, |active, events| {
        let run = &mut active.run;
        if run.tasks.is_empty() {
            return Err(ControlError::policy_blocked("the plan has no tasks yet"));
        }
        if matches!(run.state, RunState::Planning | RunState::AwaitingApproval) {
            events.push(
                run.transition(RunState::Running, Some("plan approved".into()))
                    .map_err(orch_err)?,
            );
        } else if run.state == RunState::Paused {
            // Resuming a run paused at its budget: the budget no longer stops it.
            run.budget_usd = 0.0;
            events.push(
                run.transition(RunState::Running, Some("resumed by the human".into()))
                    .map_err(orch_err)?,
            );
        }
        events.extend(run.queue_planned());
        Ok(run.clone())
    })?;
    state
        .attention_queue
        .clear_matching(|i| i.agent == format!("run {}", p.run));
    notify_conductor(
        state,
        &p.run,
        &format!("plan approved for run {}; workers are starting", p.run),
    );
    schedule(state, &p.run);
    respond(run)
}

fn run_cancel(state: &mut AppState, client: &ClientCtx, p: RunCancelParams) -> ControlResult {
    require_own_run(client, &p.run)?;
    let (run, panes) = with_run(state, &p.run, |active, events| {
        let run = &mut active.run;
        if !run.state.is_terminal() {
            events.push(
                run.transition(RunState::Cancelled, Some("cancelled".into()))
                    .map_err(orch_err)?,
            );
        }
        let mut panes: Vec<u64> = run.tasks.iter().filter_map(|t| t.pane).collect();
        panes.extend(run.conductor.as_ref().and_then(|c| c.pane));
        Ok((run.clone(), panes))
    })?;
    if p.kill_workers {
        for pane in panes {
            if state.panes.contains_key(&PaneId(pane as u32)) {
                close_pane(state, PaneId(pane as u32));
            }
        }
    }
    state.tokens.revoke_run(&p.run);
    state.runs.remove(&p.run);
    respond(run)
}

// ---------------------------------------------------------------------------------------------
// Merges
// ---------------------------------------------------------------------------------------------

/// Find the run and task whose worktree is `worktree`.
fn task_for_worktree(state: &AppState, worktree: &Path) -> Option<(String, String)> {
    let want = canonical(worktree);
    state.runs.iter().find_map(|(id, a)| {
        a.run.tasks.iter().find_map(|t| {
            let wt = t.worktree.as_ref()?;
            (canonical(Path::new(wt)) == want).then(|| (id.clone(), t.id.clone()))
        })
    })
}

/// Merge policy for conductor tokens: `auto` runs merge directly unless the diff touches a
/// protected path; `ask` runs queue the merge for the human. `Some` = answered here.
pub(crate) fn run_merge_gate(
    state: &mut AppState,
    client: &ClientCtx,
    _p: &WorktreeMergeParams,
    worktree: &Path,
) -> Option<ControlResult> {
    let Some((run_id, task_id)) = task_for_worktree(state, worktree) else {
        return Some(Err(ControlError::forbidden(
            "not a task worktree of your run",
        )));
    };
    if client.run.as_deref() != Some(run_id.as_str()) {
        return Some(Err(ControlError::forbidden("task belongs to another run")));
    }
    let (policy, repo, protected, branch) = {
        let a = &state.runs[&run_id];
        let t = a.run.task(&task_id)?;
        (
            a.run.merge_policy,
            a.repo.clone(),
            a.settings.protected_paths.clone(),
            t.branch.clone().unwrap_or_default(),
        )
    };
    let touched = if protected.is_empty() {
        Vec::new()
    } else {
        main_branch(&repo)
            .and_then(|base| {
                git(
                    &repo,
                    &["diff", "--name-only", &format!("{base}...{branch}")],
                )
            })
            .map(|out| {
                let files: Vec<String> = out.lines().map(String::from).collect();
                touches_protected(&files, &protected)
            })
            .unwrap_or_default()
    };
    if policy == MergePolicy::Auto && touched.is_empty() {
        return None;
    }
    // Queue for the human: approve the task and raise an attention item.
    let _ = with_run(state, &run_id, |a, ev| {
        if a.run.task(&task_id).map(|t| t.state) == Some(TaskState::Review) {
            ev.push(
                a.run
                    .transition_task(&task_id, TaskState::Approved, None)
                    .map_err(orch_err)?,
            );
        }
        Ok(())
    });
    let why = if touched.is_empty() {
        "merge policy is \"ask\"".to_string()
    } else {
        format!("it touches protected paths: {}", touched.join(", "))
    };
    raise_attention(
        state,
        &run_id,
        None,
        format!(
            "Merge task {task_id} of run {run_id}? ({why}) `tag ctl call worktree.merge '{{\"run\":\"{run_id}\",\"task\":\"{task_id}\"}}'`"
        ),
    );
    Some(respond(MergeResult {
        merged_commit: String::new(),
        queued: true,
    }))
}

/// After any successful merge: mark the task merged and continue the run.
pub(crate) fn on_merged(state: &mut AppState, worktree: &Path, commit: &str) {
    let Some((run_id, task_id)) = task_for_worktree(state, worktree) else {
        return;
    };
    let _ = with_run(state, &run_id, |a, ev| {
        let run = &mut a.run;
        let cur = run.task(&task_id).map(|t| t.state);
        if matches!(
            cur,
            Some(TaskState::Review) | Some(TaskState::NeedsInput) | Some(TaskState::Working)
        ) {
            if cur != Some(TaskState::Review) {
                let _ = run
                    .transition_task(&task_id, TaskState::Review, None)
                    .map(|e| ev.push(e));
            }
            let _ = run
                .transition_task(&task_id, TaskState::Approved, None)
                .map(|e| ev.push(e));
        }
        if run.task(&task_id).map(|t| t.state) == Some(TaskState::Approved) {
            ev.push(
                run.transition_task(&task_id, TaskState::Merged, Some(commit.to_string()))
                    .map_err(orch_err)?,
            );
        }
        Ok(())
    });
    let pane = state.runs[&run_id].run.task(&task_id).and_then(|t| t.pane);
    if let Some(p) = pane {
        // The worker is finished; keep its pane around only if it's visible.
        if pane_visibility(state, PaneId(p as u32)) == Visibility::Hidden {
            close_pane(state, PaneId(p as u32));
        }
    }
    notify_conductor(
        state,
        &run_id,
        &format!("task {task_id} merged ({})", &commit[..commit.len().min(8)]),
    );
    schedule(state, &run_id);
}

/// Record a test result on the task that owns `worktree`.
pub(crate) fn record_task_test(state: &mut AppState, worktree: &Path, rec: &TestRecord) {
    if let Some((run_id, task_id)) = task_for_worktree(state, worktree) {
        let _ = with_run(state, &run_id, |a, _| {
            if let Some(t) = a.run.task_mut(&task_id) {
                t.last_test = Some(TestResult {
                    commit: rec.commit.clone(),
                    exit_code: rec.exit_code,
                    at: rec.at,
                });
            }
            Ok(())
        });
    }
}

/// Resolve `{worktree}` or `{run, task}` to a worktree name/path.
pub(crate) fn resolve_task_worktree(
    state: &AppState,
    worktree: Option<&str>,
    run: Option<&str>,
    task: Option<&str>,
) -> Result<String, ControlError> {
    if let Some(w) = worktree {
        return Ok(w.to_string());
    }
    let (Some(run), Some(task)) = (run, task) else {
        return Err(ControlError::invalid_params(
            "give worktree, or run and task",
        ));
    };
    let r = match state.runs.get(run) {
        Some(a) => a.run.clone(),
        None => load_run(state, run)?.1,
    };
    r.task(task)
        .ok_or_else(|| ControlError::not_found(format!("no task {task}")))?
        .worktree
        .clone()
        .ok_or_else(|| ControlError::not_found(format!("task {task} has no worktree yet")))
}

// ---------------------------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------------------------

/// Charge usage events to runs: event session id → pane (TAG-chosen session id) → run.
/// Returns the runs whose budget was newly exceeded.
pub(crate) fn attribute_usage(
    state: &mut AppState,
    events: &[tag_usage::UsageEvent],
) -> Vec<String> {
    if state.runs.is_empty() || events.is_empty() {
        return Vec::new();
    }
    let by_session: HashMap<String, String> = state
        .pane_meta
        .values()
        .filter_map(|m| Some((m.session_id.clone()?, m.run.clone()?)))
        .collect();
    if by_session.is_empty() {
        return Vec::new();
    }
    let pricing = tag_usage::PricingConfig::default();
    let mut charges: HashMap<String, f64> = HashMap::new();
    for ev in events {
        if let Some(run) = by_session.get(&ev.session_id) {
            *charges.entry(run.clone()).or_default() += pricing.estimate_cost(ev);
        }
    }
    let mut exceeded = Vec::new();
    for (run, usd) in charges {
        let newly = with_run(state, &run, |a, _| Ok(a.run.add_cost(usd))).unwrap_or(false);
        if newly {
            exceeded.push(run.clone());
        }
        schedule(state, &run);
    }
    exceeded
}

// ---------------------------------------------------------------------------------------------
// Scheduler loop
// ---------------------------------------------------------------------------------------------

/// Called on every Tick: follow worker panes, then run the scheduler for each active run.
pub(crate) fn orchestrate_tick(state: &mut AppState) {
    if state.runs.is_empty() {
        return;
    }
    let ids: Vec<String> = state.runs.keys().cloned().collect();
    for id in ids {
        follow_workers(state, &id);
        schedule(state, &id);
    }
}

/// Map worker pane status to task state.
fn follow_workers(state: &mut AppState, run_id: &str) {
    let Some(active) = state.runs.get(run_id) else {
        return;
    };
    if active.run.state.is_terminal() {
        return;
    }
    let mut changes: Vec<(String, TaskState, Option<String>)> = Vec::new();
    for t in &active.run.tasks {
        let Some(pane) = t.pane else { continue };
        let pid = PaneId(pane as u32);
        let status = state
            .panes
            .get(&pid)
            .map(|p| p.agent_status().unwrap_or("running"));
        match (t.state, status) {
            (TaskState::Working | TaskState::NeedsInput, None) => changes.push((
                t.id.clone(),
                TaskState::Failed,
                Some("worker pane exited".into()),
            )),
            (TaskState::Working, Some("done")) => {
                changes.push((t.id.clone(), TaskState::Review, None))
            }
            (TaskState::Working, Some("awaiting-input")) => {
                changes.push((t.id.clone(), TaskState::NeedsInput, None))
            }
            (TaskState::Working, Some("failed")) => changes.push((
                t.id.clone(),
                TaskState::Failed,
                Some("worker failed".into()),
            )),
            (TaskState::NeedsInput, Some("running"))
                if t.report.as_ref().map(|r| r.status) != Some(ReportStatus::Blocked) =>
            {
                changes.push((t.id.clone(), TaskState::Working, None))
            }
            _ => {}
        }
    }
    if changes.is_empty() {
        return;
    }
    let applied: Vec<(String, TaskState, String)> = with_run(state, run_id, |a, ev| {
        let mut done = Vec::new();
        for (task, next, reason) in changes {
            if let Ok(e) = a.run.transition_task(&task, next, reason) {
                ev.push(e);
                let title = a
                    .run
                    .task(&task)
                    .map(|t| t.title.clone())
                    .unwrap_or_default();
                done.push((task, next, title));
            }
        }
        Ok(done)
    })
    .unwrap_or_default();
    for (task, next, title) in applied {
        let msg = match next {
            TaskState::Review => format!("task {task} ({title}) finished and is ready for review"),
            TaskState::NeedsInput => format!("task {task} ({title}) is waiting for input"),
            TaskState::Failed => format!("task {task} ({title}) failed"),
            _ => continue,
        };
        notify_conductor(state, run_id, &msg);
    }
    announce_all_review(state, run_id);
}

/// Run the pure scheduler for one run and carry out its actions.
fn schedule(state: &mut AppState, run_id: &str) {
    let Some(active) = state.runs.get(run_id) else {
        return;
    };
    let mut limits = Limits {
        max_workers: active.run.max_workers,
        budget_usd: active.run.budget_usd,
        ..Default::default()
    };
    for (name, cfg) in &state.config.agents {
        if cfg.max_concurrent > 0 {
            limits
                .max_concurrent_by_profile
                .insert(name.clone(), cfg.max_concurrent);
        }
    }
    let actions = next_actions(&active.run, &limits);
    for action in actions {
        match action {
            SchedulerAction::Spawn { task } => {
                if let Err(e) = spawn_worker(state, run_id, &task) {
                    let _ = with_run(state, run_id, |a, ev| {
                        if let Ok(x) =
                            a.run
                                .transition_task(&task, TaskState::Failed, Some(e.message.clone()))
                        {
                            ev.push(x);
                        }
                        Ok(())
                    });
                    notify_conductor(
                        state,
                        run_id,
                        &format!("task {task} could not start: {}", e.message),
                    );
                }
            }
            SchedulerAction::PauseForBudget => {
                let hard = state
                    .runs
                    .get(run_id)
                    .is_some_and(|a| a.settings.hard_budget);
                let killed: Vec<u64> = with_run(state, run_id, |a, ev| {
                    ev.push(
                        a.run
                            .transition(RunState::Paused, Some("budget reached".into()))
                            .map_err(orch_err)?,
                    );
                    let mut killed = Vec::new();
                    if hard {
                        let active: Vec<String> = a
                            .run
                            .tasks
                            .iter()
                            .filter(|t| t.state.is_active())
                            .map(|t| t.id.clone())
                            .collect();
                        for t in active {
                            if let Ok(e) = a.run.transition_task(
                                &t,
                                TaskState::Failed,
                                Some("budget reached".into()),
                            ) {
                                ev.push(e);
                            }
                            if let Some(p) = a.run.task(&t).and_then(|t| t.pane) {
                                killed.push(p);
                            }
                        }
                    }
                    Ok(killed)
                })
                .unwrap_or_default();
                for p in killed {
                    if state.panes.contains_key(&PaneId(p as u32)) {
                        close_pane(state, PaneId(p as u32));
                    }
                }
                let (spent, budget) = state
                    .runs
                    .get(run_id)
                    .map(|a| (a.run.spent_usd, a.run.budget_usd))
                    .unwrap_or_default();
                raise_attention(
                    state,
                    run_id,
                    None,
                    format!(
                        "Run {run_id} paused: budget reached (${spent:.2} of ${budget:.2}){}. Resume with run.approve_plan.",
                        if hard { "; workers stopped" } else { "; running workers continue" }
                    ),
                );
                notify_conductor(
                    state,
                    run_id,
                    &format!("run {run_id} is paused: budget reached"),
                );
            }
            SchedulerAction::MarkRunDone => {
                let _ = with_run(state, run_id, |a, ev| {
                    ev.push(a.run.transition(RunState::Done, None).map_err(orch_err)?);
                    Ok(())
                });
                notify_conductor(
                    state,
                    run_id,
                    &format!("run {run_id} is done; write your summary"),
                );
                state.tokens.revoke_run_workers(run_id);
            }
            // Runs stay `running` while tasks are reviewed; done when all settle.
            SchedulerAction::EnterReviewing => {}
        }
    }
}

/// Start a worker for `task`: worktree, scoped token, rendered prompt, pane.
fn spawn_worker(state: &mut AppState, run_id: &str, task_id: &str) -> Result<(), ControlError> {
    let (repo, run, mut vars, visibility) = {
        let a = &state.runs[run_id];
        (
            a.repo.clone(),
            a.run.clone(),
            base_vars(a),
            a.settings.worker_visibility,
        )
    };
    let task = run
        .task(task_id)
        .cloned()
        .ok_or_else(|| ControlError::not_found(format!("no task {task_id}")))?;
    let name = tag_orchestrator::task_worktree_name(&run, task_id);
    let name = name.trim_start_matches(".worktrees/").to_string();
    let branch = tag_orchestrator::task_branch(&run, task_id);
    let wt = ensure_worktree(&repo, &name, &branch, None)?;
    let profile = task
        .profile
        .clone()
        .or_else(|| run.worker_profiles.first().cloned())
        .unwrap_or_else(|| "claude".into());
    let token = state.tokens.mint(TokenGrant {
        role: Role::Worker,
        run: Some(run_id.to_string()),
        task: Some(task_id.to_string()),
        pane: None,
    });
    vars.insert("task", format!("{}\n\n{}", task.title, task.prompt));
    vars.insert(
        "acceptance",
        if task.acceptance.is_empty() {
            "- (none given)".into()
        } else {
            task.acceptance
                .iter()
                .map(|a| format!("- {a}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
    );
    vars.insert("worktree", wt.path.clone());
    let prompt = render(&load_template(state, "worker"), &vars);
    let params = SpawnParams {
        profile,
        project: None,
        workspace: None,
        cwd: Some(wt.path.clone()),
        worktree: None,
        prompt: Some(prompt),
        visibility,
        secrets: Vec::new(),
        title: Some(format!("{task_id}: {}", truncate(&task.title, 30))),
        run: Some(run_id.to_string()),
        task: Some(task_id.to_string()),
    };
    let extras = SpawnExtras {
        env: run_env(state, &token, run_id, Some(task_id)),
        ..Default::default()
    };
    let res = spawn_pane_now(state, &internal_ctx(), params, Some(wt.clone()), extras);
    let res = match res {
        Ok(r) => r,
        Err(e) => {
            state.tokens.revoke(&token);
            return Err(e);
        }
    };
    state.tokens.set_pane(&token, res.pane);
    with_run(state, run_id, |a, ev| {
        if let Some(t) = a.run.task_mut(task_id) {
            t.pane = Some(res.pane as u64);
            t.worktree = Some(wt.path.clone());
            t.branch = Some(branch.clone());
        }
        ev.push(
            a.run
                .transition_task(task_id, TaskState::Working, None)
                .map_err(orch_err)?,
        );
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::tests::test_state;

    fn usage(session: &str, tokens: u64) -> tag_usage::UsageEvent {
        tag_usage::UsageEvent {
            agent: "claude".into(),
            session_id: session.into(),
            step_id: None,
            project: None,
            model: "claude-sonnet-4".into(),
            input_tokens: tokens,
            output_tokens: tokens,
            cache_read: 0,
            cache_write: 0,
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn usage_is_charged_to_the_run_and_pauses_at_budget() {
        let (mut state, _rx) = test_state();
        let mut run = Run::new("r_test_0001", "p", "goal", chrono::Utc::now());
        run.state = RunState::Running;
        run.budget_usd = 1.0;
        let mut t = Task::new("t1", "x", "y");
        t.state = TaskState::Working;
        t.pane = Some(7);
        run.tasks.push(t);
        state.runs.insert(
            run.id.clone(),
            ActiveRun {
                store: RunStore::new(state.paths.state_dir.join("runs")),
                run,
                repo: state.paths.state_dir.clone(),
                settings: OrchestrationSettings::builtin(),
                announced_all_review: false,
            },
        );
        state.pane_meta.insert(
            PaneId(7),
            PaneMeta {
                run: Some("r_test_0001".into()),
                session_id: Some("sess-1".into()),
                ..Default::default()
            },
        );
        // Another session's usage isn't charged.
        assert!(attribute_usage(&mut state, &[usage("other", 10_000_000)]).is_empty());
        assert_eq!(state.runs["r_test_0001"].run.spent_usd, 0.0);

        let small = attribute_usage(&mut state, &[usage("sess-1", 1_000)]);
        assert!(small.is_empty());
        let spent = state.runs["r_test_0001"].run.spent_usd;
        assert!(spent > 0.0 && spent < 1.0, "{spent}");
        assert_eq!(state.runs["r_test_0001"].run.state, RunState::Running);

        let big = attribute_usage(&mut state, &[usage("sess-1", 10_000_000)]);
        assert_eq!(big, vec!["r_test_0001".to_string()]);
        let a = &state.runs["r_test_0001"];
        assert_eq!(a.run.state, RunState::Paused);
        // Soft budget: the running worker keeps its task.
        assert_eq!(a.run.tasks[0].state, TaskState::Working);
        assert!(state
            .attention_queue
            .items()
            .iter()
            .any(|i| i.message.contains("budget")));
    }

    #[test]
    fn session_uuid_is_v4() {
        let u = new_session_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }

    #[test]
    fn render_replaces_vars() {
        let mut v = HashMap::new();
        v.insert("goal", "ship it".to_string());
        assert_eq!(
            render("Goal: {{goal}} {{other}}", &v),
            "Goal: ship it {{other}}"
        );
    }

    #[test]
    fn templates_have_their_variables() {
        for name in ["conductor", "conductor_cli", "worker"] {
            let t = builtin_template(name);
            assert!(t.contains("{{run_id}}"), "{name}");
        }
        assert!(builtin_template("worker").contains("{{acceptance}}"));
    }
}
