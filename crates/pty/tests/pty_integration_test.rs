use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

use tag_core::event::AppEvent;
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_pty::PtyPane;

#[tokio::test]
async fn test_real_pty_spawn_and_output() {
    let (tx, mut rx) = unbounded_channel::<AppEvent>();
    let pane_id = PaneId(999);

    let pane = PtyPane::spawn(
        pane_id,
        "Test Shell".into(),
        PaneKind::Shell,
        Some("sh".into()),
        &["-c".into(), "echo TAG_HELLO_WORLD".into()],
        &HashMap::new(),
        None,
        80,
        24,
        1000,
        tx,
    );

    assert!(pane.is_ok(), "Real PTY spawn must succeed");
    let mut pane = pane.unwrap();

    let mut output_received = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(AppEvent::PtyOutput { pane_id: id, data })) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            if id == pane_id {
                let text = String::from_utf8_lossy(&data);
                if text.contains("TAG_HELLO_WORLD") {
                    output_received = true;
                    break;
                }
            }
        }
    }

    assert!(output_received, "Must receive real stdout from PTY process");

    // Test resize
    pane.on_resize(120, 40);
}
