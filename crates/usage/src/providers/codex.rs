use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::event::UsageEvent;

pub struct CodexProvider;

impl CodexProvider {
    /// Parse a Codex CLI session log (`~/.codex/sessions/**/rollout-*.jsonl`).
    ///
    /// Supports the rollout format (`session_meta`, `turn_context` and
    /// `event_msg` / `token_count` lines carrying `last_token_usage`) as well as
    /// older flat `token_count` / `usage` lines. OpenAI reports cached tokens as a
    /// subset of input tokens, so they are split out to avoid double counting.
    pub fn parse_file(path: impl AsRef<Path>) -> Vec<UsageEvent> {
        let path = path.as_ref();
        let mut events = Vec::new();
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return events,
        };

        // Unique per file, so step ids from different sessions never collide
        let mut session_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "codex-session".to_string());
        let mut model = "gpt-5".to_string();
        let mut last_total: Option<Value> = None;

        let reader = BufReader::new(file);
        for (idx, line_res) in reader.lines().enumerate() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let ts = parse_ts(&v);

            match v.get("type").and_then(|t| t.as_str()) {
                Some("session_meta") => {
                    if let Some(id) = v.pointer("/payload/id").and_then(|i| i.as_str()) {
                        session_id = id.to_string();
                    }
                    continue;
                }
                Some("turn_context") => {
                    if let Some(m) = v.pointer("/payload/model").and_then(|m| m.as_str()) {
                        model = m.to_string();
                    }
                    continue;
                }
                Some("event_msg") => {
                    if v.pointer("/payload/type").and_then(|t| t.as_str()) != Some("token_count") {
                        continue;
                    }
                    let Some(info) = v.pointer("/payload/info").filter(|i| !i.is_null()) else {
                        continue;
                    };
                    // token_count is re-emitted (e.g. for rate-limit updates) with unchanged totals
                    let total = info.get("total_token_usage").cloned();
                    if total.is_some() && total == last_total {
                        continue;
                    }
                    last_total = total;
                    let Some(usage) = info.get("last_token_usage") else {
                        continue;
                    };
                    if let Some(ev) = usage_event(usage, &session_id, &model, idx, ts) {
                        events.push(ev);
                    }
                    continue;
                }
                _ => {}
            }

            // Legacy flat format
            let is_token_event = v.get("event").and_then(|e| e.as_str()) == Some("token_count")
                || v.get("tokens").is_some()
                || v.get("usage").is_some();
            if !is_token_event {
                continue;
            }
            let usage_obj = v.get("tokens").or_else(|| v.get("usage")).unwrap_or(&v);
            let line_model = v.get("model").and_then(|m| m.as_str()).unwrap_or(&model);
            let line_session = v
                .get("session_id")
                .and_then(|s| s.as_str())
                .unwrap_or(&session_id);
            if let Some(ev) = usage_event(usage_obj, line_session, line_model, idx, ts) {
                events.push(ev);
            }
        }

        events
    }
}

fn parse_ts(v: &Value) -> DateTime<Utc> {
    v.get("timestamp")
        .or_else(|| v.get("created_at"))
        .and_then(|s| s.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

fn usage_event(
    usage: &Value,
    session_id: &str,
    model: &str,
    idx: usize,
    timestamp: DateTime<Utc>,
) -> Option<UsageEvent> {
    let get = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| usage.get(*k).and_then(|t| t.as_u64()))
            .unwrap_or(0)
    };
    let input_total = get(&["input_tokens", "prompt_tokens"]);
    let output = get(&["output_tokens", "completion_tokens"]);
    let cached = get(&["cached_input_tokens", "cached_tokens"]).min(input_total);
    if input_total + output == 0 {
        return None;
    }

    Some(UsageEvent {
        agent: "codex".into(),
        session_id: session_id.to_string(),
        step_id: Some(idx.to_string()),
        project: None,
        model: model.to_string(),
        input_tokens: input_total - cached,
        output_tokens: output,
        cache_read: cached,
        cache_write: 0,
        timestamp,
    })
}
