use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::pane::{PaneId, WorkspaceId};
use tag_core::theme::Theme;

#[derive(Debug, Clone)]
pub struct TrayItem {
    pub pane_id: PaneId,
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub title: String,
    pub icon: &'static str,
    pub branch: Option<String>,
    pub status: &'static str,
    pub is_visible: bool,
    pub is_minimized: bool,
    pub needs_attention: bool,
}

#[derive(Default)]
pub struct TabTrayState {
    pub is_open: bool,
    pub selected_index: usize,
    pub search_query: String,
    pub is_searching: bool,
}

pub fn render_tab_tray(
    frame: &mut Frame,
    area: Rect,
    items: &[TrayItem],
    state: &TabTrayState,
    theme: &Theme,
) {
    if !state.is_open || area.width < 10 {
        return;
    }

    // Filter items by search query
    let filtered_items: Vec<&TrayItem> = items
        .iter()
        .filter(|item| {
            if state.search_query.is_empty() {
                true
            } else {
                let query = state.search_query.to_lowercase();
                item.title.to_lowercase().contains(&query)
                    || item.workspace_name.to_lowercase().contains(&query)
                    || item
                        .branch
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
            }
        })
        .collect();

    let block = Block::default()
        .title(" Tab Tray (Ctrl+Space b) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Search header if searching or has query
    let (list_area, search_area) = if state.is_searching || !state.search_query.is_empty() {
        let s_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let l_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        (l_area, Some(s_area))
    } else {
        let l_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };
        (l_area, None)
    };

    if let Some(s_area) = search_area {
        let search_text = format!("Search: {}_", state.search_query);
        let p = Paragraph::new(search_text).style(theme.accent);
        frame.render_widget(p, s_area);
    }

    // Build list items
    let mut list_items = Vec::new();
    let mut current_ws: Option<WorkspaceId> = None;

    for (idx, item) in filtered_items.iter().enumerate() {
        if current_ws != Some(item.workspace_id) {
            current_ws = Some(item.workspace_id);
            list_items.push(ListItem::new(Line::from(vec![Span::styled(
                format!(" Workspace: {} ", item.workspace_name),
                theme
                    .text
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )])));
        }

        let is_selected = idx == state.selected_index;
        let mut spans = Vec::new();

        let prefix = if is_selected { "> " } else { "  " };
        spans.push(Span::raw(prefix));

        spans.push(Span::styled(format!("{} ", item.icon), theme.text));
        spans.push(Span::styled(
            format!("{} ", item.title),
            if is_selected {
                theme.title_focused
            } else {
                theme.title_unfocused
            },
        ));

        if let Some(branch) = &item.branch {
            spans.push(Span::styled(
                format!("git:{} ", branch),
                theme.info.add_modifier(Modifier::DIM),
            ));
        }

        let status_style = match item.status {
            "running" => theme.accent,
            "awaiting-input" => theme.warning.add_modifier(Modifier::BOLD),
            "done" => theme.success,
            "error" | "exited" => theme.error,
            _ => theme.text,
        };
        spans.push(Span::styled(format!("[{}] ", item.status), status_style));

        if item.is_minimized {
            spans.push(Span::styled("[minimized] ", theme.warning));
        } else if item.is_visible {
            spans.push(Span::styled("[active] ", theme.info));
        }

        if item.needs_attention {
            spans.push(Span::styled(
                "[ATTN] ",
                theme.warning.add_modifier(Modifier::BOLD),
            ));
        }

        let row_style = if is_selected {
            theme.selection
        } else {
            Style::default()
        };

        list_items.push(ListItem::new(Line::from(spans)).style(row_style));
    }

    let list = List::new(list_items);
    frame.render_widget(list, list_area);

    // Hint line at bottom of tray
    let hint_area = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };
    let hint_text = if inner.width >= 58 {
        "j/k move · Enter open/restore · / search · s/v split · m min · x close"
    } else if inner.width >= 36 {
        "Enter restore · s/v split · m min · x close"
    } else {
        "Enter restore · x close"
    };
    let hint_p = Paragraph::new(hint_text).style(theme.text.add_modifier(Modifier::DIM));
    frame.render_widget(hint_p, hint_area);
}
