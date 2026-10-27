use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::theme::ThemeConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfileConfig {
    /// Display name; defaults to the profile key when omitted.
    #[serde(default)]
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub default_cwd: Option<PathBuf>,
    #[serde(default)]
    pub hook_type: Option<String>,
    #[serde(default)]
    pub status_patterns: Vec<String>,
    #[serde(default)]
    pub prompt_snippets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub desktop_notifications: bool,
    pub sound: bool,
    pub only_when_unfocused: bool,
    pub debounce_ms: u64,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            desktop_notifications: true,
            sound: false,
            only_when_unfocused: true,
            debounce_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub leader: String,
    pub which_key_delay_ms: u64,
    pub scrollback_lines: usize,
    pub keymap_preset: String,
    pub vault_idle_timeout_mins: u64,
    pub theme: ThemeConfig,
    pub notifications: NotificationConfig,
    pub agents: HashMap<String, AgentProfileConfig>,
    pub custom_keys: HashMap<String, String>,
    pub run_in_background: bool,
}

impl Default for Config {
    fn default() -> Self {
        let mut agents = HashMap::new();

        agents.insert(
            "claude".into(),
            AgentProfileConfig {
                name: "Claude Code".into(),
                command: "claude".into(),
                args: vec![],
                env: HashMap::new(),
                default_cwd: None,
                hook_type: Some("claude-code".into()),
                status_patterns: vec![
                    r"(?i)approve".into(),
                    r"(?i)\(y/n\)".into(),
                    r"(?i)cost:".into(),
                ],
                prompt_snippets: vec!["Explain this codebase".into(), "Fix failing tests".into()],
            },
        );

        agents.insert(
            "codex".into(),
            AgentProfileConfig {
                name: "Codex CLI".into(),
                command: "codex".into(),
                args: vec![],
                env: HashMap::new(),
                default_cwd: None,
                hook_type: Some("codex-cli".into()),
                status_patterns: vec![r"(?i)permission".into(), r"(?i)confirm".into()],
                prompt_snippets: vec!["Review git diff".into()],
            },
        );

        agents.insert(
            "agy".into(),
            AgentProfileConfig {
                name: "Antigravity".into(),
                command: "agy".into(),
                args: vec![],
                env: HashMap::new(),
                default_cwd: None,
                hook_type: Some("limited".into()),
                status_patterns: vec![r"(?i)allow".into(), r"(?i)proceed\?".into()],
                prompt_snippets: vec!["/goal".into(), "/plan".into()],
            },
        );

        Self {
            leader: "ctrl+space".into(),
            which_key_delay_ms: 400,
            scrollback_lines: 50_000,
            keymap_preset: "default".into(),
            vault_idle_timeout_mins: 15,
            theme: ThemeConfig::default(),
            notifications: NotificationConfig::default(),
            agents,
            custom_keys: HashMap::new(),
            run_in_background: false,
        }
    }
}

impl Config {
    /// Parse a (possibly partial) config file. Missing fields take their defaults and the
    /// built-in agent profiles are kept unless the file overrides them by name.
    pub fn load_from_str(s: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(s)?;
        for (key, profile) in Config::default().agents {
            cfg.agents.entry(key).or_insert(profile);
        }
        for (key, profile) in cfg.agents.iter_mut() {
            if profile.name.is_empty() {
                profile.name = key.clone();
            }
        }
        Ok(cfg)
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partial_config_keeps_defaults() {
        let cfg =
            Config::load_from_str("leader = \"ctrl+a\"\n[notifications]\nsound = true\n").unwrap();
        assert_eq!(cfg.leader, "ctrl+a");
        assert!(cfg.notifications.sound);
        assert!(cfg.notifications.enabled);
        assert_eq!(cfg.scrollback_lines, Config::default().scrollback_lines);
        assert!(cfg.agents.contains_key("claude"));
    }

    #[test]
    fn test_custom_agent_merges_with_builtin_profiles() {
        let cfg = Config::load_from_str("[agents.aider]\ncommand = \"aider\"\n").unwrap();
        assert_eq!(cfg.agents["aider"].name, "aider");
        assert!(cfg.agents["aider"].args.is_empty());
        assert!(cfg.agents.contains_key("codex"));
    }

    #[test]
    fn test_invalid_config_is_an_error() {
        assert!(Config::load_from_str("leader = [").is_err());
    }
}
