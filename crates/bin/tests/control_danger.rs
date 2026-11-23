//! T1.5: merge (clean, conflict, stale tests), worktree.remove and pane.kill.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit_file(dir: &Path, file: &str, body: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", &format!("edit {file}")]);
}

#[test]
fn merge_policy_conflicts_and_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let mut d = Daemon::start_in(None, Some(&repo), |_| {});
    let mut c = d.client("ctl");
    let code = |e: tag_control::ClientError| e.control().unwrap().code;

    // --- clean merge, gated on tests at HEAD ---
    let a = c.call_raw("worktree.create", json!({"name": "a"})).unwrap();
    let a_path = std::path::PathBuf::from(a["path"].as_str().unwrap());
    commit_file(&a_path, "a.txt", "from a\n");

    let e = c
        .call_raw("worktree.merge", json!({"worktree": "a"}))
        .unwrap_err();
    assert_eq!(code(e), tag_control::codes::POLICY_BLOCKED, "no tests yet");

    c.call_raw("tests.run", json!({"worktree": "a", "command": "true"}))
        .unwrap();
    commit_file(&a_path, "a2.txt", "more\n");
    let e = c
        .call_raw("worktree.merge", json!({"worktree": "a"}))
        .unwrap_err();
    assert_eq!(
        code(e),
        tag_control::codes::POLICY_BLOCKED,
        "tests are stale"
    );

    c.call_raw("tests.run", json!({"worktree": "a", "command": "true"}))
        .unwrap();
    let m = c
        .call_raw("worktree.merge", json!({"worktree": "a"}))
        .unwrap();
    assert_eq!(m["merged_commit"].as_str().unwrap(), head(&repo));
    assert!(repo.join("a2.txt").exists());

    // --- conflict: aborted, repo clean ---
    let b = c.call_raw("worktree.create", json!({"name": "b"})).unwrap();
    let b_path = std::path::PathBuf::from(b["path"].as_str().unwrap());
    commit_file(&b_path, "README.md", "b's version\n");
    commit_file(&repo, "README.md", "main's version\n");
    let before = head(&repo);
    let e = c
        .call_raw(
            "worktree.merge",
            json!({"worktree": "b", "require_tests": false}),
        )
        .unwrap_err();
    let err = e.control().unwrap().clone();
    assert_eq!(err.code, tag_control::codes::CONFLICT);
    assert_eq!(err.data.unwrap()["files"], json!(["README.md"]));
    assert_eq!(head(&repo), before);
    assert!(
        git(&repo, &["status", "--porcelain", "--untracked-files=no"])
            .trim()
            .is_empty()
    );
    assert!(!repo.join(".git/MERGE_HEAD").exists());

    // --- remove ---
    let e = c
        .call_raw(
            "worktree.remove",
            json!({"worktree": repo.display().to_string()}),
        )
        .unwrap_err();
    assert_eq!(code(e), tag_control::codes::POLICY_BLOCKED);
    c.call_raw("worktree.remove", json!({"worktree": "a"}))
        .unwrap();
    assert!(!a_path.exists());

    // --- kill; mcp (read+act) can't ---
    let p = c
        .call_raw("pane.spawn", json!({"profile": "shell"}))
        .unwrap();
    let mut m = d.client("mcp");
    let e = m
        .call_raw("pane.kill", json!({"pane": p["pane"]}))
        .unwrap_err();
    assert_eq!(code(e), tag_control::codes::FORBIDDEN);
    c.call_raw("pane.kill", json!({"pane": p["pane"]})).unwrap();
    assert_eq!(c.call_raw("panes.list", json!({})).unwrap(), json!([]));

    assert!(d.shutdown().success());
}
