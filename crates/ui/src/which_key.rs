use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;
use tag_keys::Keymap;

pub fn render_which_key(frame: &mut Frame, total_area: Rect, keymap: &Keymap, theme: &Theme) {
    let height = 8.min(total_area.height);
    let area = Rect {
        x: 0,
        y: total_area.height.saturating_sub(height + 1),
        width: total_area.width,
        height,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" ⌨  Leader Chords (Which-Key) ")
        .borders(Borders::ALL)
        .border_style(theme.accent);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut bindings: Vec<(&char, &&'static str)> = keymap
        .leader_bindings
        .iter()
        .map(|(ch, (_action, desc))| (ch, desc))
        .collect();
    bindings.sort_by_key(|(ch, _)| *ch);

    let mut lines = Vec::new();
    let chunk_size = 4;
    for chunk in bindings.chunks(chunk_size) {
        let mut spans = Vec::new();
        for (ch, desc) in chunk {
            let key_str = format!(" [{}] ", ch);
            spans.push(Span::styled(
                key_str,
                theme.title_focused.add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!("{:<18}", desc), theme.text));
        }
        lines.push(Line::from(spans));
    }

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}
