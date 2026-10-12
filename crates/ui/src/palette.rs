use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::command::{Command, CommandRegistry};
use tag_core::theme::Theme;

#[derive(Default)]
pub struct PaletteState {
    pub is_open: bool,
    pub query: String,
    pub selected_index: usize,
}

pub fn render_command_palette(
    frame: &mut Frame,
    total_area: Rect,
    registry: &CommandRegistry,
    state: &PaletteState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    // Centered modal layout (width 60%, height 50%)
    let modal_width = (total_area.width * 6 / 10)
        .clamp(40, 100)
        .min(total_area.width);
    let modal_height = (total_area.height * 5 / 10)
        .clamp(12, 30)
        .min(total_area.height);
    let modal_x = (total_area.width.saturating_sub(modal_width)) / 2;
    let modal_y = (total_area.height.saturating_sub(modal_height)) / 3;

    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Command Palette (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Search bar area
    let search_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };

    let search_line = format!(" > {}_", state.query);
    let search_p = Paragraph::new(search_line).style(theme.title_focused);
    frame.render_widget(search_p, search_area);

    // Fuzzy match commands
    let matcher = SkimMatcherV2::default();
    let mut matched_commands: Vec<(&Command, i64)> = registry
        .commands()
        .iter()
        .filter_map(|cmd| {
            if state.query.is_empty() {
                Some((cmd, 0))
            } else {
                let score = matcher
                    .fuzzy_match(&cmd.name, &state.query)
                    .or_else(|| matcher.fuzzy_match(&cmd.description, &state.query))
                    .or_else(|| matcher.fuzzy_match(cmd.category.name(), &state.query));
                score.map(|s| (cmd, s))
            }
        })
        .collect();

    if !state.query.is_empty() {
        matched_commands.sort_by_key(|a| std::cmp::Reverse(a.1));
    }

    let mut list_items = Vec::new();
    for (idx, (cmd, _)) in matched_commands.iter().enumerate() {
        let is_selected = idx == state.selected_index;
        let mut spans = Vec::new();

        let prefix = if is_selected { "> " } else { "  " };
        spans.push(Span::raw(prefix));

        spans.push(Span::styled(
            format!("[{}] ", cmd.category.name()),
            theme.info.add_modifier(Modifier::DIM),
        ));

        spans.push(Span::styled(
            format!("{} ", cmd.name),
            if is_selected {
                theme.title_focused
            } else {
                theme.title_unfocused
            },
        ));

        if let Some(ref sc) = cmd.default_shortcut {
            spans.push(Span::styled(
                format!("({}) ", sc),
                theme.accent.add_modifier(Modifier::BOLD),
            ));
        }

        spans.push(Span::styled(
            cmd.description.clone(),
            theme.text.add_modifier(Modifier::DIM),
        ));

        let row_style = if is_selected {
            Style::default().bg(Color::Rgb(40, 44, 52))
        } else {
            Style::default()
        };

        list_items.push(ListItem::new(Line::from(spans)).style(row_style));
    }

    let list = List::new(list_items);
    frame.render_widget(list, list_area);
}

pub fn get_selected_command<'a>(
    registry: &'a CommandRegistry,
    state: &PaletteState,
) -> Option<&'a Command> {
    let matcher = SkimMatcherV2::default();
    let mut matched_commands: Vec<(&Command, i64)> = registry
        .commands()
        .iter()
        .filter_map(|cmd| {
            if state.query.is_empty() {
                Some((cmd, 0))
            } else {
                let score = matcher
                    .fuzzy_match(&cmd.name, &state.query)
                    .or_else(|| matcher.fuzzy_match(&cmd.description, &state.query))
                    .or_else(|| matcher.fuzzy_match(cmd.category.name(), &state.query));
                score.map(|s| (cmd, s))
            }
        })
        .collect();

    if !state.query.is_empty() {
        matched_commands.sort_by_key(|a| std::cmp::Reverse(a.1));
    }

    matched_commands.get(state.selected_index).map(|(c, _)| *c)
}
