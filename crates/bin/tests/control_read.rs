//! T1.3: read methods against a live daemon with a real PTY pane.

mod common;

use common::{eventually, seed_shell_session, Daemon};
use serde_json::json;

#[test]
fn read_methods_see_live_pane_output() {
    let mut d = Daemon::start_with(None, |st| {
        seed_shell_session(st, "for i in $(seq 1 200); do echo line $i; done; sleep 60");
    });
    let mut c = d.client("reader");

    let panes = c.call_raw("panes.list", json!({})).unwrap();
    let panes = panes.as_array().unwrap();
    assert_eq!(panes.len(), 1, "{panes:?}");
    assert_eq!(panes[0]["id"], 1);
    assert_eq!(panes[0]["kind"], "shell");
    assert_eq!(panes[0]["visibility"], "pane");

    let ws = c.call_raw("workspaces.list", json!({})).unwrap();
    assert_eq!(ws[0]["panes"], json!([1]));

    // Wait until the output is complete, then read a short tail.
    let text = eventually(10, || {
        let r = c
            .call_raw("pane.read", json!({"pane": 1, "lines": 5}))
            .unwrap();
        let t = r["text"].as_str().unwrap().to_string();
        t.contains("line 200").then_some(r)
    });
    assert_eq!(
        text["text"],
        "line 196\nline 197\nline 198\nline 199\nline 200"
    );
    assert_eq!(text["truncated"], true);

    // Byte cap applies from the end.
    let r = c
        .call_raw(
            "pane.read",
            json!({"pane": 1, "lines": 50, "max_bytes": 12}),
        )
        .unwrap();
    assert_eq!(r["text"], "199\nline 200");
    assert_eq!(r["truncated"], true);

    let e = c.call_raw("pane.read", json!({"pane": 99})).unwrap_err();
    assert_eq!(e.control().unwrap().code, tag_control::codes::NOT_FOUND);

    let st = c.call_raw("agent.status", json!({"pane": 1})).unwrap();
    assert!(st["status"].is_null());

    let u = c
        .call_raw("usage.summary", json!({"since": "2020-01-01"}))
        .unwrap();
    assert!(u["input"].is_u64());

    let runs = c.call_raw("run.list", json!({})).unwrap();
    assert_eq!(runs, json!([]));

    assert!(d.shutdown().success());
}
