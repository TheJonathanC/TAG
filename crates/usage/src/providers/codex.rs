use chrono::Utc;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::event::UsageEvent;

pub struct CodexProvider;

impl CodexProvider {
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
                // Check if this is a token_count event or usage payload
                let is_token_event = v.get("event").and_then(|e| e.as_str()) == Some("token_count")
                    || v.get("tokens").is_some()
                    || v.get("usage").is_some();

                if is_token_event {
                    let usage_obj = v.get("tokens").or_else(|| v.get("usage")).unwrap_or(&v);

                    let in_tok = usage_obj
                        .get("input_tokens")
                        .or_else(|| usage_obj.get("prompt_tokens"))
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    let out_tok = usage_obj
                        .get("output_tokens")
                        .or_else(|| usage_obj.get("completion_tokens"))
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    let cache_read = usage_obj
                        .get("cached_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);

                    let model = v
                        .get("model")
                        .and_then(|m| m.as_str())
                        .unwrap_or("gpt-4o")
                        .to_string();

                    let session_id = v
                        .get("session_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("codex-session")
                        .to_string();

                    events.push(UsageEvent {
                        agent: "codex".into(),
                        session_id,
                        step_id: Some(idx.to_string()),
                        project: None,
                        model,
                        input_tokens: in_tok,
                        output_tokens: out_tok,
                        cache_read,
                        cache_write: 0,
                        timestamp: Utc::now(),
                    });
                }
            }
        }

        events
    }
}
