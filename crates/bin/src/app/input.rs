#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::{KeyCode, KeyModifiers};

/// Map Ctrl+<key> to its control byte (as xterm does).
pub(crate) fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Encode a key event the way xterm would, so programs in panes receive
/// navigation keys, function keys and modifier combinations.
pub(crate) fn key_event_to_bytes(key: crossterm::event::KeyEvent, app_cursor: bool) -> Vec<u8> {
    let mods = key.modifiers;
    let shift = mods.contains(KeyModifiers::SHIFT);
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;

    // CSI 1;<m><final> with modifiers, otherwise CSI/SS3 <final>
    let cursor = |fin: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{}{}", m, fin).into_bytes()
        } else if app_cursor {
            format!("\x1bO{}", fin).into_bytes()
        } else {
            format!("\x1b[{}", fin).into_bytes()
        }
    };
    // CSI <n>[;<m>]~
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{};{}~", n, m).into_bytes()
        } else {
            format!("\x1b[{}~", n).into_bytes()
        }
    };
    let with_alt = |mut bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    match key.code {
        KeyCode::Char(c) => {
            let bytes = if ctrl {
                ctrl_byte(c)
                    .map(|b| vec![b])
                    .unwrap_or_else(|| c.to_string().into_bytes())
            } else {
                c.to_string().into_bytes()
            };
            with_alt(bytes)
        }
        KeyCode::Enter => with_alt(vec![b'\r']),
        KeyCode::Backspace => with_alt(vec![if ctrl { 0x08 } else { 0x7f }]),
        KeyCode::Tab => with_alt(vec![b'\t']),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => with_alt(vec![0x1b]),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let fin = (b'P' + (n - 1)) as char;
            if m > 1 {
                format!("\x1b[1;{}{}", m, fin).into_bytes()
            } else {
                format!("\x1bO{}", fin).into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            tilde(code)
        }
        _ => Vec::new(),
    }
}

/// Printable character for a text field, ignoring Ctrl/Alt chords.
pub(crate) fn text_input_char(key: &crossterm::event::KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn test_navigation_and_function_keys_are_encoded() {
        let none = KeyModifiers::empty();
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Delete, none), false),
            b"\x1b[3~"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Home, none), false),
            b"\x1b[H"
        );
        assert_eq!(key_event_to_bytes(key(KeyCode::End, none), true), b"\x1bOF");
        assert_eq!(
            key_event_to_bytes(key(KeyCode::PageDown, none), false),
            b"\x1b[6~"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            b"\x1b[Z"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::F(1), none), false),
            b"\x1bOP"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::F(5), none), false),
            b"\x1b[15~"
        );
        assert_eq!(key_event_to_bytes(key(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(key_event_to_bytes(key(KeyCode::Up, none), false), b"\x1b[A");
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Right, KeyModifiers::CONTROL), false),
            b"\x1b[1;5C"
        );
    }

    #[test]
    fn test_modifier_chars_are_encoded() {
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Backspace, KeyModifiers::empty()), false),
            vec![0x7f]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            vec![3]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('\\'), KeyModifiers::CONTROL), false),
            vec![0x1c]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char(']'), KeyModifiers::CONTROL), false),
            vec![0x1d]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char(' '), KeyModifiers::CONTROL), false),
            vec![0]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('é'), KeyModifiers::empty()), false),
            "é".as_bytes()
        );
    }
}
