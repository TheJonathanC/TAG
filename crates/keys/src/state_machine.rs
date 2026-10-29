use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    /// Leader was pressed while in passthrough; return to passthrough afterwards.
    resume_passthrough: bool,
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
            resume_passthrough: false,
        }
    }

    pub fn from_config(
        config: &tag_core::config::Config,
        registry: &tag_core::command::CommandRegistry,
    ) -> Self {
        Self::new(
            Keymap::from_config(config, registry),
            Duration::from_millis(config.which_key_delay_ms),
        )
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
        self.resume_passthrough = false;
    }

    pub fn should_show_which_key(&self) -> bool {
        if self.state != KeyState::LeaderPending {
            return false;
        }
        match self.leader_press_time {
            Some(t) => t.elapsed() >= self.which_key_delay,
            None => true,
        }
    }

    pub fn process_key(&mut self, event: KeyEvent) -> Option<Action> {
        let chord = KeyChord::new(event.code, event.modifiers);

        match self.state {
            KeyState::Passthrough => {
                // In passthrough only the leader is intercepted (to reach TAG commands)
                if chord == self.keymap.leader {
                    self.state = KeyState::LeaderPending;
                    self.leader_press_time = Some(Instant::now());
                    self.resume_passthrough = true;
                }
                None
            }
            KeyState::Normal => {
                // Check if leader was pressed
                if chord == self.keymap.leader {
                    self.state = KeyState::LeaderPending;
                    self.leader_press_time = Some(Instant::now());
                    self.resume_passthrough = false;
                    return None;
                }

                // Check direct bindings
                self.keymap
                    .direct_bindings
                    .get(&chord)
                    .map(|(action, _desc)| action.clone())
            }
            KeyState::LeaderPending => {
                let action = self.leader_follower_action(&chord, &event);
                // Leader commands issued from passthrough return to passthrough
                // (a TogglePassthrough action then flips it back to Normal).
                self.state = if self.resume_passthrough {
                    KeyState::Passthrough
                } else {
                    KeyState::Normal
                };
                self.leader_press_time = None;
                self.resume_passthrough = false;
                action
            }
        }
    }

    fn leader_follower_action(&self, chord: &KeyChord, event: &KeyEvent) -> Option<Action> {
        // Leader, Leader sends the leader key itself through to the pane
        if *chord == self.keymap.leader {
            return Some(Action::SendLeaderKey);
        }

        // Followers are plain (optionally shifted) keys; Ctrl/Alt combos cancel the chord
        if event
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }

        match event.code {
            KeyCode::Esc => None,
            KeyCode::Tab => Some(Action::NextWorkspace),
            KeyCode::BackTab => Some(Action::PrevWorkspace),
            KeyCode::Char(c) => {
                // Normalize shift key if uppercase
                let ch = if event.modifiers.contains(KeyModifiers::SHIFT) {
                    c.to_ascii_uppercase()
                } else {
                    c
                };
                self.keymap
                    .leader_bindings
                    .get(&ch)
                    .map(|(action, _desc)| action.clone())
            }
            _ => None,
        }
    }
}
