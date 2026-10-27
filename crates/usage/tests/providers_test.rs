use std::path::PathBuf;
use tag_usage::{AgyProvider, ClaudeCodeProvider, CodexProvider, UsageStore};

#[test]
fn test_claude_provider_fixture() {
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_transcript.jsonl");
    let events = ClaudeCodeProvider::parse_file(&fixture_path);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].input_tokens, 1250);
    assert_eq!(events[0].output_tokens, 350);
    assert_eq!(events[0].cache_read, 500);
    assert_eq!(events[0].cache_write, 100);
}

#[test]
fn test_codex_provider_fixture() {
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex_session.jsonl");
    let events = CodexProvider::parse_file(&fixture_path);
    assert_eq!(events.len(), 2);
    // Cached tokens are a subset of OpenAI input tokens and are reported separately
    assert_eq!(events[0].input_tokens, 1800 - 400);
    assert_eq!(events[0].cache_read, 400);
    assert_eq!(events[0].output_tokens, 620);
    assert_eq!(events[1].input_tokens, 3100 - 1100);
    assert_eq!(events[1].output_tokens, 750);
}

#[test]
fn test_agy_provider_fixture() {
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agy_transcript.jsonl");
    let events = AgyProvider::parse_file(&fixture_path, "conv-123");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].input_tokens, 15400);
    assert_eq!(events[0].output_tokens, 890);
    assert_eq!(events[0].cache_read, 2500);
    assert_eq!(events[1].input_tokens, 18200);
    assert_eq!(events[1].output_tokens, 1120);
}

#[test]
fn test_usage_store_deduplication_and_aggregates() {
    let mut store = UsageStore::open_in_memory().unwrap();
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_transcript.jsonl");
    let events = ClaudeCodeProvider::parse_file(&fixture_path);

    for ev in &events {
        let inserted = store.insert_event(ev).unwrap();
        assert!(inserted);
    }

    // Duplicate insert should be ignored
    for ev in &events {
        let inserted = store.insert_event(ev).unwrap();
        assert!(!inserted);
    }

    let today = store.query_today().unwrap();
    assert_eq!(today.event_count, 2);
    assert_eq!(today.input_tokens, 1250 + 2200);
    assert_eq!(today.output_tokens, 350 + 420);
    assert!(today.estimated_cost_usd > 0.0);
}

#[test]
fn test_codex_rollout_format() {
    let dir = std::env::temp_dir().join(format!(
        "tag_test_codex_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-2026-01-01T00-00-00-abc.jsonl");
    let lines = [
        r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"sess-xyz"}}"#,
        r#"{"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
        r#"{"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        r#"{"timestamp":"2026-01-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50}}}}"#,
        r#"{"timestamp":"2026-01-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50}}}}"#,
    ];
    std::fs::write(&path, lines.join("\n")).unwrap();

    let events = CodexProvider::parse_file(&path);
    assert_eq!(events.len(), 1, "repeated totals are not double counted");
    assert_eq!(events[0].session_id, "sess-xyz");
    assert_eq!(events[0].model, "gpt-5-codex");
    assert_eq!(events[0].input_tokens, 800);
    assert_eq!(events[0].cache_read, 200);
    assert_eq!(events[0].output_tokens, 50);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_claude_repeated_content_blocks_count_once() {
    let dir = std::env::temp_dir().join(format!(
        "tag_test_claude_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.jsonl");
    let block = r#"{"sessionId":"s","requestId":"req_1","message":{"id":"msg_1","model":"claude-opus-4-1","usage":{"input_tokens":5,"output_tokens":7}}}"#;
    std::fs::write(&path, format!("{}\n{}\n{}\n", block, block, block)).unwrap();
    let events = ClaudeCodeProvider::parse_file(&path);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].step_id.as_deref(), Some("msg_1:req_1"));
    let _ = std::fs::remove_dir_all(&dir);
}
