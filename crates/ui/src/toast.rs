use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

#[derive(Debug, Clone)]
pub struct Toast {
    pub title: String,
    pub message: String,
    pub level: ToastLevel,
    pub created_at: DateTime<Utc>,
    pub duration_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warning,
    Error,
    Success,
}

pub fn render_toasts(frame: &mut Frame, total_area: Rect, toasts: &[Toast], theme: &Theme) {
    if toasts.is_empty() {
        return;
    }

    let toast_width = 36.min(total_area.width);
    let mut current_y = 1;

    for toast in toasts.iter().take(3) {
        let toast_height = 4.min(total_area.height.saturating_sub(current_y));
        if toast_height < 3 {
            break;
        }

        let area = Rect {
            x: total_area.width.saturating_sub(toast_width + 1),
            y: current_y,
            width: toast_width,
            height: toast_height,
        };

        frame.render_widget(Clear, area);

        let (border_style, icon) = match toast.level {
            ToastLevel::Info => (theme.info, "[i] "),
            ToastLevel::Warning => (theme.warning, "[!] "),
            ToastLevel::Error => (theme.error, "[x] "),
            ToastLevel::Success => (theme.success, "[ok] "),
        };

        let block = Block::default()
            .title(format!(" {}{} ", icon, toast.title))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = vec![Line::from(vec![Span::styled(
            &toast.message,
            theme.text.add_modifier(Modifier::BOLD),
        )])];
        let p = Paragraph::new(lines);
        frame.render_widget(p, inner);

        current_y += toast_height + 1;
    }
}
