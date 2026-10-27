use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use std::collections::HashMap;

use crate::snippets::SnippetBank;

#[derive(Default)]
pub struct SnippetOverlayState {
    pub is_open: bool,
    pub selected_index: usize,
    pub search_query: String,
    /// Active `{{placeholder}}` prompt while filling in a template
    pub fill: Option<SnippetFillState>,
}

/// Collects values for a template's `{{placeholders}}` one at a time.
#[derive(Debug, Clone)]
pub struct SnippetFillState {
    pub template: String,
    pub placeholders: Vec<String>,
    pub values: HashMap<String, String>,
    pub current: usize,
    pub input: String,
}

impl SnippetFillState {
    pub fn current_placeholder(&self) -> &str {
        self.placeholders
            .get(self.current)
            .map(String::as_str)
            .unwrap_or("")
    }
}

impl SnippetOverlayState {
    /// Start inserting `template`. Returns the text to paste immediately when the
    /// template has no placeholders; otherwise starts prompting and returns None.
    pub fn begin_fill(&mut self, template: &str) -> Option<String> {
        let placeholders = SnippetBank::extract_placeholders(template);
        if placeholders.is_empty() {
            self.fill = None;
            return Some(template.to_string());
        }
        self.fill = Some(SnippetFillState {
            template: template.to_string(),
            placeholders,
            values: HashMap::new(),
            current: 0,
            input: String::new(),
        });
        None
    }

    /// Accept the value typed for the current placeholder. Returns the rendered text
    /// once every placeholder has a value.
    pub fn submit_fill_value(&mut self) -> Option<String> {
        let fill = self.fill.as_mut()?;
        let name = fill.current_placeholder().to_string();
        fill.values.insert(name, std::mem::take(&mut fill.input));
        fill.current += 1;
        if fill.current < fill.placeholders.len() {
            return None;
        }
        let fill = self.fill.take()?;
        Some(SnippetBank::render(&fill.template, &fill.values))
    }
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
        .title(" Snippet Bank (~/.config/tag/snippets.toml | Enter: Insert, Esc: Close) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut list_items = Vec::new();
    for (idx, snip) in bank.snippets.iter().enumerate() {
        let is_selected = idx == state.selected_index;
        let mut spans = Vec::new();

        let prefix = if is_selected { "> " } else { "  " };
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
            theme.selection
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

    // Placeholder prompt replaces the preview while filling in a template
    if let Some(ref fill) = state.fill {
        let prompt_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(4),
            width: inner.width,
            height: 4,
        };
        let p_block = Block::default()
            .title(format!(
                " Fill placeholder {}/{} (Enter: next, Esc: cancel) ",
                fill.current + 1,
                fill.placeholders.len()
            ))
            .borders(Borders::TOP)
            .border_style(theme.border_focused);
        let p = Paragraph::new(format!(
            "{{{{{}}}}} = {}_",
            fill.current_placeholder(),
            fill.input
        ))
        .style(theme.title_focused)
        .block(p_block);
        frame.render_widget(p, prompt_area);
        return;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fill_prompts_for_each_placeholder() {
        let mut state = SnippetOverlayState::default();
        assert_eq!(state.begin_fill("plain").as_deref(), Some("plain"));

        assert_eq!(state.begin_fill("cargo test {{name}} -- {{flag}}"), None);
        state.fill.as_mut().unwrap().input = "parser".into();
        assert_eq!(state.submit_fill_value(), None);
        assert_eq!(state.fill.as_ref().unwrap().current_placeholder(), "flag");
        state.fill.as_mut().unwrap().input = "--nocapture".into();
        assert_eq!(
            state.submit_fill_value().as_deref(),
            Some("cargo test parser -- --nocapture")
        );
        assert!(state.fill.is_none());
    }
}
