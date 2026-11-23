//! T2.5 acceptance: a fake conductor plans two tasks with `tag ctl`, fake workers implement and
//! report them, and merges follow the run's merge policy.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

const CONDUCTOR: &str = r#"
ctl task.add {"run":"${env:TAG_RUN_ID}","title":"one","prompt":"write t1","acceptance":["t1.txt exists"],"profile":"fakew"}
ctl task.add {"run":"${env:TAG_RUN_ID}","title":"two","prompt":"write t2","profile":"fakew"}
wait_input all tasks are ready for review
ctl tests.run {"run":"${env:TAG_RUN_ID}","task":"t1"}
ctl tests.run {"run":"${env:TAG_RUN_ID}","task":"t2"}
ctl worktree.merge {"run":"${env:TAG_RUN_ID}","task":"t1"}
ctl worktree.merge {"run":"${env:TAG_RUN_ID}","task":"t2"}
print MERGES REQUESTED
wait_input is done
print CONDUCTOR FINISHED
sleep 60000
"#;

const WORKER: &str = r#"
print worker ${env:TAG_TASK_ID}
write ${env:TAG_TASK_ID}.txt hello from ${env:TAG_TASK_ID}
commit work ${env:TAG_TASK_ID}
ctl task.report {"summary":"wrote ${env:TAG_TASK_ID}.txt","status":"done"}
sleep 60000
"#;

fn setup(tmp: &Path) -> (std::path::PathBuf, String) {
    let repo = tmp.join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join(".tag")).unwrap();
    std::fs::write(
        repo.join(".tag/project.toml"),
        "test_command = \"true\"\nworker_visibility = \"hidden\"\n",
    )
    .unwrap();
    let c = tmp.join("conductor.script");
    let w = tmp.join("worker.script");
    std::fs::write(&c, CONDUCTOR).unwrap();
    std::fs::write(&w, WORKER).unwrap();
    let profile = |name: &str, script: &Path| {
        format!(
            r#"
[agents.{name}]
name = "{name}"
command = "{bin}"
args = ["--script", "{script}"]
prompt_delivery = "arg"
prompt_args = ["{{prompt}}"]

[agents.{name}.env]
FAKE_AGENT_TAG_BIN = "{tag}"
FAKE_AGENT_NOTIFY_BIN = "{notify}"
"#,
            bin = fake_agent_bin().display(),
            script = script.display(),
            tag = env!("CARGO_BIN_EXE_tag"),
            notify = notify_bin().display(),
        )
    };
    (repo, profile("fakec", &c) + &profile("fakew", &w))
}

fn run_state(c: &mut tag_control::ControlClient, run: &str) -> Value {
    c.call_raw("run.get", json!({ "run": run })).unwrap()
}

fn wait_run(
    c: &mut tag_control::ControlClient,
    run: &str,
    secs: u64,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = Value::Null;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        last = run_state(c, run);
        if pred(&last) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("run never reached the expected state; last: {last:#}");
}

fn task_states(r: &Value) -> Vec<String> {
    r["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["state"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn auto_merge_run_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, config) = setup(tmp.path());
    let mut d = Daemon::start_in(Some(&config), Some(&repo), |_| {});
    let mut c = d.client("ctl");

    let r = c
        .call_raw(
            "run.create",
            json!({"goal": "write two files", "conductor_profile": "fakec",
                   "worker_profiles": ["fakew"], "merge_policy": "auto", "spawn_conductor": true}),
        )
        .unwrap();
    let run = r["run"]["id"].as_str().unwrap().to_string();

    wait_run(&mut c, &run, 20, |r| {
        r["state"] == "awaiting_approval" && r["tasks"].as_array().unwrap().len() == 2
    });
    // Workers can't start before approval.
    assert_eq!(
        task_states(&run_state(&mut c, &run)),
        vec!["planned", "planned"]
    );
    c.call_raw("run.approve_plan", json!({ "run": run }))
        .unwrap();

    let done = wait_run(&mut c, &run, 45, |r| r["state"] == "done");
    assert_eq!(task_states(&done), vec!["merged", "merged"]);
    assert!(repo.join("t1.txt").exists() && repo.join("t2.txt").exists());
    assert_eq!(done["tasks"][0]["report"]["summary"], "wrote t1.txt");
    assert_eq!(done["tasks"][0]["last_test"]["exit_code"], 0);
    assert!(done["tasks"][0]["branch"]
        .as_str()
        .unwrap()
        .starts_with("tag/r_"));

    let conductor = done["conductor"]["pane"].as_u64().unwrap();
    eventually(10, || {
        let t = c
            .call_raw("pane.read", json!({"pane": conductor, "lines": 200}))
            .unwrap();
        t["text"]
            .as_str()
            .unwrap()
            .contains("CONDUCTOR FINISHED")
            .then_some(())
    });
    assert!(d.shutdown().success());
}

#[test]
fn ask_policy_queues_merges_for_the_human() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, config) = setup(tmp.path());
    let mut d = Daemon::start_in(Some(&config), Some(&repo), |_| {});
    let mut c = d.client("ctl");
    let r = c
        .call_raw(
            "run.create",
            json!({"goal": "write two files", "conductor_profile": "fakec",
                   "worker_profiles": ["fakew"], "merge_policy": "ask", "spawn_conductor": true}),
        )
        .unwrap();
    let run = r["run"]["id"].as_str().unwrap().to_string();
    wait_run(&mut c, &run, 20, |r| {
        r["state"] == "awaiting_approval" && r["tasks"].as_array().unwrap().len() == 2
    });
    c.call_raw("run.approve_plan", json!({ "run": run }))
        .unwrap();

    // The conductor's merges are queued: tasks approved, nothing merged, run still running.
    let r = wait_run(&mut c, &run, 45, |r| {
        task_states(r) == vec!["approved", "approved"]
    });
    assert_eq!(r["state"], "running");
    assert!(!repo.join("t1.txt").exists());

    // The human merges.
    for t in ["t1", "t2"] {
        let m = c
            .call_raw("worktree.merge", json!({"run": run, "task": t}))
            .unwrap();
        assert_eq!(m["queued"], false);
    }
    let done = wait_run(&mut c, &run, 20, |r| r["state"] == "done");
    assert_eq!(task_states(&done), vec!["merged", "merged"]);
    assert!(repo.join("t2.txt").exists());
    assert!(d.shutdown().success());
}

/// T2.6: `tag orchestrate` starts a run (here without plan approval) that completes.
#[test]
fn tag_orchestrate_cli_runs_to_completion() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, config) = setup(tmp.path());
    let mut d = Daemon::start_in(Some(&config), Some(&repo), |_| {});
    let out = tag_cmd(&d.home)
        .env("TAG_CONTROL_SOCKET", d.control_socket())
        .args([
            "orchestrate",
            "write two files",
            "--conductor",
            "fakec",
            "--workers",
            "fakew",
            "--merge",
            "auto",
            "--yes",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let run = v["run"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["run"]["require_plan_approval"], false);
    let mut c = d.client("ctl");
    let done = wait_run(&mut c, &run, 45, |r| r["state"] == "done");
    assert_eq!(task_states(&done), vec!["merged", "merged"]);
    assert!(d.shutdown().success());
}
