use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use tag_orchestrator::*;

fn tmpdir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tag-orch-{}-{}-{}",
        tag,
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn run() -> Run {
    Run::new(
        "r_20261123_0915_ab12",
        "proj",
        "goal",
        Utc.with_ymd_and_hms(2026, 11, 23, 9, 15, 0).unwrap(),
    )
}

fn running_run() -> Run {
    let mut r = run();
    r.transition(RunState::Running, None).unwrap();
    r
}

fn task(id: &str, deps: &[&str]) -> Task {
    let mut t = Task::new(id, id, "do it");
    t.depends_on = deps.iter().map(|s| s.to_string()).collect();
    t
}

fn limits(max: usize) -> Limits {
    Limits {
        max_workers: max,
        ..Default::default()
    }
}

fn spawns(a: &[SchedulerAction]) -> Vec<String> {
    a.iter()
        .filter_map(|a| match a {
            SchedulerAction::Spawn { task } => Some(task.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn run_transition_table_exhaustive() {
    use RunState::*;
    let allowed: &[(RunState, RunState)] = &[
        (Planning, AwaitingApproval),
        (Planning, Running),
        (AwaitingApproval, Running),
        (AwaitingApproval, Planning),
        (Running, Reviewing),
        (Running, Merging),
        (Running, Done),
        (Running, Paused),
        (Reviewing, Running),
        (Reviewing, Merging),
        (Reviewing, Done),
        (Reviewing, Paused),
        (Merging, Running),
        (Merging, Reviewing),
        (Merging, Done),
        (Merging, Paused),
        (Paused, Running),
        (Paused, Reviewing),
        (Paused, Merging),
        (Planning, Failed),
        (Planning, Cancelled),
        (AwaitingApproval, Failed),
        (AwaitingApproval, Cancelled),
        (Running, Failed),
        (Running, Cancelled),
        (Reviewing, Failed),
        (Reviewing, Cancelled),
        (Merging, Failed),
        (Merging, Cancelled),
        (Paused, Failed),
        (Paused, Cancelled),
    ];
    for from in RunState::ALL {
        for to in RunState::ALL {
            assert_eq!(
                from.can_transition_to(to),
                allowed.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }
}

#[test]
fn task_transition_table_exhaustive() {
    use TaskState::*;
    let mut allowed: Vec<(TaskState, TaskState)> = vec![
        (Planned, Queued),
        (Queued, Working),
        (Working, NeedsInput),
        (Working, Review),
        (NeedsInput, Working),
        (Review, Approved),
        (Review, ChangesRequested),
        (ChangesRequested, Working),
        (Approved, Merged),
        (Approved, ChangesRequested),
        (Failed, Queued),
    ];
    for from in TaskState::ALL {
        if !from.is_terminal() {
            if from != Failed {
                allowed.push((from, Failed));
            }
            allowed.push((from, Skipped));
        }
    }
    for from in TaskState::ALL {
        for to in TaskState::ALL {
            assert_eq!(
                from.can_transition_to(to),
                allowed.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }
}

#[test]
fn run_transition_updates_and_pause_restores_previous() {
    let mut r = running_run();
    r.transition(RunState::Reviewing, None).unwrap();
    let ev = r
        .transition(RunState::Paused, Some("budget".into()))
        .unwrap();
    assert!(matches!(
        ev.kind,
        RunEventKind::RunState {
            from: RunState::Reviewing,
            to: RunState::Paused,
            ..
        }
    ));
    assert_eq!(r.pause_reason.as_deref(), Some("budget"));
    assert!(r.transition(RunState::Merging, None).is_err());
    r.transition(RunState::Reviewing, None).unwrap();
    assert_eq!(r.pause_reason, None);
    assert!(r.transition(RunState::Done, None).is_ok());
    assert!(r.transition(RunState::Running, None).is_err());
}

#[test]
fn failed_run_records_reason() {
    let mut r = running_run();
    r.transition(RunState::Failed, Some("boom".into())).unwrap();
    assert_eq!(r.failure_reason.as_deref(), Some("boom"));
}

#[test]
fn task_transition_and_retry() {
    let mut r = running_run();
    r.add_task(task("t1", &[])).unwrap();
    r.queue_planned();
    r.transition_task("t1", TaskState::Working, None).unwrap();
    assert_eq!(r.task("t1").unwrap().attempts, 1);
    r.transition_task("t1", TaskState::Failed, Some("x".into()))
        .unwrap();
    assert_eq!(r.task("t1").unwrap().failure_reason.as_deref(), Some("x"));
    r.transition_task("t1", TaskState::Queued, None).unwrap();
    assert_eq!(r.task("t1").unwrap().failure_reason, None);
    assert!(r.transition_task("t1", TaskState::Review, None).is_err());
    assert!(matches!(
        r.transition_task("nope", TaskState::Queued, None),
        Err(OrchestratorError::UnknownTask(_))
    ));
}

#[test]
fn add_task_validation() {
    let mut r = run();
    r.add_task(task("a", &[])).unwrap();
    assert!(matches!(
        r.add_task(task("a", &[])),
        Err(OrchestratorError::DuplicateTask(_))
    ));
    assert!(matches!(
        r.add_task(task("b", &["zzz"])),
        Err(OrchestratorError::UnknownDependency { .. })
    ));
    assert!(matches!(
        r.add_task(task("c", &["c"])),
        Err(OrchestratorError::DependencyCycle(_))
    ));
    r.add_task(task("d", &["a"])).unwrap();
    // A cycle among existing tasks (injected directly) is detected.
    r.task_mut("a").unwrap().depends_on.push("e".into());
    assert!(matches!(
        r.add_task(task("e", &["d"])),
        Err(OrchestratorError::DependencyCycle(_))
    ));
    r.transition(RunState::Running, None).unwrap();
    r.transition(RunState::Done, None).unwrap();
    assert!(matches!(
        r.add_task(task("z", &[])),
        Err(OrchestratorError::RunNotEditable(_))
    ));
}

#[test]
fn scheduler_dependency_order() {
    let mut r = running_run();
    r.add_task(task("t1", &[])).unwrap();
    r.add_task(task("t2", &["t1"])).unwrap();
    r.queue_planned();
    let l = limits(4);
    assert_eq!(spawns(&next_actions(&r, &l)), vec!["t1"]);
    r.transition_task("t1", TaskState::Working, None).unwrap();
    assert!(next_actions(&r, &l).is_empty());
    r.transition_task("t1", TaskState::Review, None).unwrap();
    r.transition_task("t1", TaskState::Approved, None).unwrap();
    assert!(spawns(&next_actions(&r, &l)).is_empty());
    r.transition_task("t1", TaskState::Merged, None).unwrap();
    assert_eq!(spawns(&next_actions(&r, &l)), vec!["t2"]);
}

#[test]
fn scheduler_parallel_ok_allows_approved() {
    let mut r = running_run();
    let mut t1 = task("t1", &[]);
    t1.parallel_ok = true;
    r.add_task(t1).unwrap();
    r.add_task(task("t2", &["t1"])).unwrap();
    r.queue_planned();
    for s in [TaskState::Working, TaskState::Review, TaskState::Approved] {
        r.transition_task("t1", s, None).unwrap();
    }
    assert_eq!(spawns(&next_actions(&r, &limits(4))), vec!["t2"]);
}

#[test]
fn scheduler_max_workers_cap() {
    let mut r = running_run();
    for i in 1..=4 {
        r.add_task(task(&format!("t{i}"), &[])).unwrap();
    }
    r.queue_planned();
    assert_eq!(spawns(&next_actions(&r, &limits(2))), vec!["t1", "t2"]);
    r.transition_task("t1", TaskState::Working, None).unwrap();
    assert_eq!(spawns(&next_actions(&r, &limits(2))), vec!["t2"]);
    r.transition_task("t1", TaskState::NeedsInput, None)
        .unwrap();
    assert_eq!(spawns(&next_actions(&r, &limits(2))), vec!["t2"]);
}

#[test]
fn scheduler_per_profile_cap() {
    let mut r = running_run();
    for i in 1..=3 {
        let mut t = task(&format!("t{i}"), &[]);
        t.profile = Some("agy".into());
        r.add_task(t).unwrap();
    }
    let mut other = task("t4", &[]);
    other.profile = Some("claude".into());
    r.add_task(other).unwrap();
    r.queue_planned();
    let l = Limits {
        max_workers: 10,
        max_concurrent_by_profile: HashMap::from([("agy".to_string(), 1)]),
        budget_usd: 0.0,
    };
    assert_eq!(spawns(&next_actions(&r, &l)), vec!["t1", "t4"]);
}

#[test]
fn scheduler_budget_pause_and_not_running() {
    let mut r = running_run();
    r.add_task(task("t1", &[])).unwrap();
    r.queue_planned();
    r.budget_usd = 1.0;
    let l = Limits {
        max_workers: 4,
        budget_usd: 1.0,
        ..Default::default()
    };
    assert!(!r.add_cost(0.5));
    assert_eq!(spawns(&next_actions(&r, &l)), vec!["t1"]);
    assert!(r.add_cost(0.6));
    assert!(!r.add_cost(0.1));
    assert_eq!(next_actions(&r, &l), vec![SchedulerAction::PauseForBudget]);
    r.transition(RunState::Paused, None).unwrap();
    assert!(next_actions(&r, &l).is_empty());
}

#[test]
fn scheduler_done_and_reviewing() {
    let mut r = running_run();
    assert!(next_actions(&r, &limits(4)).is_empty());
    r.add_task(task("t1", &[])).unwrap();
    r.add_task(task("t2", &[])).unwrap();
    r.queue_planned();
    r.transition_task("t1", TaskState::Working, None).unwrap();
    r.transition_task("t1", TaskState::Review, None).unwrap();
    r.transition_task("t2", TaskState::Skipped, None).unwrap();
    assert_eq!(
        next_actions(&r, &limits(4)),
        vec![SchedulerAction::EnterReviewing]
    );
    r.transition_task("t1", TaskState::Approved, None).unwrap();
    r.transition_task("t1", TaskState::Merged, None).unwrap();
    assert_eq!(
        next_actions(&r, &limits(4)),
        vec![SchedulerAction::MarkRunDone]
    );
}

#[test]
fn ids_and_names() {
    let now = Utc.with_ymd_and_hms(2026, 11, 23, 9, 15, 0).unwrap();
    let id = new_run_id(now);
    assert!(id.starts_with("r_20261123_0915_"), "{id}");
    assert_eq!(id.len(), "r_20261123_0915_ab12".len());
    assert!(id[16..]
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    let r = run();
    assert_eq!(r.short_id(), "r_ab12");
    assert_eq!(task_worktree_name(&r, "t1"), ".worktrees/r_ab12-t1");
    assert_eq!(task_branch(&r, "t1"), "tag/r_ab12/t1");
}

#[test]
fn example_fixture_parses() {
    let text = include_str!("fixtures/run_example.json");
    let r: Run = serde_json::from_str(text).unwrap();
    assert_eq!(r.id, "r_20261123_0915_ab12");
    assert_eq!(r.state, RunState::Planning);
    assert_eq!(r.conductor.as_ref().unwrap().pane, Some(12));
    assert_eq!(r.budget_usd, 5.0);
    assert_eq!(r.spent_usd, 0.0);
    assert_eq!(r.tasks[0].state, TaskState::Working);
    assert_eq!(r.tasks[0].last_test.as_ref().unwrap().exit_code, 0);
    assert!(r.tasks[0].report.is_none());
    assert_eq!(r.max_workers, 4);
}

#[test]
fn event_serde_tagged() {
    let mut r = running_run();
    let ev = r.add_task(task("t1", &[])).unwrap();
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["type"], "task_added");
    let back: RunEvent = serde_json::from_value(v).unwrap();
    assert_eq!(back, ev);
}

#[test]
fn persistence_round_trip() {
    let dir = tmpdir("rt");
    let store = RunStore::new(&dir);
    let mut r = running_run();
    let e1 = r.add_task(task("t1", &[])).unwrap();
    store.save(&r).unwrap();
    store.append_event(&r.id, &e1).unwrap();
    let e2 = r.transition(RunState::Reviewing, None).unwrap();
    store.append_event(&r.id, &e2).unwrap();
    assert!(dir.join(&r.id).join("logs").is_dir());
    assert_eq!(store.load(&r.id).unwrap().state, RunState::Running);
    store.save(&r).unwrap();
    assert_eq!(store.load(&r.id).unwrap(), r);
    assert_eq!(store.events(&r.id).unwrap(), vec![e1, e2]);
    let l = store.list().unwrap();
    assert_eq!(l.runs.len(), 1);
    assert!(l.warnings.is_empty());
    assert!(matches!(
        store.load("missing"),
        Err(OrchestratorError::RunNotFound(_))
    ));
    assert!(store.load("../x").is_err());
}

#[test]
fn list_skips_corrupt_with_warning() {
    let dir = tmpdir("list");
    let store = RunStore::new(&dir);
    store.save(&running_run()).unwrap();
    std::fs::create_dir_all(dir.join("bad")).unwrap();
    std::fs::write(dir.join("bad/run.json"), "{ not json").unwrap();
    let l = store.list().unwrap();
    assert_eq!(l.runs.len(), 1);
    assert_eq!(l.warnings.len(), 1);
    assert!(l.warnings[0].contains("bad"));
}

#[test]
fn crash_safety_leftover_tmp_does_not_corrupt() {
    let dir = tmpdir("crash");
    let store = RunStore::new(&dir);
    let mut r = running_run();
    store.save(&r).unwrap();
    let good = r.clone();
    let run_dir = dir.join(&r.id);

    // Simulated crash mid-write: garbage in run.json.tmp, never renamed.
    std::fs::write(run_dir.join("run.json.tmp"), "{\"id\": \"r_trunc").unwrap();
    assert_eq!(store.load(&r.id).unwrap(), good);

    // A subsequent save overwrites the stale tmp and succeeds.
    r.goal = "changed".into();
    store.save(&r).unwrap();
    assert_eq!(store.load(&r.id).unwrap().goal, "changed");

    // Torn trailing event line is ignored.
    let ev = r.add_task(task("t1", &[])).unwrap();
    store.append_event(&r.id, &ev).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(run_dir.join("events.jsonl"))
        .unwrap();
    use std::io::Write;
    f.write_all(b"{\"at\":\"2026").unwrap();
    assert_eq!(store.events(&r.id).unwrap(), vec![ev]);
}

#[test]
fn project_config_load_and_precedence() {
    let dir = tmpdir("cfg");
    assert!(ProjectConfig::load(&dir).unwrap().is_none());
    std::fs::create_dir_all(dir.join(".tag")).unwrap();
    std::fs::write(
        dir.join(".tag/project.toml"),
        r#"
name = "tag"
test_command = "cargo test --workspace"
default_workers = ["agy-neon"]
merge_policy = "auto"
max_workers = 3
worker_visibility = "pane"
budget_usd = 5.0
protected_paths = [".github/", "AGENTS.md"]
"#,
    )
    .unwrap();
    let project = ProjectConfig::load(&dir).unwrap().unwrap();
    assert_eq!(project.max_workers, Some(3));
    assert_eq!(project.require_plan_approval, None);

    let global = ProjectConfig {
        max_workers: Some(8),
        require_plan_approval: Some(false),
        budget_usd: Some(1.0),
        merge_policy: Some(MergePolicy::Ask),
        ..Default::default()
    };
    let overrides = ProjectConfig {
        budget_usd: Some(0.0),
        ..Default::default()
    };
    let builtin = OrchestrationSettings::builtin();
    assert_eq!(builtin.max_workers, 4);
    assert!(builtin.require_plan_approval);
    assert_eq!(builtin.merge_policy, MergePolicy::Ask);
    assert_eq!(builtin.worker_visibility, Visibility::Card);

    let s = OrchestrationSettings::resolve(&builtin, &global, &project, &overrides);
    assert_eq!(s.budget_usd, 0.0); // override
    assert_eq!(s.max_workers, 3); // project beats global
    assert_eq!(s.merge_policy, MergePolicy::Auto); // project beats global
    assert!(!s.require_plan_approval); // global beats builtin
    assert_eq!(s.worker_visibility, Visibility::Pane);
    assert_eq!(s.default_workers, vec!["agy-neon"]);
    assert_eq!(s.protected_paths, vec![".github/", "AGENTS.md"]);

    let empty = ProjectConfig::default();
    assert_eq!(
        OrchestrationSettings::resolve(&builtin, &empty, &empty, &empty),
        builtin
    );
}

#[test]
fn protected_paths_prefix_match() {
    let paths: Vec<String> = [".github/workflows/ci.yml", "src/main.rs", "AGENTS.md"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let prot = vec![".github/".to_string(), "AGENTS.md".to_string()];
    assert_eq!(
        touches_protected(&paths, &prot),
        vec![".github/workflows/ci.yml", "AGENTS.md"]
    );
    assert!(touches_protected(&paths, &[]).is_empty());
}
