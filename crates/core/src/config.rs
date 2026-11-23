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
    /// Sugar: when set, `model_flag` (default `--model`) and the model are appended to argv.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_flag: Option<String>,
    #[serde(default)]
    pub roles: Vec<AgentRole>,
    #[serde(default)]
    pub prompt_delivery: PromptDelivery,
    /// Used when `prompt_delivery = "arg"`; `{prompt}` is substituted.
    #[serde(default)]
    pub prompt_args: Vec<String>,
    /// `{session_id}` is substituted.
    #[serde(default)]
    pub resume_args: Vec<String>,
    /// `{mcp_config_path}` is substituted.
    #[serde(default)]
    pub mcp_config_args: Vec<String>,
    /// Lets TAG choose a session id up front; `{session_id}` is substituted.
    #[serde(default)]
    pub session_id_args: Vec<String>,
    #[serde(default)]
    pub done_signal: Vec<DoneSignal>,
    #[serde(default)]
    pub done_regex: Option<String>,
    /// Seconds of idleness that count as "done" (0 = disabled).
    #[serde(default)]
    pub idle_done_secs: u64,
    /// Max simultaneous panes for this profile (0 = unlimited).
    #[serde(default)]
    pub max_concurrent: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Conductor,
    Worker,
    Reviewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptDelivery {
    /// Type the prompt into the running agent (bracketed paste + Enter).
    #[default]
    Type,
    /// Pass the prompt as command-line arguments (`prompt_args`).
    Arg,
    /// Write the prompt to stdin.
    Stdin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoneSignal {
    Hook,
    Osc,
    Idle,
    Regex,
    Exit,
}

/// Inputs for [`AgentProfileConfig::build_argv`].
#[derive(Debug, Clone, Default)]
pub struct LaunchOpts {
    pub prompt: Option<String>,
    pub resume_session: Option<String>,
    pub new_session_id: Option<String>,
    pub mcp_config_path: Option<String>,
}

impl AgentProfileConfig {
    /// Build the command and argument vector for launching this profile.
    /// Order: base args, model flag, MCP config, resume (or new session id), prompt args.
    pub fn build_argv(&self, opts: &LaunchOpts) -> (String, Vec<String>) {
        let sub = |tpl: &[String], key: &str, val: &str| -> Vec<String> {
            tpl.iter().map(|a| a.replace(key, val)).collect()
        };
        let mut args = self.args.clone();
        if let Some(model) = &self.model {
            args.push(self.model_flag.clone().unwrap_or_else(|| "--model".into()));
            args.push(model.clone());
        }
        if let Some(path) = &opts.mcp_config_path {
            args.extend(sub(&self.mcp_config_args, "{mcp_config_path}", path));
        }
        if let Some(id) = &opts.resume_session {
            args.extend(sub(&self.resume_args, "{session_id}", id));
        } else if let Some(id) = &opts.new_session_id {
            args.extend(sub(&self.session_id_args, "{session_id}", id));
        }
        if let (Some(prompt), PromptDelivery::Arg) = (&opts.prompt, self.prompt_delivery) {
            args.extend(sub(&self.prompt_args, "{prompt}", prompt));
        }
        (self.command.clone(), args)
    }
}

impl Default for AgentProfileConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            default_cwd: None,
            hook_type: None,
            status_patterns: vec![],
            prompt_snippets: vec![],
            model: None,
            model_flag: None,
            roles: vec![],
            prompt_delivery: PromptDelivery::Type,
            prompt_args: vec![],
            resume_args: vec![],
            mcp_config_args: vec![],
            session_id_args: vec![],
            done_signal: vec![],
            done_regex: None,
            idle_done_secs: 0,
            max_concurrent: 0,
        }
    }
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
                roles: vec![AgentRole::Conductor, AgentRole::Worker, AgentRole::Reviewer],
                prompt_delivery: PromptDelivery::Arg,
                prompt_args: vec!["{prompt}".into()],
                resume_args: vec!["--resume".into(), "{session_id}".into()],
                session_id_args: vec!["--session-id".into(), "{session_id}".into()],
                mcp_config_args: vec!["--mcp-config".into(), "{mcp_config_path}".into()],
                done_signal: vec![DoneSignal::Hook, DoneSignal::Idle],
                idle_done_secs: 60,
                max_concurrent: 4,
                ..Default::default()
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
                roles: vec![AgentRole::Conductor, AgentRole::Worker, AgentRole::Reviewer],
                prompt_delivery: PromptDelivery::Arg,
                prompt_args: vec!["{prompt}".into()],
                // [verify] codex is not installed on the dev machine; resume syntax unconfirmed.
                resume_args: vec!["resume".into(), "{session_id}".into()],
                done_signal: vec![DoneSignal::Osc, DoneSignal::Idle],
                idle_done_secs: 45,
                max_concurrent: 4,
                ..Default::default()
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
                roles: vec![AgentRole::Conductor, AgentRole::Worker],
                prompt_delivery: PromptDelivery::Arg,
                prompt_args: vec!["-i".into(), "{prompt}".into()],
                resume_args: vec!["--conversation".into(), "{session_id}".into()],
                done_signal: vec![DoneSignal::Osc, DoneSignal::Idle],
                idle_done_secs: 45,
                max_concurrent: 6,
                ..Default::default()
            },
        );

        if let Some(agy) = agents.get("agy").cloned() {
            agents.insert(
                "agy-neon".into(),
                AgentProfileConfig {
                    name: "agy · Gemini 4 Neon".into(),
                    model: Some("gemini-4-neon-high".into()),
                    ..agy
                },
            );
        }

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
    fn old_style_profile_still_parses() {
        let cfg = Config::load_from_str(
            "[agents.mine]\ncommand = \"foo\"\nargs = [\"-x\"]\nhook_type = \"limited\"\n",
        )
        .unwrap();
        let p = &cfg.agents["mine"];
        assert_eq!(p.prompt_delivery, PromptDelivery::Type);
        assert!(p.roles.is_empty() && p.done_signal.is_empty());
        assert_eq!((p.idle_done_secs, p.max_concurrent), (0, 0));
        assert!(cfg.agents.contains_key("agy-neon"));
    }

    #[test]
    fn new_keys_parse() {
        let cfg = Config::load_from_str(
            "[agents.x]\ncommand=\"x\"\nroles=[\"conductor\"]\nprompt_delivery=\"stdin\"\ndone_signal=[\"regex\",\"exit\"]\ndone_regex=\"DONE\"\n",
        )
        .unwrap();
        let p = &cfg.agents["x"];
        assert_eq!(p.roles, vec![AgentRole::Conductor]);
        assert_eq!(p.prompt_delivery, PromptDelivery::Stdin);
        assert_eq!(p.done_signal, vec![DoneSignal::Regex, DoneSignal::Exit]);
    }

    fn full() -> AgentProfileConfig {
        let c = Config::default();
        c.agents["claude"].clone()
    }

    #[test]
    fn argv_empty_opts() {
        let (cmd, args) = full().build_argv(&LaunchOpts::default());
        assert_eq!(cmd, "claude");
        assert!(args.is_empty());
    }

    #[test]
    fn argv_all_new_session() {
        let o = LaunchOpts {
            prompt: Some("hi".into()),
            new_session_id: Some("S1".into()),
            mcp_config_path: Some("/m.json".into()),
            ..Default::default()
        };
        let mut p = full();
        p.model = Some("opus".into());
        let (_, args) = p.build_argv(&o);
        assert_eq!(
            args,
            [
                "--model",
                "opus",
                "--mcp-config",
                "/m.json",
                "--session-id",
                "S1",
                "hi"
            ]
        );
    }

    #[test]
    fn argv_resume_wins_over_new_id() {
        let o = LaunchOpts {
            resume_session: Some("R".into()),
            new_session_id: Some("N".into()),
            ..Default::default()
        };
        let (_, args) = full().build_argv(&o);
        assert_eq!(args, ["--resume", "R"]);
    }

    #[test]
    fn argv_prompt_only_when_arg_delivery_and_custom_model_flag() {
        let mut p = full();
        p.prompt_delivery = PromptDelivery::Type;
        let o = LaunchOpts {
            prompt: Some("x".into()),
            ..Default::default()
        };
        assert!(p.build_argv(&o).1.is_empty());
        p.model = Some("m".into());
        p.model_flag = Some("-m".into());
        p.args = vec!["--a".into()];
        assert_eq!(p.build_argv(&o).1, ["--a", "-m", "m"]);
    }

    #[test]
    fn argv_substring_substitution_and_agy_neon() {
        let mut p = Config::default().agents["agy-neon"].clone();
        let o = LaunchOpts {
            prompt: Some("go".into()),
            resume_session: Some("c1".into()),
            mcp_config_path: Some("/ignored".into()),
            ..Default::default()
        };
        assert_eq!(
            p.build_argv(&o).1,
            [
                "--model",
                "gemini-4-neon-high",
                "--conversation",
                "c1",
                "-i",
                "go"
            ]
        );
        p.prompt_args = vec!["--p={prompt}".into()];
        assert_eq!(p.build_argv(&o).1.last().unwrap(), "--p=go");
    }

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
