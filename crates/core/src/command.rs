use serde::{Deserialize, Serialize};

use crate::action::{Action, Direction, SplitDirection};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandCategory {
    Navigation,
    Layout,
    Agents,
    Git,
    Tools,
    System,
}

impl CommandCategory {
    pub fn name(&self) -> &'static str {
        match self {
            CommandCategory::Navigation => "Navigation",
            CommandCategory::Layout => "Layout",
            CommandCategory::Agents => "Agents",
            CommandCategory::Git => "Git",
            CommandCategory::Tools => "Tools",
            CommandCategory::System => "System",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub category: CommandCategory,
    pub description: String,
    pub default_shortcut: Option<String>,
    pub action: Action,
}

#[derive(Debug, Clone, Default)]
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: Vec::new(),
        };
        registry.register_defaults();
        registry
    }

    pub fn register(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub fn find_by_id(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }

    pub fn register_defaults(&mut self) {
        let defaults = vec![
            Command {
                id: "layout.split_right".into(),
                name: "Split Right".into(),
                category: CommandCategory::Layout,
                description: "Split current pane vertically (right)".into(),
                default_shortcut: Some("^Space+|".into()),
                action: Action::Split(SplitDirection::Vertical),
            },
            Command {
                id: "layout.split_down".into(),
                name: "Split Down".into(),
                category: CommandCategory::Layout,
                description: "Split current pane horizontally (down)".into(),
                default_shortcut: Some("^Space+-".into()),
                action: Action::Split(SplitDirection::Horizontal),
            },
            Command {
                id: "layout.close_pane".into(),
                name: "Close Pane".into(),
                category: CommandCategory::Layout,
                description: "Close the currently focused pane".into(),
                default_shortcut: Some("^Space+x".into()),
                action: Action::ClosePane,
            },
            Command {
                id: "layout.toggle_zoom".into(),
                name: "Maximize / Restore Pane".into(),
                category: CommandCategory::Layout,
                description: "Toggle fullscreen zoom on focused pane".into(),
                default_shortcut: Some("Alt+Enter / ^Space+m".into()),
                action: Action::ToggleZoom,
            },
            Command {
                id: "layout.new_workspace".into(),
                name: "New Workspace".into(),
                category: CommandCategory::Layout,
                description: "Create a new virtual workspace".into(),
                default_shortcut: Some("^Space+w".into()),
                action: Action::NewWorkspace,
            },
            Command {
                id: "layout.close_workspace".into(),
                name: "Close Active Workspace".into(),
                category: CommandCategory::Layout,
                description: "Close the active workspace and terminate its panes".into(),
                default_shortcut: Some("^Space+W / Alt+w".into()),
                action: Action::CloseActiveWorkspace,
            },
            Command {
                id: "pane.rename".into(),
                name: "Rename Pane".into(),
                category: CommandCategory::Layout,
                description: "Change the title of the active pane".into(),
                default_shortcut: Some("^Space+,".into()),
                action: Action::RenamePane(None),
            },
            Command {
                id: "layout.undo".into(),
                name: "Undo Layout Change".into(),
                category: CommandCategory::Layout,
                description: "Revert the last layout modification".into(),
                default_shortcut: Some("^Space+Z".into()),
                action: Action::UndoLayout,
            },
            Command {
                id: "nav.focus_left".into(),
                name: "Focus Left".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to left pane".into(),
                default_shortcut: Some("Alt+h / ^Space+h".into()),
                action: Action::Focus(Direction::Left),
            },
            Command {
                id: "nav.focus_right".into(),
                name: "Focus Right".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to right pane".into(),
                default_shortcut: Some("Alt+l / ^Space+l".into()),
                action: Action::Focus(Direction::Right),
            },
            Command {
                id: "nav.focus_up".into(),
                name: "Focus Up".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to upper pane".into(),
                default_shortcut: Some("Alt+k / ^Space+k".into()),
                action: Action::Focus(Direction::Up),
            },
            Command {
                id: "nav.focus_down".into(),
                name: "Focus Down".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to lower pane".into(),
                default_shortcut: Some("Alt+j / ^Space+j".into()),
                action: Action::Focus(Direction::Down),
            },
            Command {
                id: "nav.tab_tray".into(),
                name: "Toggle Tab Tray".into(),
                category: CommandCategory::Navigation,
                description: "Open the cross-workspace tab navigator".into(),
                default_shortcut: Some("^Space+b".into()),
                action: Action::ToggleTabTray,
            },
            Command {
                id: "nav.quick_switcher".into(),
                name: "Quick Switcher".into(),
                category: CommandCategory::Navigation,
                description: "Fuzzy switch between panes and workspaces".into(),
                default_shortcut: Some("^Space+Space".into()),
                action: Action::ToggleQuickSwitcher,
            },
            Command {
                id: "tools.palette".into(),
                name: "Command Palette".into(),
                category: CommandCategory::Tools,
                description: "Search and run any command".into(),
                default_shortcut: Some("^Space+p".into()),
                action: Action::ToggleCommandPalette,
            },
            Command {
                id: "tools.help".into(),
                name: "Help Cheat-Sheet".into(),
                category: CommandCategory::Tools,
                description: "Contextual shortcut cheat-sheet".into(),
                default_shortcut: Some("^Space+? / F1".into()),
                action: Action::ToggleHelp,
            },
            Command {
                id: "tools.attention".into(),
                name: "Attention Queue".into(),
                category: CommandCategory::Agents,
                description: "View pending agent notifications and requests".into(),
                default_shortcut: Some("^Space+a".into()),
                action: Action::ToggleAttentionQueue,
            },
            Command {
                id: "tools.usage".into(),
                name: "Token Usage & Costs".into(),
                category: CommandCategory::Agents,
                description: "View token usage, rolling windows, and estimates".into(),
                default_shortcut: Some("^Space+u".into()),
                action: Action::ToggleUsageOverlay,
            },
            Command {
                id: "tools.git".into(),
                name: "Git Worktrees & Diff".into(),
                category: CommandCategory::Git,
                description: "Manage worktrees, inspect status, view diffs".into(),
                default_shortcut: Some("^Space+g".into()),
                action: Action::ToggleGitOverlay,
            },
            Command {
                id: "tools.snippets".into(),
                name: "Snippet Bank".into(),
                category: CommandCategory::Tools,
                description: "Browse and insert templated snippets".into(),
                default_shortcut: Some("^Space+s".into()),
                action: Action::ToggleSnippetOverlay,
            },
            Command {
                id: "tools.image_bridge".into(),
                name: "Paste Clipboard Image".into(),
                category: CommandCategory::Tools,
                description: "Save clipboard image to cache and insert path".into(),
                default_shortcut: Some("^Space+i".into()),
                action: Action::PasteImageBridge,
            },
            Command {
                id: "tools.vault".into(),
                name: "Secrets Vault".into(),
                category: CommandCategory::Tools,
                description: "Manage encrypted secrets and env sets".into(),
                default_shortcut: Some("^Space+v".into()),
                action: Action::ToggleVaultOverlay,
            },
            Command {
                id: "tools.dnd".into(),
                name: "Toggle Do Not Disturb".into(),
                category: CommandCategory::Tools,
                description: "Mute non-critical out-of-app notifications".into(),
                default_shortcut: Some("^Space+n".into()),
                action: Action::ToggleDnd,
            },
            Command {
                id: "tools.passthrough".into(),
                name: "Toggle Passthrough Mode".into(),
                category: CommandCategory::Tools,
                description: "Send all key inputs directly to the child process".into(),
                default_shortcut: Some("^Space+Esc".into()),
                action: Action::TogglePassthrough,
            },
            Command {
                id: "agents.launch_agy".into(),
                name: "Launch Agent: Antigravity (agy)".into(),
                category: CommandCategory::Agents,
                description: "Spawn an Antigravity agent pane".into(),
                default_shortcut: None,
                action: Action::SpawnAgent {
                    profile: "agy".into(),
                    worktree: None,
                },
            },
            Command {
                id: "agents.launch_claude".into(),
                name: "Launch Agent: Claude Code".into(),
                category: CommandCategory::Agents,
                description: "Spawn a Claude Code agent pane".into(),
                default_shortcut: None,
                action: Action::SpawnAgent {
                    profile: "claude".into(),
                    worktree: None,
                },
            },
            Command {
                id: "agents.launch_codex".into(),
                name: "Launch Agent: Codex CLI".into(),
                category: CommandCategory::Agents,
                description: "Spawn a Codex CLI agent pane".into(),
                default_shortcut: None,
                action: Action::SpawnAgent {
                    profile: "codex".into(),
                    worktree: None,
                },
            },
            Command {
                id: "agents.attach_tmux".into(),
                name: "Attach Tmux Session".into(),
                category: CommandCategory::Agents,
                description: "Spawn or attach to a tmux session pane".into(),
                default_shortcut: None,
                action: Action::AttachTmux {
                    session: "default".into(),
                },
            },
            Command {
                id: "system.quit".into(),
                name: "Quit TAG".into(),
                category: CommandCategory::System,
                description: "Exit TAG and restore terminal".into(),
                default_shortcut: Some("^Space+q".into()),
                action: Action::Quit,
            },
            Command {
                id: "system.reload_config".into(),
                name: "Reload Configuration".into(),
                category: CommandCategory::System,
                description: "Reload settings, themes, and keymaps".into(),
                default_shortcut: Some("^Space+r".into()),
                action: Action::ReloadConfig,
            },
            Command {
                id: "layout.minimize_pane".into(),
                name: "Minimize Active Pane".into(),
                category: CommandCategory::Layout,
                description: "Minimize pane to tab tray (keeps process running in bg)".into(),
                default_shortcut: Some("^Space+_".into()),
                action: Action::MinimizePane(None),
            },
            Command {
                id: "system.detach".into(),
                name: "Detach Session (Keep in Background)".into(),
                category: CommandCategory::System,
                description: "Detach TAG client and leave all panes running in background".into(),
                default_shortcut: Some("^Space+d".into()),
                action: Action::Detach,
            },
            Command {
                id: "system.toggle_background".into(),
                name: "Toggle Background Persistence Mode".into(),
                category: CommandCategory::System,
                description: "Toggle whether TAG runs as a persistent background daemon".into(),
                default_shortcut: None,
                action: Action::ToggleBackgroundMode,
            },
            Command {
                id: "system.tour".into(),
                name: "Start Tour".into(),
                category: CommandCategory::System,
                description: "Run interactive first-launch tour".into(),
                default_shortcut: None,
                action: Action::StartTour,
            },
        ];

        for cmd in defaults {
            self.register(cmd);
        }
    }
}
