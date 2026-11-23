//! Control server tests against a fake app loop.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tag_control::methods::CATALOG;
use tag_control::{
    codes, serve, AsyncControlClient, ClientError, Role, ServerSettings, StateEvent, Tier,
    TokenGrant, TokenRegistry,
};
use tag_core::config::{ControlClientConfig, ControlConfig};
use tag_core::event::AppEvent;
use tag_core::pane::PaneId;
use tokio::sync::{broadcast, mpsc};

struct Harness {
    _dir: tempfile::TempDir,
    path: PathBuf,
    audit: PathBuf,
    tokens: TokenRegistry,
    state_tx: broadcast::Sender<StateEvent>,
    _server: tag_control::ServerHandle,
}

/// Start a server whose "app loop" echoes the method name and reports pane 5 as `running`.
async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tag-control-test.sock");
    let audit = dir.path().join("audit.jsonl");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let (state_tx, _) = broadcast::channel(64);
    let mut control = ControlConfig::default();
    control.clients.insert(
        "reader".into(),
        ControlClientConfig {
            tiers: vec![Tier::Read],
        },
    );
    let mut settings = ServerSettings::new("0.0.0-test", control);
    settings.audit_path = Some(audit.clone());
    let tokens = TokenRegistry::default();
    let server = serve(
        path.clone(),
        settings,
        tokens.clone(),
        event_tx,
        state_tx.clone(),
    )
    .await
    .unwrap();
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if let AppEvent::Control(env) = ev {
                let res = match env.method.as_str() {
                    "agent.status" => {
                        if env.params["pane"] == 404 {
                            Err(tag_control::ControlError::not_found("no pane"))
                        } else {
                            Ok(json!({"status": "running"}))
                        }
                    }
                    m => Ok(json!({ "echo": m })),
                };
                env.reply.send(res);
            }
        }
    });
    Harness {
        _dir: dir,
        path,
        audit,
        tokens,
        state_tx,
        _server: server,
    }
}

async fn raw(path: &PathBuf) -> tokio::net::UnixStream {
    tokio::net::UnixStream::connect(path).await.unwrap()
}

async fn roundtrip(stream: &mut tokio::net::UnixStream, line: &str) -> Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    stream.write_all(line.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    let mut reader = tokio::io::BufReader::new(stream);
    let mut out = String::new();
    reader.read_line(&mut out).await.unwrap();
    serde_json::from_str(&out).unwrap()
}

fn code(e: ClientError) -> i64 {
    e.control().map(|c| c.code).unwrap_or(0)
}

#[tokio::test]
async fn first_call_must_be_hello() {
    let h = harness().await;
    let mut s = raw(&h.path).await;
    let v = roundtrip(
        &mut s,
        r#"{"jsonrpc":"2.0","id":1,"method":"panes.list","params":{}}"#,
    )
    .await;
    assert_eq!(v["error"]["code"], codes::NOT_AUTHENTICATED);
}

#[tokio::test]
async fn malformed_line_keeps_connection_open() {
    let h = harness().await;
    let mut c = AsyncControlClient::connect(&h.path, "ctl", None)
        .await
        .unwrap();
    let err = c.send_line("{not json").await.unwrap().unwrap_err();
    assert_eq!(err.code, codes::PARSE_ERROR);
    let v = c.call_raw("panes.list", json!({})).await.unwrap();
    assert_eq!(v["echo"], "panes.list");
}

#[tokio::test]
async fn oversized_line_is_rejected() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let h = harness().await;
    let mut s = raw(&h.path).await;
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tag.hello","params":{{"client":"{}"}}}}"#,
        "x".repeat(1024 * 1024 + 10)
    );
    let _ = s.write_all(big.as_bytes()).await;
    let _ = s.write_all(b"\n").await;
    let mut reader = tokio::io::BufReader::new(&mut s);
    let mut out = String::new();
    reader.read_line(&mut out).await.unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["code"], codes::INVALID_REQUEST);
    // Then the server closes the connection.
    out.clear();
    assert_eq!(reader.read_line(&mut out).await.unwrap(), 0);
}

#[tokio::test]
async fn invalid_token_is_rejected() {
    let h = harness().await;
    let err = AsyncControlClient::connect(&h.path, "x", Some("tagt_bogus".into()))
        .await
        .err()
        .unwrap();
    assert_eq!(code(err), codes::NOT_AUTHENTICATED);
}

/// Table-driven permission test: every method × every client kind.
#[tokio::test]
async fn permission_table() {
    let h = harness().await;
    let conductor = h.tokens.mint(TokenGrant {
        role: Role::Conductor,
        run: Some("r_1".into()),
        task: None,
        pane: Some(9),
    });
    let worker = h.tokens.mint(TokenGrant {
        role: Role::Worker,
        run: Some("r_1".into()),
        task: Some("t1".into()),
        pane: Some(5),
    });
    let cases: Vec<(&str, Option<String>, Role, Vec<Tier>)> = vec![
        ("reader", None, Role::External, vec![Tier::Read]),
        ("stranger", None, Role::External, vec![Tier::Read]),
        ("mcp", None, Role::External, vec![Tier::Read, Tier::Act]),
        (
            "ctl",
            None,
            Role::External,
            vec![Tier::Read, Tier::Act, Tier::Dangerous],
        ),
        (
            "c",
            Some(conductor),
            Role::Conductor,
            vec![Tier::Read, Tier::Act, Tier::Dangerous],
        ),
        ("w", Some(worker), Role::Worker, vec![Tier::Read, Tier::Act]),
    ];
    for (client, token, role, tiers) in cases {
        let mut c = AsyncControlClient::connect(&h.path, client, token)
            .await
            .unwrap();
        let hello = c.hello.clone().unwrap();
        assert_eq!(hello.role, role, "{client}");
        assert_eq!(hello.tiers, tiers, "{client}");
        for m in CATALOG.iter().filter(|m| m.tier.is_some()) {
            if m.name == "events.subscribe" {
                continue; // turns the connection into a subscriber; covered elsewhere
            }
            // Params that pass scope checks for scoped tokens.
            let params = match m.name {
                "agent.wait" => json!({"pane": 5, "until": ["running"]}),
                _ => json!({"pane": 5, "run": "r_1", "task": "t1"}),
            };
            let allowed = tiers.contains(&m.tier.unwrap()) && m.roles.contains(&role);
            let res = c.call_raw(m.name, params).await;
            if allowed {
                let v = res.unwrap_or_else(|e| panic!("{client} {}: {e}", m.name));
                if m.name == "agent.wait" {
                    assert_eq!(v["reason"], "already");
                } else if m.name == "agent.status" {
                    assert_eq!(v["status"], "running");
                } else {
                    assert_eq!(v["echo"], m.name);
                }
            } else {
                assert_eq!(
                    code(res.unwrap_err()),
                    codes::FORBIDDEN,
                    "{client} {}",
                    m.name
                );
            }
        }
    }
    // Scoped tokens can't reach outside their run/pane.
    let worker2 = h.tokens.mint(TokenGrant {
        role: Role::Worker,
        run: Some("r_1".into()),
        task: Some("t1".into()),
        pane: Some(5),
    });
    let mut w = AsyncControlClient::connect(&h.path, "w", Some(worker2))
        .await
        .unwrap();
    let e = w
        .call_raw("pane.read", json!({"pane": 6}))
        .await
        .unwrap_err();
    assert_eq!(code(e), codes::FORBIDDEN);
    let e = w
        .call_raw("tests.run", json!({"worktree": "x", "run": "r_2"}))
        .await
        .unwrap_err();
    assert_eq!(code(e), codes::FORBIDDEN);

    // Act/dangerous calls were audited; reads were not.
    let audit = std::fs::read_to_string(&h.audit).unwrap();
    assert!(audit
        .lines()
        .any(|l| l.contains("\"method\":\"pane.spawn\"")));
    assert!(!audit
        .lines()
        .any(|l| l.contains("\"method\":\"panes.list\"")));
}

#[tokio::test]
async fn agent_wait_follows_broadcast_and_times_out() {
    let h = harness().await;
    let mut c = AsyncControlClient::connect(&h.path, "mcp", None)
        .await
        .unwrap();
    let tx = h.state_tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = tx.send(StateEvent::PaneStatusChanged {
            pane: PaneId(5),
            from: Some("running".into()),
            to: "awaiting_input".into(),
            at: chrono::Utc::now(),
        });
    });
    let v = c
        .call_raw("agent.wait", json!({"pane": 5, "timeout_s": 5}))
        .await
        .unwrap();
    assert_eq!(v["status"], "awaiting_input");
    assert_eq!(v["reason"], "status");

    let e = c
        .call_raw(
            "agent.wait",
            json!({"pane": 5, "until": ["done"], "timeout_s": 1}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(e), codes::TIMEOUT);

    let e = c
        .call_raw("agent.wait", json!({"pane": 404, "timeout_s": 1}))
        .await
        .unwrap_err();
    assert_eq!(code(e), codes::NOT_FOUND);
}

#[tokio::test]
async fn subscription_delivers_filtered_events() {
    let h = harness().await;
    let mut c = AsyncControlClient::connect(&h.path, "reader", None)
        .await
        .unwrap();
    c.call_raw("events.subscribe", json!({"kinds": ["pane_exited"]}))
        .await
        .unwrap();
    let now = chrono::Utc::now();
    h.state_tx
        .send(StateEvent::PaneStatusChanged {
            pane: PaneId(1),
            from: None,
            to: "idle".into(),
            at: now,
        })
        .unwrap();
    h.state_tx
        .send(StateEvent::PaneExited {
            pane: PaneId(1),
            exit_code: Some(0),
            at: now,
        })
        .unwrap();
    let ev = c.next_event(Duration::from_secs(2)).await.unwrap().unwrap();
    assert_eq!(ev.kind(), "pane_exited");
}
