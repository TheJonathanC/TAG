use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::event::UsageEvent;

pub struct ClaudeCodeProvider;

impl ClaudeCodeProvider {
    pub fn parse_file(path: impl AsRef<Path>) -> Vec<UsageEvent> {
        let mut events = Vec::new();
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return events,
        };

        let reader = BufReader::new(file);
        for (idx, line_res) in reader.lines().enumerate() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };

            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                // Defensive extraction of message token usage
                if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                    let in_tok = usage
                        .get("input_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    let out_tok = usage
                        .get("output_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    let cache_read = usage
                        .get("cache_read_input_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    let cache_write = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);

                    let model = v
                        .get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("claude-3-5-sonnet")
                        .to_string();

                    let session_id = v
                        .get("session_id")
                        .or_else(|| v.get("sessionId"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("claude-session")
                        .to_string();

                    let ts = v
                        .get("timestamp")
                        .or_else(|| v.get("created_at"))
                        .and_then(|s| s.as_str())
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now);

                    events.push(UsageEvent {
                        agent: "claude".into(),
                        session_id,
                        step_id: Some(idx.to_string()),
                        project: None,
                        model,
                        input_tokens: in_tok,
                        output_tokens: out_tok,
                        cache_read,
                        cache_write,
                        timestamp: ts,
                    });
                }
            }
        }

        events
    }
}
