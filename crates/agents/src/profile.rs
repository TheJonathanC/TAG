use std::collections::HashMap;
use std::path::PathBuf;

use tag_core::config::{AgentProfileConfig, DoneSignal};

#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub default_cwd: Option<PathBuf>,
    pub hook_type: Option<String>,
    pub status_patterns: Vec<String>,
    pub prompt_snippets: Vec<String>,
    pub done_signal: Vec<DoneSignal>,
    pub done_regex: Option<String>,
    pub idle_done_secs: u64,
}

impl AgentProfile {
    pub fn from_config(config: &AgentProfileConfig) -> Self {
        Self {
            name: config.name.clone(),
            command: config.command.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
            default_cwd: config.default_cwd.clone(),
            hook_type: config.hook_type.clone(),
            status_patterns: config.status_patterns.clone(),
            prompt_snippets: config.prompt_snippets.clone(),
            done_signal: config.done_signal.clone(),
            done_regex: config.done_regex.clone(),
            idle_done_secs: config.idle_done_secs,
        }
    }
}
