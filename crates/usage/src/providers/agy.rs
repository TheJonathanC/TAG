use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::event::UsageEvent;

pub struct AgyProvider;

impl AgyProvider {
    pub fn parse_file(path: impl AsRef<Path>, conversation_id: &str) -> Vec<UsageEvent> {
        let mut events = Vec::new();
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return events,
        };

        let reader = BufReader::new(file);
        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };

            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                // Check if this step has tokens recorded
                let in_tok = v.get("input_tokens").and_then(|t| t.as_u64());
                let out_tok = v.get("output_tokens").and_then(|t| t.as_u64());

                if let (Some(input_tokens), Some(output_tokens)) = (in_tok, out_tok) {
                    let cache_read = v
                        .get("cache_read_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    let step_idx = v.get("step_index").and_then(|s| s.as_u64()).unwrap_or(0);

                    let ts = v
                        .get("created_at")
                        .and_then(|s| s.as_str())
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now);

                    events.push(UsageEvent {
                        agent: "agy".into(),
                        session_id: conversation_id.to_string(),
                        step_id: Some(step_idx.to_string()),
                        project: None,
                        model: "Gemini 3.8 Flash".into(),
                        input_tokens,
                        output_tokens,
                        cache_read,
                        cache_write: 0,
                        timestamp: ts,
                    });
                }
            }
        }

        events
    }
}
