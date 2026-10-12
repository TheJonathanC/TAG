use crossterm::event::{KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

use tag_core::action::Action;

use crate::keymap::{KeyChord, Keymap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Normal,
    LeaderPending,
    Passthrough,
}

pub struct KeyStateMachine {
    keymap: Keymap,
    state: KeyState,
    leader_press_time: Option<Instant>,
    which_key_delay: Duration,
}

impl Default for KeyStateMachine {
    fn default() -> Self {
        Self::new(Keymap::default(), Duration::from_millis(400))
    }
}

impl KeyStateMachine {
    pub fn new(keymap: Keymap, which_key_delay: Duration) -> Self {
        Self {
            keymap,
            state: KeyState::Normal,
            leader_press_time: None,
            which_key_delay,
        }
    }

    pub fn state(&self) -> KeyState {
        self.state
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn toggle_passthrough(&mut self) {
        self.state = match self.state {
            KeyState::Passthrough => KeyState::Normal,
            _ => KeyState::Passthrough,
        };
        self.leader_press_time = None;
    }

    pub fn should_show_which_key(&self) -> bool {
        if self.state == KeyState::LeaderPending {
            if let Some(t) = self.leader_press_time {
                return t.elapsed() >= self.which_key_delay;
            }
        }
        false
    }

    pub fn process_key(&mut self, event: KeyEvent) -> Option<Action> {
        let chord = KeyChord::new(event.code, event.modifiers);

        // Check for passthrough toggle escape chord
        if self.state == KeyState::Passthrough {
            if chord == self.keymap.leader {
                self.state = KeyState::LeaderPending;
                self.leader_press_time = Some(Instant::now());
                return None;
            }
            // In passthrough, don't intercept other keys
            return None;
        }

        match self.state {
            KeyState::Normal => {
                // Check if leader was pressed
                if chord == self.keymap.leader {
                    self.state = KeyState::LeaderPending;
                    self.leader_press_time = Some(Instant::now());
                    return None;
                }

                // Check direct bindings
                if let Some((action, _desc)) = self.keymap.direct_bindings.get(&chord) {
                    return Some(action.clone());
                }

                None
            }
            KeyState::LeaderPending => {
                self.state = KeyState::Normal;
                self.leader_press_time = None;

                // Check if leader followed by Esc cancels or toggles passthrough
                match event.code {
                    crossterm::event::KeyCode::Esc => {
                        return Some(Action::TogglePassthrough);
                    }
                    crossterm::event::KeyCode::Tab => {
                        return Some(Action::NextWorkspace);
                    }
                    crossterm::event::KeyCode::BackTab => {
                        return Some(Action::PrevWorkspace);
                    }
                    crossterm::event::KeyCode::Char(c) => {
                        // Normalize shift key if uppercase
                        let ch = if event.modifiers.contains(KeyModifiers::SHIFT) {
                            c.to_ascii_uppercase()
                        } else {
                            c
                        };

                        if let Some((action, _desc)) = self.keymap.leader_bindings.get(&ch) {
                            return Some(action.clone());
                        }
                    }
                    _ => {}
                }

                None
            }
            KeyState::Passthrough => None,
        }
    }
}
