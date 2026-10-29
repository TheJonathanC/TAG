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

#[test]
fn test_copy_selection_direct_chord() {
    let keymap = Keymap::default_preset();
    let mut sm = KeyStateMachine::new(keymap, Duration::from_millis(400));

    // Ctrl+Shift+C directly triggers CopySelection
    let ev = KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    let act = sm.process_key(ev);
    assert_eq!(act, Some(Action::CopySelection));

    // Ctrl+C (uppercase with CONTROL) triggers CopySelection
    let ev2 = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL);
    let act2 = sm.process_key(ev2);
    assert_eq!(act2, Some(Action::CopySelection));
}

#[test]
fn test_paste_clipboard_chords() {
    let keymap = Keymap::default_preset();
    let mut sm = KeyStateMachine::new(keymap, Duration::from_millis(400));

    // Direct Ctrl+Shift+V
    let ev = KeyEvent::new(
        KeyCode::Char('V'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(sm.process_key(ev), Some(Action::PasteFromClipboard));

    // Direct Shift+Insert
    let ev_ins = KeyEvent::new(KeyCode::Insert, KeyModifiers::SHIFT);
    assert_eq!(sm.process_key(ev_ins), Some(Action::PasteFromClipboard));

    // Leader + p
    let leader = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    assert_eq!(sm.process_key(leader), None);
    assert_eq!(sm.state(), KeyState::LeaderPending);
    let p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty());
    assert_eq!(sm.process_key(p), Some(Action::PasteFromClipboard));

    // Leader + V
    assert_eq!(sm.process_key(leader), None);
    let v_shift = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SHIFT);
    assert_eq!(sm.process_key(v_shift), Some(Action::PasteFromClipboard));

    // Leader + v
    assert_eq!(sm.process_key(leader), None);
    let v_key = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::empty());
    assert_eq!(sm.process_key(v_key), Some(Action::PasteFromClipboard));

    // Leader + e (Vault)
    assert_eq!(sm.process_key(leader), None);
    let e_key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::empty());
    assert_eq!(sm.process_key(e_key), Some(Action::ToggleVaultOverlay));
}

fn leader_event() -> KeyEvent {
    KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)
}

#[test]
fn test_leader_c_spawns_shell() {
    let mut sm = KeyStateMachine::new(Keymap::default_preset(), Duration::from_millis(400));
    sm.process_key(leader_event());
    let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
    assert_eq!(sm.process_key(c), Some(Action::SpawnShell));
}

#[test]
fn test_double_leader_sends_leader_key() {
    let mut sm = KeyStateMachine::new(Keymap::default_preset(), Duration::from_millis(400));
    sm.process_key(leader_event());
    assert_eq!(sm.process_key(leader_event()), Some(Action::SendLeaderKey));
    assert_eq!(sm.state(), KeyState::Normal);
}

#[test]
fn test_ctrl_v_is_not_intercepted() {
    let mut sm = KeyStateMachine::new(Keymap::default_preset(), Duration::from_millis(400));
    let ev = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    assert_eq!(sm.process_key(ev), None);
}

#[test]
fn test_leader_followers_ignore_ctrl_and_unbound_uppercase() {
    let mut sm = KeyStateMachine::new(Keymap::default_preset(), Duration::from_millis(400));
    sm.process_key(leader_event());
    let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
    assert_eq!(sm.process_key(ctrl_x), None);

    // 'X' has no binding and must not fall back to 'x' (close pane)
    sm.process_key(leader_event());
    let shift_x = KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT);
    assert_eq!(sm.process_key(shift_x), None);
}

#[test]
fn test_leader_in_passthrough_returns_to_passthrough() {
    let mut sm = KeyStateMachine::new(Keymap::default_preset(), Duration::from_millis(400));
    sm.toggle_passthrough();
    assert_eq!(sm.process_key(leader_event()), None);
    assert_eq!(sm.state(), KeyState::LeaderPending);
    let b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::empty());
    assert_eq!(sm.process_key(b), Some(Action::ToggleTabTray));
    assert_eq!(sm.state(), KeyState::Passthrough);

    // Leader+t from passthrough yields TogglePassthrough; applying it leaves passthrough
    sm.process_key(leader_event());
    let t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::empty());
    assert_eq!(sm.process_key(t), Some(Action::TogglePassthrough));
    sm.toggle_passthrough();
    assert_eq!(sm.state(), KeyState::Normal);
}

#[test]
fn test_workspace_number_keys_switch_by_position() {
    let mut sm = KeyStateMachine::new(Keymap::default_preset(), Duration::from_millis(400));
    let alt_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
    assert_eq!(sm.process_key(alt_1), Some(Action::SwitchWorkspaceIndex(0)));
}
