use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use ratatui::Frame;

use tag_core::command::{Command, CommandRegistry};
use tag_core::theme::Theme;

#[derive(Default, Debug, Clone)]
pub struct PaletteState {
    pub is_open: bool,
    pub query: String,
    pub selected_index: usize,
    pub scroll_offset: usize,
}

pub fn get_matched_commands<'a>(
    registry: &'a CommandRegistry,
    query: &str,
) -> Vec<(&'a Command, i64)> {
    let matcher = SkimMatcherV2::default();
    let mut matched_commands: Vec<(&Command, i64)> = registry
        .commands()
        .iter()
        .filter_map(|cmd| {
            if query.is_empty() {
                Some((cmd, 0))
            } else {
                let score = matcher
                    .fuzzy_match(&cmd.name, query)
                    .or_else(|| matcher.fuzzy_match(&cmd.description, query))
                    .or_else(|| matcher.fuzzy_match(cmd.category.name(), query));
                score.map(|s| (cmd, s))
            }
        })
        .collect();

    if !query.is_empty() {
        matched_commands.sort_by_key(|a| std::cmp::Reverse(a.1));
    }

    matched_commands
}

pub fn render_command_palette(
    frame: &mut Frame,
    total_area: Rect,
    registry: &CommandRegistry,
    state: &mut PaletteState,
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

    let matched_commands = get_matched_commands(registry, &state.query);
    let total_matches = matched_commands.len();

    // Clamp selected_index
    if total_matches > 0 && state.selected_index >= total_matches {
        state.selected_index = total_matches - 1;
    }

    let title = if total_matches > 0 {
        format!(
            " Command Palette [{}/{}] (Esc to close) ",
            state.selected_index + 1,
            total_matches
        )
    } else {
        " Command Palette [0/0] (Esc to close) ".to_string()
    };

    let block = Block::default()
        .title(title)
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

    let visible_count = list_area.height as usize;
    if visible_count == 0 || total_matches == 0 {
        return;
    }

    // Scroll window adjustment: ensure selected_index is always inside visible viewport
    if state.selected_index < state.scroll_offset {
        state.scroll_offset = state.selected_index;
    } else if state.selected_index >= state.scroll_offset + visible_count {
        state.scroll_offset = state.selected_index + 1 - visible_count;
    }

    // Clamp scroll_offset
    let max_scroll = total_matches.saturating_sub(visible_count);
    state.scroll_offset = state.scroll_offset.min(max_scroll);

    let start_idx = state.scroll_offset;
    let end_idx = (start_idx + visible_count).min(total_matches);

    let mut list_items = Vec::new();
    for (offset, (cmd, _)) in matched_commands[start_idx..end_idx].iter().enumerate() {
        let idx = start_idx + offset;
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

    // Render scrollbar if there are more items than visible
    if total_matches > visible_count {
        let mut scrollbar_state = ScrollbarState::new(total_matches).position(state.selected_index);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("#")
            .track_symbol(Some("|"));
        frame.render_stateful_widget(scrollbar, list_area, &mut scrollbar_state);
    }
}

pub fn get_selected_command<'a>(
    registry: &'a CommandRegistry,
    state: &PaletteState,
) -> Option<&'a Command> {
    let matched_commands = get_matched_commands(registry, &state.query);
    matched_commands.get(state.selected_index).map(|(c, _)| *c)
}
