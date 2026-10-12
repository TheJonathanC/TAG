use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::HashMap;

use tag_core::action::{Action, Direction, SplitDirection};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
        let mut modifiers = KeyModifiers::empty();
        let mut code = None;

        for part in parts {
            match part.to_lowercase().as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "space" => code = Some(KeyCode::Char(' ')),
                "esc" | "escape" => code = Some(KeyCode::Esc),
                "enter" => code = Some(KeyCode::Enter),
                "tab" => code = Some(KeyCode::Tab),
                "backspace" => code = Some(KeyCode::Backspace),
                "pageup" => code = Some(KeyCode::PageUp),
                "pagedown" => code = Some(KeyCode::PageDown),
                "f1" => code = Some(KeyCode::F(1)),
                "f2" => code = Some(KeyCode::F(2)),
                "f3" => code = Some(KeyCode::F(3)),
                "f4" => code = Some(KeyCode::F(4)),
                "f5" => code = Some(KeyCode::F(5)),
                "f6" => code = Some(KeyCode::F(6)),
                "f7" => code = Some(KeyCode::F(7)),
                "f8" => code = Some(KeyCode::F(8)),
                "f9" => code = Some(KeyCode::F(9)),
                "f10" => code = Some(KeyCode::F(10)),
                "f11" => code = Some(KeyCode::F(11)),
                "f12" => code = Some(KeyCode::F(12)),
                other => {
                    if other.chars().count() == 1 {
                        code = Some(KeyCode::Char(other.chars().next().unwrap()));
                    }
                }
            }
        }

        code.map(|c| Self { code: c, modifiers })
    }

    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            s.push_str("Ctrl+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            s.push_str("Alt+");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            s.push_str("Shift+");
        }
        match self.code {
            KeyCode::Char(' ') => s.push_str("Space"),
            KeyCode::Char(c) => s.push(c),
            KeyCode::Esc => s.push_str("Esc"),
            KeyCode::Enter => s.push_str("Enter"),
            KeyCode::Tab => s.push_str("Tab"),
            KeyCode::Backspace => s.push_str("Backspace"),
            KeyCode::PageUp => s.push_str("PageUp"),
            KeyCode::PageDown => s.push_str("PageDown"),
            KeyCode::F(n) => s.push_str(&format!("F{}", n)),
            _ => s.push('?'),
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    pub leader: KeyChord,
    pub leader_bindings: HashMap<char, (Action, &'static str)>,
    pub direct_bindings: HashMap<KeyChord, (Action, &'static str)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::default_preset()
    }
}

impl Keymap {
    pub fn default_preset() -> Self {
        let leader = KeyChord::new(KeyCode::Char(' '), KeyModifiers::CONTROL);

        let mut leader_bindings = HashMap::new();
        leader_bindings.insert(
            '|',
            (Action::Split(SplitDirection::Vertical), "Split right"),
        );
        leader_bindings.insert(
            '-',
            (Action::Split(SplitDirection::Horizontal), "Split down"),
        );
        leader_bindings.insert('x', (Action::ClosePane, "Close pane"));
        leader_bindings.insert('z', (Action::ToggleZoom, "Toggle zoom"));
        leader_bindings.insert('Z', (Action::UndoLayout, "Undo layout"));
        leader_bindings.insert('h', (Action::Focus(Direction::Left), "Focus left"));
        leader_bindings.insert('j', (Action::Focus(Direction::Down), "Focus down"));
        leader_bindings.insert('k', (Action::Focus(Direction::Up), "Focus up"));
        leader_bindings.insert('l', (Action::Focus(Direction::Right), "Focus right"));
        leader_bindings.insert('c', (Action::SpawnShell, "New shell pane"));
        leader_bindings.insert('w', (Action::NewWorkspace, "New workspace"));
        leader_bindings.insert('b', (Action::ToggleTabTray, "Tab tray"));
        leader_bindings.insert(' ', (Action::ToggleQuickSwitcher, "Quick switcher"));
        leader_bindings.insert('a', (Action::ToggleAttentionQueue, "Attention queue"));
        leader_bindings.insert('u', (Action::ToggleUsageOverlay, "Usage & tokens"));
        leader_bindings.insert('g', (Action::ToggleGitOverlay, "Git worktrees"));
        leader_bindings.insert('s', (Action::ToggleSnippetOverlay, "Snippets"));
        leader_bindings.insert('i', (Action::PasteImageBridge, "Paste image"));
        leader_bindings.insert('v', (Action::ToggleVaultOverlay, "Vault"));
        leader_bindings.insert('n', (Action::ToggleDnd, "Do Not Disturb"));
        leader_bindings.insert('p', (Action::ToggleCommandPalette, "Command palette"));
        leader_bindings.insert('?', (Action::ToggleHelp, "Help cheat-sheet"));
        leader_bindings.insert('q', (Action::Quit, "Quit"));
        leader_bindings.insert('r', (Action::ReloadConfig, "Reload config"));

        let mut direct_bindings = HashMap::new();
        // Alt+h/j/k/l focus navigation
        direct_bindings.insert(
            KeyChord::new(KeyCode::Char('h'), KeyModifiers::ALT),
            (Action::Focus(Direction::Left), "Focus left"),
        );
        direct_bindings.insert(
            KeyChord::new(KeyCode::Char('j'), KeyModifiers::ALT),
            (Action::Focus(Direction::Down), "Focus down"),
        );
        direct_bindings.insert(
            KeyChord::new(KeyCode::Char('k'), KeyModifiers::ALT),
            (Action::Focus(Direction::Up), "Focus up"),
        );
        direct_bindings.insert(
            KeyChord::new(KeyCode::Char('l'), KeyModifiers::ALT),
            (Action::Focus(Direction::Right), "Focus right"),
        );
        direct_bindings.insert(
            KeyChord::new(KeyCode::Enter, KeyModifiers::ALT),
            (Action::ToggleZoom, "Toggle zoom"),
        );
        direct_bindings.insert(
            KeyChord::new(KeyCode::F(1), KeyModifiers::empty()),
            (Action::ToggleHelp, "Help cheat-sheet"),
        );

        // Alt+1..9 for workspace jumps
        for i in 1..=9 {
            if let Some(ch) = char::from_digit(i as u32, 10) {
                direct_bindings.insert(
                    KeyChord::new(KeyCode::Char(ch), KeyModifiers::ALT),
                    (
                        Action::SwitchWorkspace(tag_core::pane::WorkspaceId(i)),
                        "Switch workspace",
                    ),
                );
            }
        }

        Self {
            leader,
            leader_bindings,
            direct_bindings,
        }
    }

    pub fn check_conflicts(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        // Check if leader matches any direct binding
        if self.direct_bindings.contains_key(&self.leader) {
            warnings.push(format!(
                "Leader key '{}' conflicts with direct binding!",
                self.leader.display()
            ));
        }
        warnings
    }
}
