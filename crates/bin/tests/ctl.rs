//! Integration tests for `tag ctl` against a fake control server.

use serde_json::json;
use std::path::PathBuf;
use std::process::{Command, Output};
use tag_control::{serve, ServerSettings, TokenRegistry};
use tag_core::config::ControlConfig;
use tag_core::event::AppEvent;
use tokio::sync::{broadcast, mpsc};

struct Fake {
    _dir: tempfile::TempDir,
    sock: PathBuf,
    // Dropping the runtime stops the server.
    _rt: tokio::runtime::Runtime,
    _server: tag_control::ServerHandle,
}

fn fake() -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("tag-control-test.sock");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = rt.block_on(async {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
        let (state_tx, _) = broadcast::channel(64);
        let settings = ServerSettings::new("0.0.0-test", ControlConfig::default());
        let server = serve(
            sock.clone(),
            settings,
            TokenRegistry::default(),
            event_tx,
            state_tx,
        )
        .await
        .unwrap();
        tokio::spawn(async move {
            while let Some(ev) = event_rx.recv().await {
                if let AppEvent::Control(env) = ev {
                    let res = match env.method.as_str() {
                        "panes.list" => Ok(json!([
                            {"id": 1, "title": "main", "kind": "shell", "visibility": "pane", "cwd": "/tmp"},
                            {"id": 7, "title": "worker", "kind": "agent", "profile": "claude",
                             "status": "running", "visibility": "card", "cwd": "/work"}
                        ])),
                        "pane.read" => Ok(json!({"text": "hello from pane", "truncated": false, "status": "running"})),
                        "pane.spawn" => Ok(json!({"pane": 42})),
                        "agent.status" => Ok(json!({"status": "done"})),
                        m => Ok(json!({"echo": m, "params": env.params})),
                    };
                    env.reply.send(res);
                }
            }
        });
        server
    });
    Fake {
        _dir: dir,
        sock,
        _rt: rt,
        _server: server,
    }
}

fn ctl(sock: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tag"))
        .arg("ctl")
        .arg("--socket")
        .arg(sock)
        .args(args)
        .env_remove("TAG_CONTROL_TOKEN")
        .env_remove("TAG_CONTROL_SOCKET")
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

#[test]
fn ls_prints_table() {
    let f = fake();
    let o = ctl(&f.sock, &["ls"]);
    assert!(o.status.success(), "{o:?}");
    let s = out(&o);
    assert!(s.starts_with("ID"), "{s}");
    assert!(s.contains("worker"), "{s}");
    assert!(s.contains("claude"), "{s}");
    assert!(s.contains("/work"), "{s}");
}

#[test]
fn ls_json() {
    let f = fake();
    let o = ctl(&f.sock, &["ls", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v[1]["id"], 7);
}

#[test]
fn read_prints_text() {
    let f = fake();
    let o = ctl(&f.sock, &["read", "7", "--lines", "5"]);
    assert!(o.status.success());
    assert_eq!(out(&o), "hello from pane\n");
}

#[test]
fn spawn_prints_pane_id() {
    let f = fake();
    let o = ctl(
        &f.sock,
        &["spawn", "claude", "--worktree", "w1", "--prompt", "hi"],
    );
    assert!(o.status.success(), "{o:?}");
    assert_eq!(out(&o).trim(), "42");
}

#[test]
fn wait_matches_already_done() {
    let f = fake();
    let o = ctl(&f.sock, &["wait", "7", "--until", "done", "--timeout", "5"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert_eq!(out(&o).trim(), "done");
}

#[test]
fn wait_times_out_with_exit_2() {
    let f = fake();
    let o = ctl(
        &f.sock,
        &["wait", "7", "--until", "failed", "--timeout", "1"],
    );
    assert_eq!(o.status.code(), Some(2), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("code -32006"));
}

#[test]
fn call_raw() {
    let f = fake();
    let o = ctl(&f.sock, &["call", "run.list", r#"{"active_only":true}"#]);
    assert!(o.status.success(), "{o:?}");
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["echo"], "run.list");
    assert_eq!(v["params"]["active_only"], true);
}

#[test]
fn not_running_exits_3() {
    let dir = tempfile::tempdir().unwrap();
    let o = ctl(&dir.path().join("missing.sock"), &["ls"]);
    assert_eq!(o.status.code(), Some(3), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).starts_with("error:"));
}
