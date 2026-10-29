pub struct HookSetupInfo {
    pub agent: String,
    pub target_file: Option<String>,
    pub diff: String,
    pub instructions: String,
}

pub fn get_setup_hooks(agent: &str) -> HookSetupInfo {
    match agent.to_lowercase().as_str() {
        "claude" | "claude-code" => HookSetupInfo {
            agent: "Claude Code".into(),
            target_file: Some("~/.claude/settings.json".into()),
            diff: r#"
+ {
+   "hooks": {
+     "Notification": [
+       { "hooks": [ { "type": "command", "command": "cockpit-notify NeedsPermission --agent claude --stdin" } ] }
+     ],
+     "Stop": [
+       { "hooks": [ { "type": "command", "command": "cockpit-notify TaskDone --agent claude --message 'Turn finished'" } ] }
+     ]
+   }
+ }
"#.trim().into(),
            instructions: "Merge the \"hooks\" entries above into ~/.claude/settings.json (or .claude/settings.json in a project). Claude Code passes the notification JSON on stdin, which --stdin reads for the message. Outside TAG, cockpit-notify exits silently.".into(),
        },
        "codex" => HookSetupInfo {
            agent: "Codex CLI".into(),
            target_file: Some("~/.codex/config.toml".into()),
            diff: r#"
+ notify = ["cockpit-notify", "TaskDone", "--agent", "codex"]
"#.trim().into(),
            instructions: "Add the top-level notify setting to ~/.codex/config.toml (above any [table] sections). Codex appends a JSON payload argument, which cockpit-notify uses as the message.".into(),
        },
        "agy" | "antigravity" => HookSetupInfo {
            agent: "Antigravity (agy)".into(),
            target_file: None,
            diff: "# No config hook command exposed in current agy CLI release".into(),
            instructions: "TAG automatically monitors agy via terminal signals (OSC 9 / BEL) and transcript JSONL logs under ~/.gemini/antigravity-cli/brain/. Direct hook execution marked 'limited' until native hook flags are provided.".into(),
        },
        other => HookSetupInfo {
            agent: other.into(),
            target_file: None,
            diff: format!(
                "+ Run: cockpit-notify NeedsPermission --agent {} --message 'Approval required'",
                other
            ),
            instructions: "Configure your agent's notification callback or script to invoke cockpit-notify.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_hooks_target_settings_json() {
        let info = get_setup_hooks("claude");
        assert_eq!(info.target_file.as_deref(), Some("~/.claude/settings.json"));
        assert!(info.diff.contains("\"Notification\""));
        assert!(info.diff.contains("\"Stop\""));
    }

    #[test]
    fn test_codex_uses_top_level_notify_array() {
        let info = get_setup_hooks("codex");
        assert!(info.diff.contains("notify = [\"cockpit-notify\""));
        assert!(!info.diff.contains("notify_cmd"));
    }
}
