//! MCP stdio server tests against a fake control server.

use serde_json::{json, Value};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tag_control::mcp::{run_mcp, McpOptions};
use tag_control::{serve, ServerSettings, TokenRegistry};
use tag_core::config::ControlConfig;
use tag_core::event::AppEvent;
use tokio::sync::{broadcast, mpsc};

async fn start() -> (tempfile::TempDir, PathBuf, tag_control::ServerHandle) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tag-mcp-test.sock");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let (state_tx, _) = broadcast::channel(64);
    let mut settings = ServerSettings::new("0.0.0-test", ControlConfig::default());
    settings.audit_path = Some(dir.path().join("audit.jsonl"));
    let server = serve(
        path.clone(),
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
                let m = env.method.clone();
                env.reply.send(Ok(json!({ "echo": m })));
            }
        }
    });
    (dir, path, server)
}

fn drive(socket: &Path, lines: &[Value]) -> Vec<Value> {
    let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let mut out = Vec::new();
    run_mcp(
        Cursor::new(input),
        &mut out,
        McpOptions {
            client_name: "mcp".into(),
            socket: Some(socket.to_path_buf()),
            token: None,
        },
    )
    .unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn script(call: Value) -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":call}),
        json!({"jsonrpc":"2.0","id":4,"method":"nope"}),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_and_calls_tools() {
    let (_dir, path, _server) = start().await;
    let replies = tokio::task::spawn_blocking(move || {
        drive(
            &path,
            &script(json!({"name":"tag_panes_list","arguments":{}})),
        )
    })
    .await
    .unwrap();
    // The initialized notification gets no reply.
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "tag");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"tag_panes_list"));
    assert!(names.contains(&"tag_pane_spawn"));
    assert!(!names.contains(&"tag_worktree_merge"));
    assert!(!names.contains(&"tag_tag_hello"));
    assert!(!names.contains(&"tag_events_subscribe"));
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(t["inputSchema"]["properties"].is_object());
    }
    let res = &replies[2]["result"];
    assert_eq!(res["isError"], false);
    let text = res["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"echo\": \"panes.list\""), "{text}");
    assert_eq!(replies[3]["error"]["code"], -32601);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_tool_is_invalid_params() {
    let (_dir, path, _server) = start().await;
    let replies = tokio::task::spawn_blocking(move || {
        drive(&path, &script(json!({"name":"tag_worktree_nothing"})))
    })
    .await
    .unwrap();
    assert_eq!(replies[2]["error"]["code"], -32602);
}

#[test]
fn not_running_reports_tool_error() {
    let dir = tempfile::tempdir().unwrap();
    let replies = drive(
        &dir.path().join("missing.sock"),
        &script(json!({"name":"tag_panes_list","arguments":{}})),
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert!(tools.iter().all(|t| t["name"] != "tag_pane_spawn"));
    let res = &replies[2]["result"];
    assert_eq!(res["isError"], true);
    assert_eq!(
        res["content"][0]["text"],
        "TAG is not running. Start `tag` or `tag daemon`."
    );
}
