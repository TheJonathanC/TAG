use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use crate::attention::AttentionQueue;

#[derive(Default)]
pub struct AttentionOverlayState {
    pub is_open: bool,
    pub selected_index: usize,
}

pub fn render_attention_overlay(
    frame: &mut Frame,
    total_area: Rect,
    queue: &AttentionQueue,
    state: &AttentionOverlayState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let width = (total_area.width * 6 / 10)
        .clamp(40, 90)
        .min(total_area.width);
    let height = (total_area.height * 5 / 10)
        .clamp(12, 24)
        .min(total_area.height);
    let x = (total_area.width.saturating_sub(width)) / 2;
    let y = (total_area.height.saturating_sub(height)) / 2;

    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Attention Queue (Enter: jump to pane, x: dismiss, Esc: close) ")
        .borders(Borders::ALL)
        .border_style(theme.warning);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if queue.is_empty() {
        let p = Paragraph::new(" No pending agent alerts. All caught up! ")
            .style(theme.text.add_modifier(Modifier::DIM));
        frame.render_widget(p, inner);
        return;
    }

    let mut list_items = Vec::new();
    for (idx, item) in queue.items().iter().enumerate() {
        let is_selected = idx == state.selected_index;
        let mut spans = Vec::new();

        let prefix = if is_selected { "> " } else { "  " };
        spans.push(Span::raw(prefix));

        spans.push(Span::styled(
            format!("[{}] ", item.agent),
            theme.accent.add_modifier(Modifier::BOLD),
        ));

        spans.push(Span::styled(
            format!("{:?}: ", item.event),
            theme.warning.add_modifier(Modifier::BOLD),
        ));

        spans.push(Span::styled(&item.message, theme.text));

        let row_style = if is_selected {
            theme.selection
        } else {
            Style::default()
        };

        list_items.push(ListItem::new(Line::from(spans)).style(row_style));
    }

    // Stateful render keeps the selected alert scrolled into view
    let list = List::new(list_items);
    let mut list_state = ListState::default().with_selected(Some(state.selected_index));
    frame.render_stateful_widget(list, inner, &mut list_state);
}
