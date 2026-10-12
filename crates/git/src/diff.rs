use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn format_diff_lines(diff_text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for raw_line in diff_text.lines() {
        if raw_line.starts_with('+') && !raw_line.starts_with("+++") {
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default().fg(Color::Green),
            )]));
        } else if raw_line.starts_with('-') && !raw_line.starts_with("---") {
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default().fg(Color::Red),
            )]));
        } else if raw_line.starts_with("@@") {
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));
        } else if raw_line.starts_with("diff ") || raw_line.starts_with("index ") {
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default().fg(Color::Yellow),
            )]));
        } else {
            lines.push(Line::from(vec![Span::styled(
                raw_line.to_string(),
                Style::default().fg(Color::Gray),
            )]));
        }
    }

    lines
}
