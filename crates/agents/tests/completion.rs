use std::collections::HashMap;
use std::time::Duration;

use tag_agents::{AgentPane, AgentProfile, AgentStatus};
use tag_core::config::DoneSignal;
use tag_core::event::AppEvent;
use tag_core::pane::{Pane, PaneId};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

fn profile(script: &str, signals: Vec<DoneSignal>) -> AgentProfile {
    AgentProfile {
        name: "t".into(),
        command: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: HashMap::new(),
        default_cwd: None,
        hook_type: None,
        status_patterns: vec![],
        prompt_snippets: vec![],
        done_signal: signals,
        done_regex: None,
        idle_done_secs: 0,
    }
}

async fn pump(pane: &mut AgentPane, rx: &mut UnboundedReceiver<AppEvent>, want: AgentStatus) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < deadline && pane.status() != want {
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            pane.handle_event(&ev);
        }
    }
}

#[tokio::test]
async fn osc_signal_marks_done_via_pty() {
    let (tx, mut rx) = unbounded_channel::<AppEvent>();
    let p = profile(
        "echo hi; sleep 0.3; printf '\\a'; sleep 5",
        vec![DoneSignal::Osc],
    );
    let mut pane = AgentPane::spawn(PaneId(4242), &p, None, 80, 24, 100, tx).unwrap();
    pump(&mut pane, &mut rx, AgentStatus::Done).await;
    assert_eq!(pane.status(), AgentStatus::Done);
    pane.kill();
}

#[tokio::test]
async fn nonzero_exit_is_failed() {
    let (tx, mut rx) = unbounded_channel::<AppEvent>();
    let p = profile("exit 3", vec![]);
    let mut pane = AgentPane::spawn(PaneId(4243), &p, None, 80, 24, 100, tx).unwrap();
    pump(&mut pane, &mut rx, AgentStatus::Failed).await;
    assert_eq!(pane.status(), AgentStatus::Failed);
}

#[tokio::test]
async fn mark_failed_overrides() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let p = profile("sleep 3", vec![]);
    let mut pane = AgentPane::spawn(PaneId(4244), &p, None, 80, 24, 100, tx).unwrap();
    pane.mark_failed();
    assert_eq!(pane.status(), AgentStatus::Failed);
    assert_eq!(pane.agent_status(), Some("failed"));
    pane.set_status_override(None);
    assert_eq!(pane.status(), AgentStatus::Running);
    pane.kill();
}
