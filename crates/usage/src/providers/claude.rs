use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::event::UsageEvent;

pub struct ClaudeCodeProvider;

impl ClaudeCodeProvider {
    /// Parse a Claude Code transcript (`~/.claude/projects/<proj>/<session>.jsonl`).
    ///
    /// Claude Code writes one line per streamed content block, repeating the same
    /// message usage on each, so events are keyed by message id + request id and
    /// counted once.
    pub fn parse_file(path: impl AsRef<Path>) -> Vec<UsageEvent> {
        let path = path.as_ref();
        let mut events = Vec::new();
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return events,
        };
        let file_session = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "claude-session".to_string());
        let mut seen = HashSet::new();

        let reader = BufReader::new(file);
        for (idx, line_res) in reader.lines().enumerate() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };

            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(message) = v.get("message") else {
                continue;
            };
            let Some(usage) = message.get("usage") else {
                continue;
            };

            let token = |key: &str| usage.get(key).and_then(|t| t.as_u64()).unwrap_or(0);
            let in_tok = token("input_tokens");
            let out_tok = token("output_tokens");
            let cache_read = token("cache_read_input_tokens");
            let cache_write = token("cache_creation_input_tokens");
            if in_tok + out_tok + cache_read + cache_write == 0 {
                continue;
            }

            let message_id = message.get("id").and_then(|m| m.as_str());
            let request_id = v.get("requestId").and_then(|r| r.as_str());
            let step_id = match (message_id, request_id) {
                (Some(m), Some(r)) => format!("{}:{}", m, r),
                (Some(m), None) => m.to_string(),
                _ => idx.to_string(),
            };
            if !seen.insert(step_id.clone()) {
                continue; // same API response repeated for another content block
            }

            let model = message
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("claude")
                .to_string();

            let session_id = v
                .get("session_id")
                .or_else(|| v.get("sessionId"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| file_session.clone());

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
                step_id: Some(step_id),
                project: None,
                model,
                input_tokens: in_tok,
                output_tokens: out_tok,
                cache_read,
                cache_write,
                timestamp: ts,
            });
        }

        events
    }
}
