use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use crate::snippets::SnippetBank;

#[derive(Default)]
pub struct SnippetOverlayState {
    pub is_open: bool,
    pub selected_index: usize,
    pub search_query: String,
}

pub fn render_snippet_overlay(
    frame: &mut Frame,
    total_area: Rect,
    bank: &SnippetBank,
    state: &SnippetOverlayState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let width = (total_area.width * 7 / 10)
        .clamp(50, 100)
        .min(total_area.width);
    let height = (total_area.height * 6 / 10)
        .clamp(14, 28)
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
        .title(" 📋 Snippet Bank (Enter to insert, Esc to close) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut list_items = Vec::new();
    for (idx, snip) in bank.snippets.iter().enumerate() {
        let is_selected = idx == state.selected_index;
        let mut spans = Vec::new();

        let prefix = if is_selected { " ▶ " } else { "   " };
        spans.push(Span::raw(prefix));

        spans.push(Span::styled(
            format!("{:<20} ", snip.name),
            if is_selected {
                theme.title_focused
            } else {
                theme.text
            },
        ));

        spans.push(Span::styled(
            format!("({}) ", snip.tags.join(", ")),
            theme.info.add_modifier(Modifier::DIM),
        ));

        spans.push(Span::styled(&snip.description, theme.text));

        let row_style = if is_selected {
            Style::default().bg(Color::Rgb(40, 44, 52))
        } else {
            Style::default()
        };

        list_items.push(ListItem::new(Line::from(spans)).style(row_style));
    }

    let list_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(5),
    };
    let list = List::new(list_items);
    frame.render_widget(list, list_area);

    // Template preview at bottom
    if let Some(snip) = bank.snippets.get(state.selected_index) {
        let preview_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(4),
            width: inner.width,
            height: 4,
        };

        let p_block = Block::default()
            .title(" Template Preview ")
            .borders(Borders::TOP)
            .border_style(theme.border_unfocused);

        let p = Paragraph::new(snip.template.clone())
            .style(theme.accent)
            .block(p_block);

        frame.render_widget(p, preview_area);
    }
}
