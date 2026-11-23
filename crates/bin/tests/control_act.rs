//! T1.4: act methods end to end with the fake agent.

mod common;

use common::*;
use serde_json::json;

#[test]
fn spawn_wait_and_test_in_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    std::fs::create_dir_all(repo.join(".tag")).unwrap();
    std::fs::write(
        repo.join(".tag/project.toml"),
        "test_command = \"test -f done.txt && echo PASSED\"\n",
    )
    .unwrap();
    let script = tmp.path().join("agent.script");
    std::fs::write(
        &script,
        "print working\nwrite done.txt ok\ncommit worker change\nnotify TaskDone\nsleep 60000\n",
    )
    .unwrap();

    let config = fake_agent_config(&script);
    let mut d = Daemon::start_in(Some(&config), Some(&repo), |_| {});
    let mut c = d.client("ctl");

    let r = c
        .call_raw(
            "pane.spawn",
            json!({"profile": "fake", "worktree": {"name": "w1"}, "prompt": "do the thing", "visibility": "hidden"}),
        )
        .unwrap();
    let pane = r["pane"].as_u64().unwrap();
    let wt_path = r["worktree"]["path"].as_str().unwrap().to_string();
    assert!(wt_path.ends_with(".worktrees/w1"), "{wt_path}");

    let w = c
        .call_raw(
            "agent.wait",
            json!({"pane": pane, "until": ["done"], "timeout_s": 20}),
        )
        .unwrap();
    assert_eq!(w["status"], "done");

    let text = c.call_raw("pane.read", json!({"pane": pane})).unwrap();
    let text = text["text"].as_str().unwrap();
    assert!(text.contains("PROMPT: do the thing"), "{text}");
    assert!(text.contains("working"), "{text}");

    let panes = c.call_raw("panes.list", json!({})).unwrap();
    let p = &panes.as_array().unwrap()[0];
    assert_eq!(p["visibility"], "hidden");
    assert_eq!(p["profile"], "fake");
    assert!(p["worktree"].as_str().unwrap().ends_with("w1"));

    let t = c.call_raw("tests.run", json!({"worktree": "w1"})).unwrap();
    assert_eq!(t["exit_code"], 0, "{t}");
    assert_eq!(t["commit"], head(std::path::Path::new(&wt_path)));
    assert!(t["tail"].as_str().unwrap().contains("PASSED"));

    let t = c
        .call_raw("tests.run", json!({"worktree": "w1", "command": "exit 3"}))
        .unwrap();
    assert_eq!(t["exit_code"], 3);

    let diff = c
        .call_raw("worktree.diff", json!({"worktree": "w1"}))
        .unwrap();
    assert!(
        diff["stat"].as_str().unwrap().contains("done.txt"),
        "{diff}"
    );

    let wts = c.call_raw("worktree.list", json!({})).unwrap();
    assert_eq!(wts.as_array().unwrap().len(), 2, "{wts}");

    // .worktrees/ is git-excluded in the main repo
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&status.stdout).contains(".worktrees"));

    assert!(d.shutdown().success());
}

#[test]
fn shell_pane_send_and_visibility() {
    let tmp = tempfile::tempdir().unwrap();
    let mut d = Daemon::start_in(None, Some(tmp.path()), |_| {});
    let mut c = d.client("mcp");
    let r = c
        .call_raw(
            "pane.spawn",
            json!({"profile": "shell", "visibility": "card"}),
        )
        .unwrap();
    let pane = r["pane"].as_u64().unwrap();
    c.call_raw(
        "pane.send",
        json!({"pane": pane, "text": "echo hello-$((40+2))"}),
    )
    .unwrap();
    eventually(10, || {
        let t = c.call_raw("pane.read", json!({"pane": pane})).unwrap();
        t["text"]
            .as_str()
            .unwrap()
            .contains("hello-42")
            .then_some(())
    });
    let p = c.call_raw("panes.list", json!({})).unwrap();
    assert_eq!(p[0]["visibility"], "card");
    c.call_raw(
        "pane.set_visibility",
        json!({"pane": pane, "visibility": "pane"}),
    )
    .unwrap();
    let p = c.call_raw("panes.list", json!({})).unwrap();
    assert_eq!(p[0]["visibility"], "pane");

    let e = c
        .call_raw("pane.spawn", json!({"profile": "nope"}))
        .unwrap_err();
    assert_eq!(e.control().unwrap().code, tag_control::codes::NOT_FOUND);

    assert!(d.shutdown().success());
}
