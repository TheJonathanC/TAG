//! Floating "peek" overlay frame: a centered box (~80% of the screen) around a live pane.

use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;
use tag_core::theme::Theme;

/// The centered overlay rectangle (~80% of `screen`).
pub fn peek_area(screen: Rect) -> Rect {
    let w = (screen.width * 4 / 5).max(20).min(screen.width);
    let h = (screen.height * 4 / 5).max(6).min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

pub fn peek_title(pane_title: &str, input: bool) -> String {
    format!(
        " Peek: {pane_title}  [i] input {} · [Enter] open as pane · [Esc] close ",
        if input { "on" } else { "off" }
    )
}

/// Clear and draw the frame; returns the inner rectangle for the pane content.
pub fn render_peek_frame(
    frame: &mut Frame,
    screen: Rect,
    pane_title: &str,
    input: bool,
    theme: &Theme,
) -> Rect {
    let area = peek_area(screen);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(peek_title(pane_title, input))
        .borders(Borders::ALL)
        .border_style(if input {
            theme.warning
        } else {
            theme.border_focused
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn title_shows_hints() {
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let theme = Theme::default();
        t.draw(|f| {
            render_peek_frame(f, f.area(), "worker-1", false, &theme);
        })
        .unwrap();
        let area = peek_area(Rect::new(0, 0, 120, 30));
        let row: String = (area.x..area.x + area.width)
            .map(|x| t.backend().buffer()[(x, area.y)].symbol().to_string())
            .collect();
        assert!(row.contains("Peek: worker-1"), "{row}");
        assert!(row.contains("[i] input off"));
        assert!(row.contains("[Enter] open as pane"));
        assert!(row.contains("[Esc] close"));
        assert_eq!(area.width, 96);
    }
}
