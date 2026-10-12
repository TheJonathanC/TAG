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
                default_shortcut: Some("Leader+|".into()),
                action: Action::Split(SplitDirection::Vertical),
            },
            Command {
                id: "layout.split_down".into(),
                name: "Split Down".into(),
                category: CommandCategory::Layout,
                description: "Split current pane horizontally (down)".into(),
                default_shortcut: Some("Leader+-".into()),
                action: Action::Split(SplitDirection::Horizontal),
            },
            Command {
                id: "layout.close_pane".into(),
                name: "Close Pane".into(),
                category: CommandCategory::Layout,
                description: "Close the currently focused pane".into(),
                default_shortcut: Some("Leader+x".into()),
                action: Action::ClosePane,
            },
            Command {
                id: "layout.toggle_zoom".into(),
                name: "Toggle Zoom".into(),
                category: CommandCategory::Layout,
                description: "Toggle fullscreen zoom on focused pane".into(),
                default_shortcut: Some("Leader+z".into()),
                action: Action::ToggleZoom,
            },
            Command {
                id: "layout.undo".into(),
                name: "Undo Layout Change".into(),
                category: CommandCategory::Layout,
                description: "Revert the last layout modification".into(),
                default_shortcut: Some("Leader+Z".into()),
                action: Action::UndoLayout,
            },
            Command {
                id: "nav.focus_left".into(),
                name: "Focus Left".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to left pane".into(),
                default_shortcut: Some("Alt+h / Leader+h".into()),
                action: Action::Focus(Direction::Left),
            },
            Command {
                id: "nav.focus_right".into(),
                name: "Focus Right".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to right pane".into(),
                default_shortcut: Some("Alt+l / Leader+l".into()),
                action: Action::Focus(Direction::Right),
            },
            Command {
                id: "nav.focus_up".into(),
                name: "Focus Up".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to upper pane".into(),
                default_shortcut: Some("Alt+k / Leader+k".into()),
                action: Action::Focus(Direction::Up),
            },
            Command {
                id: "nav.focus_down".into(),
                name: "Focus Down".into(),
                category: CommandCategory::Navigation,
                description: "Move focus to lower pane".into(),
                default_shortcut: Some("Alt+j / Leader+j".into()),
                action: Action::Focus(Direction::Down),
            },
            Command {
                id: "nav.tab_tray".into(),
                name: "Toggle Tab Tray".into(),
                category: CommandCategory::Navigation,
                description: "Open the cross-workspace tab navigator".into(),
                default_shortcut: Some("Leader+b".into()),
                action: Action::ToggleTabTray,
            },
            Command {
                id: "nav.quick_switcher".into(),
                name: "Quick Switcher".into(),
                category: CommandCategory::Navigation,
                description: "Fuzzy switch between panes and workspaces".into(),
                default_shortcut: Some("Leader+Space".into()),
                action: Action::ToggleQuickSwitcher,
            },
            Command {
                id: "tools.palette".into(),
                name: "Command Palette".into(),
                category: CommandCategory::Tools,
                description: "Search and run any command".into(),
                default_shortcut: Some("Leader+p".into()),
                action: Action::ToggleCommandPalette,
            },
            Command {
                id: "tools.help".into(),
                name: "Help Cheat-Sheet".into(),
                category: CommandCategory::Tools,
                description: "Contextual shortcut cheat-sheet".into(),
                default_shortcut: Some("Leader+? / F1".into()),
                action: Action::ToggleHelp,
            },
            Command {
                id: "tools.attention".into(),
                name: "Attention Queue".into(),
                category: CommandCategory::Agents,
                description: "View pending agent notifications and requests".into(),
                default_shortcut: Some("Leader+a".into()),
                action: Action::ToggleAttentionQueue,
            },
            Command {
                id: "tools.usage".into(),
                name: "Token Usage & Costs".into(),
                category: CommandCategory::Agents,
                description: "View token usage, rolling windows, and estimates".into(),
                default_shortcut: Some("Leader+u".into()),
                action: Action::ToggleUsageOverlay,
            },
            Command {
                id: "tools.git".into(),
                name: "Git Worktrees & Diff".into(),
                category: CommandCategory::Git,
                description: "Manage worktrees, inspect status, view diffs".into(),
                default_shortcut: Some("Leader+g".into()),
                action: Action::ToggleGitOverlay,
            },
            Command {
                id: "tools.snippets".into(),
                name: "Snippet Bank".into(),
                category: CommandCategory::Tools,
                description: "Browse and insert templated snippets".into(),
                default_shortcut: Some("Leader+s".into()),
                action: Action::ToggleSnippetOverlay,
            },
            Command {
                id: "tools.image_bridge".into(),
                name: "Paste Clipboard Image".into(),
                category: CommandCategory::Tools,
                description: "Save clipboard image to cache and insert path".into(),
                default_shortcut: Some("Leader+i".into()),
                action: Action::PasteImageBridge,
            },
            Command {
                id: "tools.vault".into(),
                name: "Secrets Vault".into(),
                category: CommandCategory::Tools,
                description: "Manage encrypted secrets and env sets".into(),
                default_shortcut: Some("Leader+v".into()),
                action: Action::ToggleVaultOverlay,
            },
            Command {
                id: "tools.dnd".into(),
                name: "Toggle Do Not Disturb".into(),
                category: CommandCategory::Tools,
                description: "Mute non-critical out-of-app notifications".into(),
                default_shortcut: Some("Leader+n".into()),
                action: Action::ToggleDnd,
            },
            Command {
                id: "tools.passthrough".into(),
                name: "Toggle Passthrough Mode".into(),
                category: CommandCategory::Tools,
                description: "Send all key inputs directly to the child process".into(),
                default_shortcut: Some("Leader+Esc".into()),
                action: Action::TogglePassthrough,
            },
            Command {
                id: "system.quit".into(),
                name: "Quit TAG".into(),
                category: CommandCategory::System,
                description: "Exit TAG and restore terminal".into(),
                default_shortcut: Some("Leader+q".into()),
                action: Action::Quit,
            },
            Command {
                id: "system.reload_config".into(),
                name: "Reload Configuration".into(),
                category: CommandCategory::System,
                description: "Reload settings, themes, and keymaps".into(),
                default_shortcut: Some("Leader+r".into()),
                action: Action::ReloadConfig,
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
