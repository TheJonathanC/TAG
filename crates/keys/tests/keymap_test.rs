use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use tag_core::action::{Action, Direction, SplitDirection};
use tag_keys::{KeyChord, KeyState, KeyStateMachine, Keymap};

#[test]
fn test_key_chord_parse() {
    let chord = KeyChord::parse("Ctrl+Space").unwrap();
    assert_eq!(chord.code, KeyCode::Char(' '));
    assert_eq!(chord.modifiers, KeyModifiers::CONTROL);

    let chord = KeyChord::parse("Alt+h").unwrap();
    assert_eq!(chord.code, KeyCode::Char('h'));
    assert_eq!(chord.modifiers, KeyModifiers::ALT);
}

#[test]
fn test_leader_state_machine_flow() {
    let keymap = Keymap::default_preset();
    let mut sm = KeyStateMachine::new(keymap, Duration::from_millis(400));
    assert_eq!(sm.state(), KeyState::Normal);

    // Press leader (Ctrl+Space)
    let ev = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    let act = sm.process_key(ev);
    assert_eq!(act, None);
    assert_eq!(sm.state(), KeyState::LeaderPending);

    // Follower key '|' -> Split right
    let ev2 = KeyEvent::new(KeyCode::Char('|'), KeyModifiers::empty());
    let act2 = sm.process_key(ev2);
    assert_eq!(act2, Some(Action::Split(SplitDirection::Vertical)));
    assert_eq!(sm.state(), KeyState::Normal);
}

#[test]
fn test_direct_keys_bypass() {
    let keymap = Keymap::default_preset();
    let mut sm = KeyStateMachine::new(keymap, Duration::from_millis(400));

    // Direct Alt+h -> Focus Left
    let ev = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT);
    let act = sm.process_key(ev);
    assert_eq!(act, Some(Action::Focus(Direction::Left)));
    assert_eq!(sm.state(), KeyState::Normal);
}

#[test]
fn test_passthrough_toggle() {
    let keymap = Keymap::default_preset();
    let mut sm = KeyStateMachine::new(keymap, Duration::from_millis(400));

    sm.toggle_passthrough();
    assert_eq!(sm.state(), KeyState::Passthrough);

    // In passthrough, Alt+h is not intercepted
    let ev = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT);
    let act = sm.process_key(ev);
    assert_eq!(act, None);

    sm.toggle_passthrough();
    assert_eq!(sm.state(), KeyState::Normal);
}
