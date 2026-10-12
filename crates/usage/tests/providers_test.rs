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
    assert_eq!(events[0].input_tokens, 1800);
    assert_eq!(events[0].output_tokens, 620);
    assert_eq!(events[1].input_tokens, 3100);
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
