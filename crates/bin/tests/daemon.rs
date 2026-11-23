//! T1.0: the headless daemon serves hook and control sockets and shuts down cleanly.

mod common;

use common::Daemon;
use std::io::Write;
use std::time::Duration;

#[test]
fn daemon_serves_hooks_and_shuts_down_cleanly() {
    let mut d = Daemon::start(None);
    let mut c = d.client("ctl");
    assert_eq!(c.hello().role, tag_control::Role::External);
    c.call_raw(
        "events.subscribe",
        serde_json::json!({"kinds": ["attention_added"]}),
    )
    .unwrap();

    let payload = serde_json::json!({
        "event": "NeedsInput",
        "pane_id": null,
        "agent": "fake",
        "worktree": null,
        "message": "hello from test",
        "timestamp": chrono::Utc::now(),
    });
    let mut s = std::os::unix::net::UnixStream::connect(d.hook_socket()).unwrap();
    s.write_all(payload.to_string().as_bytes()).unwrap();
    drop(s);

    let ev = c
        .next_event(Some(Duration::from_secs(10)))
        .unwrap()
        .expect("attention event");
    match ev {
        tag_control::StateEvent::AttentionAdded { message, .. } => {
            assert_eq!(message, "hello from test")
        }
        other => panic!("unexpected {other:?}"),
    }

    let state_dir = d.state_dir.clone();
    let control = d.control_socket();
    let status = d.shutdown();
    assert!(status.success(), "daemon exit: {status:?}");
    let has_project_session = std::fs::read_dir(state_dir.join("projects"))
        .unwrap()
        .flatten()
        .any(|e| e.path().join("session.json").exists());
    assert!(has_project_session, "per-project session saved on exit");
    assert!(!control.exists(), "control socket removed on exit");
}
