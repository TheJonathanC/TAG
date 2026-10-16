use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;

use tag_core::theme::Theme;
use tag_keys::Keymap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhichKeyEntry {
    pub key_label: String,
    pub desc: &'static str,
    pub sort_key: (u8, char),
}

/// Formats a collection of characters into a compact sequence or group representation.
/// E.g.
/// - ['1', '2', ..., '9'] -> "1-9"
/// - [',', 'R'] -> ",/R"
/// - ['m', 'z'] -> "m/z"
/// - [' '] -> "Space"
/// - ['c'] -> "c"
pub fn format_key_sequence(keys: &[char]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    if keys.len() == 1 {
        return match keys[0] {
            ' ' => "Space".to_string(),
            c => c.to_string(),
        };
    }

    let mut sorted = keys.to_vec();
    sorted.sort_unstable();

    let mut chunks: Vec<String> = Vec::new();
    let mut run_start = sorted[0];
    let mut run_end = sorted[0];
    let mut run_len = 1;

    for &ch in &sorted[1..] {
        if ch as u32 == run_end as u32 + 1 {
            run_end = ch;
            run_len += 1;
        } else {
            if run_len >= 3 {
                chunks.push(format!("{}-{}", run_start, run_end));
            } else if run_len == 2 {
                chunks.push(format!("{}/{}", run_start, run_end));
            } else {
                let s = if run_start == ' ' {
                    "Space"
                } else {
                    &run_start.to_string()
                };
                chunks.push(s.to_string());
            }
            run_start = ch;
            run_end = ch;
            run_len = 1;
        }
    }

    if run_len >= 3 {
        chunks.push(format!("{}-{}", run_start, run_end));
    } else if run_len == 2 {
        chunks.push(format!("{}/{}", run_start, run_end));
    } else {
        let s = if run_start == ' ' {
            "Space"
        } else {
            &run_start.to_string()
        };
        chunks.push(s.to_string());
    }

    chunks.join("/")
}

pub fn get_grouped_bindings(keymap: &Keymap) -> Vec<WhichKeyEntry> {
    let mut desc_to_keys: HashMap<&'static str, Vec<char>> = HashMap::new();
    for (&ch, (_action, desc)) in &keymap.leader_bindings {
        desc_to_keys.entry(desc).or_default().push(ch);
    }

    let mut entries: Vec<WhichKeyEntry> = desc_to_keys
        .into_iter()
        .map(|(desc, keys)| {
            let min_char = *keys.iter().min().unwrap_or(&' ');
            let key_label = format_key_sequence(&keys);
            let category = if min_char.is_ascii_digit() {
                0
            } else if min_char.is_ascii_alphabetic() {
                1
            } else {
                2
            };
            WhichKeyEntry {
                key_label,
                desc,
                sort_key: (category, min_char),
            }
        })
        .collect();

    entries.sort_by_key(|e| e.sort_key);
    entries
}

pub fn render_which_key(frame: &mut Frame, total_area: Rect, keymap: &Keymap, theme: &Theme) {
    let entries = get_grouped_bindings(keymap);

    let inner_width = total_area.width.saturating_sub(2) as usize;
    let col_width = 26usize;
    let chunk_size = (inner_width / col_width).clamp(1, 6);
    let num_rows = entries.len().div_ceil(chunk_size);

    let height = (num_rows as u16 + 2)
        .min(total_area.height.saturating_sub(1))
        .max(4);

    let area = Rect {
        x: 0,
        y: total_area.height.saturating_sub(height + 1),
        width: total_area.width,
        height,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" ^Space Chords (Which-Key) ")
        .borders(Borders::ALL)
        .border_style(theme.accent);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    for chunk in entries.chunks(chunk_size) {
        let mut spans = Vec::new();
        for entry in chunk {
            let key_str = format!(" [{}]", entry.key_label);
            spans.push(Span::styled(
                format!("{:<8}", key_str),
                theme.title_focused.add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!("{:<18}", entry.desc), theme.text));
        }
        lines.push(Line::from(spans));
    }

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_key_sequence_digits() {
        let keys = vec!['1', '2', '3', '4', '5', '6', '7', '8', '9'];
        assert_eq!(format_key_sequence(&keys), "1-9");
    }

    #[test]
    fn test_format_key_sequence_discrete() {
        let keys = vec![',', 'R'];
        assert_eq!(format_key_sequence(&keys), ",/R");
    }

    #[test]
    fn test_format_key_sequence_two_keys() {
        let keys = vec!['m', 'z'];
        assert_eq!(format_key_sequence(&keys), "m/z");
    }

    #[test]
    fn test_format_key_sequence_single() {
        assert_eq!(format_key_sequence(&['c']), "c");
        assert_eq!(format_key_sequence(&[' ']), "Space");
    }

    #[test]
    fn test_default_keymap_no_duplicates_and_collapsed_workspace() {
        let keymap = Keymap::default_preset();
        let entries = get_grouped_bindings(&keymap);

        // Check workspace switching is collapsed to 1-9
        let ws_entry = entries
            .iter()
            .find(|e| e.desc == "Switch workspace")
            .expect("Switch workspace entry found");
        assert_eq!(ws_entry.key_label, "1-9");

        // Verify there are no duplicate descriptions anywhere in the list
        let mut seen = std::collections::HashSet::new();
        for entry in &entries {
            assert!(
                seen.insert(entry.desc),
                "Duplicate description found in which-key: {}",
                entry.desc
            );
        }
    }
}
