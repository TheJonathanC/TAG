use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
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
    /// First visible list row (rows include workspace headers); kept by render
    pub scroll_offset: usize,
}

/// Items matching the tray search query (shared by rendering, keys and mouse).
pub fn filter_tray_items<'a>(items: &'a [TrayItem], query: &str) -> Vec<&'a TrayItem> {
    let query = query.to_lowercase();
    items
        .iter()
        .filter(|item| {
            query.is_empty()
                || item.title.to_lowercase().contains(&query)
                || item.workspace_name.to_lowercase().contains(&query)
                || item.icon.to_lowercase().contains(&query)
                || item
                    .branch
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query)
        })
        .collect()
}

/// List rows as rendered: `None` for a workspace header, `Some(i)` for filtered item `i`.
fn tray_rows(filtered: &[&TrayItem]) -> Vec<Option<usize>> {
    let mut rows = Vec::new();
    let mut current_ws: Option<WorkspaceId> = None;
    for (idx, item) in filtered.iter().enumerate() {
        if current_ws != Some(item.workspace_id) {
            current_ws = Some(item.workspace_id);
            rows.push(None);
        }
        rows.push(Some(idx));
    }
    rows
}

/// (list area, optional search line area) inside the tray block
fn tray_areas(area: Rect, state: &TabTrayState) -> (Rect, Option<Rect>) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if state.is_searching || !state.search_query.is_empty() {
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
    }
}

/// Map a click at screen `row` inside the tray `area` to an index into the filtered items.
pub fn tray_item_at_row(
    area: Rect,
    items: &[TrayItem],
    state: &TabTrayState,
    row: u16,
) -> Option<usize> {
    let (list_area, _) = tray_areas(area, state);
    if row < list_area.y || row >= list_area.y + list_area.height {
        return None;
    }
    let filtered = filter_tray_items(items, &state.search_query);
    let rows = tray_rows(&filtered);
    rows.get(state.scroll_offset + (row - list_area.y) as usize)
        .copied()
        .flatten()
}

pub fn render_tab_tray(
    frame: &mut Frame,
    area: Rect,
    items: &[TrayItem],
    state: &mut TabTrayState,
    theme: &Theme,
) {
    if !state.is_open || area.width < 10 {
        return;
    }

    frame.render_widget(Clear, area);

    let filtered_items = filter_tray_items(items, &state.search_query);

    let block = Block::default()
        .title(" Tab Tray (Ctrl+Space b) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Search header if searching or has query
    let (list_area, search_area) = tray_areas(area, state);
    if let Some(s_area) = search_area {
        let search_text = format!("Search: {}_", state.search_query);
        let p = Paragraph::new(search_text).style(theme.accent);
        frame.render_widget(p, s_area);
    }

    // Keep the selected row (and its workspace header when possible) in view
    let rows = tray_rows(&filtered_items);
    let visible = list_area.height as usize;
    if let Some(sel_row) = rows.iter().position(|r| *r == Some(state.selected_index)) {
        let want_top = if sel_row > 0 && rows[sel_row - 1].is_none() {
            sel_row - 1
        } else {
            sel_row
        };
        if want_top < state.scroll_offset {
            state.scroll_offset = want_top;
        } else if visible > 0 && sel_row >= state.scroll_offset + visible {
            state.scroll_offset = sel_row + 1 - visible;
        }
    }
    state.scroll_offset = state
        .scroll_offset
        .min(rows.len().saturating_sub(visible.max(1)));

    // Build list items
    let mut list_items = Vec::new();
    for (pos, row) in rows
        .iter()
        .enumerate()
        .skip(state.scroll_offset)
        .take(visible)
    {
        let Some(idx) = *row else {
            // Workspace header: name of the workspace of the item right after it
            let next = rows[pos + 1..].iter().find_map(|r| *r);
            let name = next
                .and_then(|i| filtered_items.get(i))
                .map(|it| it.workspace_name.as_str())
                .unwrap_or("");
            list_items.push(ListItem::new(Line::from(vec![Span::styled(
                format!(" Workspace: {} ", name),
                theme
                    .text
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )])));
            continue;
        };
        let item = filtered_items[idx];

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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pid: u32, ws: usize) -> TrayItem {
        TrayItem {
            pane_id: PaneId(pid),
            workspace_id: WorkspaceId(ws),
            workspace_name: format!("WS-{}", ws),
            title: format!("pane {}", pid),
            icon: "[sh]",
            branch: None,
            status: "running",
            is_visible: false,
            is_minimized: false,
            needs_attention: false,
        }
    }

    #[test]
    fn test_tray_click_mapping_respects_headers_and_scroll() {
        let items = vec![item(1, 1), item(2, 1), item(3, 2)];
        let area = Rect::new(0, 0, 30, 20);
        let mut state = TabTrayState {
            is_open: true,
            ..Default::default()
        };
        // rows: [hdr ws1, item0, item1, hdr ws2, item2]; list starts at y=1 (inside border)
        assert_eq!(tray_item_at_row(area, &items, &state, 1), None);
        assert_eq!(tray_item_at_row(area, &items, &state, 2), Some(0));
        assert_eq!(tray_item_at_row(area, &items, &state, 5), Some(2));
        state.scroll_offset = 3;
        assert_eq!(tray_item_at_row(area, &items, &state, 2), Some(2));
    }
}
