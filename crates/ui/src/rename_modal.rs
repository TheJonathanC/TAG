use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::pane::PaneId;
use tag_core::theme::Theme;

#[derive(Debug, Clone)]
pub struct RenameModalState {
    pub pane_id: PaneId,
    pub input: String,
}

impl RenameModalState {
    pub fn new(pane_id: PaneId, current_title: String) -> Self {
        Self {
            pane_id,
            input: current_title,
        }
    }
}

pub fn render_rename_modal(
    frame: &mut Frame,
    total_area: Rect,
    state: &RenameModalState,
    theme: &Theme,
) {
    let modal_width = 54.min(total_area.width.saturating_sub(4));
    let modal_height = 7.min(total_area.height.saturating_sub(2));

    let modal_x = total_area.x + (total_area.width.saturating_sub(modal_width)) / 2;
    let modal_y = total_area.y + (total_area.height.saturating_sub(modal_height)) / 3;

    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Rename Pane ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let lines = vec![
        Line::from(vec![Span::styled(
            " Enter new title:",
            theme.text.add_modifier(Modifier::DIM),
        )]),
        Line::from(vec![
            Span::styled(" > ", theme.accent),
            Span::styled(
                format!("{}_", state.input),
                theme.title_focused.add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![Span::styled(
            " [Enter] Confirm · [Esc] Cancel",
            theme.text.add_modifier(Modifier::DIM),
        )]),
    ];

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}
