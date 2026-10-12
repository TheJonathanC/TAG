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
            target_file: Some("~/.claude/config.json".into()),
            diff: r#"
+ {
+   "hooks": {
+     "notification": "cockpit-notify NeedsPermission --message '$MESSAGE'",
+     "turn_stop": "cockpit-notify TaskDone --message 'Turn finished'"
+   }
+ }
"#.trim().into(),
            instructions: "Add the hook entries above to ~/.claude/config.json. When running inside TAG, cockpit-notify communicates with the session socket seamlessly.".into(),
        },
        "codex" => HookSetupInfo {
            agent: "Codex CLI".into(),
            target_file: Some("~/.codex/config.toml".into()),
            diff: r#"
+ [notifications]
+ notify_cmd = "cockpit-notify TaskDone --message 'Codex turn completed'"
"#.trim().into(),
            instructions: "Add notify_cmd to ~/.codex/config.toml. When running, Codex calls cockpit-notify upon completing tasks.".into(),
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
