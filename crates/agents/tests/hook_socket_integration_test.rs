use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

use tag_agents::HookServer;
use tag_core::event::{AppEvent, HookEvent, HookPayload};
use tag_core::pane::PaneId;

#[tokio::test]
async fn test_hook_server_socket_reception() {
    let socket_path =
        std::env::temp_dir().join(format!("tag-test-sock-{}.sock", std::process::id()));
    let server = HookServer::new(socket_path.clone());

    let (tx, mut rx) = unbounded_channel::<AppEvent>();
    server.start(tx).await.unwrap();

    // Give socket a brief moment to bind
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connect and send HookPayload
    let payload = HookPayload {
        event: HookEvent::NeedsPermission,
        pane_id: Some(PaneId(42)),
        agent: "claude".into(),
        worktree: None,
        message: "Command execution permission requested".into(),
        timestamp: chrono::Utc::now(),
    };

    let mut stream = UnixStream::connect(&socket_path).expect("Should connect to private socket");
    let json_bytes = serde_json::to_vec(&payload).unwrap();
    stream.write_all(&json_bytes).unwrap();
    stream.flush().unwrap();
    drop(stream);

    // Receive on channel
    let received = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(received.is_ok(), "Must receive hook within timeout");

    if let Ok(Some(AppEvent::AgentHook(received_payload))) = received {
        assert_eq!(received_payload.agent, "claude");
        assert!(matches!(received_payload.event, HookEvent::NeedsPermission));
        assert_eq!(received_payload.pane_id, Some(PaneId(42)));
        assert_eq!(
            received_payload.message,
            "Command execution permission requested"
        );
    } else {
        panic!("Did not receive expected AgentHook event");
    }

    let _ = std::fs::remove_file(socket_path);
}
