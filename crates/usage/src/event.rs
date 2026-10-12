use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub agent: String,
    pub session_id: String,
    pub step_id: Option<String>,
    pub project: Option<String>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub timestamp: DateTime<Utc>,
}
