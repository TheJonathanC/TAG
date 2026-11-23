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

/// Display order of the queue's items: grouped by source (`agent`), groups in order of first
/// appearance, items in arrival order within a group. `selected_index` indexes this order.
pub fn display_order(queue: &AttentionQueue) -> Vec<usize> {
    let items = queue.items();
    let mut groups: Vec<&str> = Vec::new();
    for i in items {
        if !groups.contains(&i.agent.as_str()) {
            groups.push(&i.agent);
        }
    }
    groups
        .iter()
        .flat_map(|g| {
            items
                .iter()
                .enumerate()
                .filter(move |(_, i)| i.agent == *g)
                .map(|(idx, _)| idx)
        })
        .collect()
}

/// The item under the selection, honoring [`display_order`].
pub fn selected_item<'a>(
    queue: &'a AttentionQueue,
    state: &AttentionOverlayState,
) -> Option<&'a crate::attention::AttentionItem> {
    display_order(queue)
        .get(state.selected_index)
        .and_then(|&i| queue.items().get(i))
}

pub fn render_attention_overlay(
    frame: &mut Frame,
    total_area: Rect,
    queue: &AttentionQueue,
    state: &AttentionOverlayState,
    theme: &Theme,
    actions: &std::collections::HashMap<u64, String>,
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
        .title(" Attention Queue (Enter: jump · x: dismiss · p/m/r: quick action · Esc: close) ")
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
    let mut list_selected = 0;
    let mut last_group: Option<&str> = None;
    let items = queue.items();
    for (pos, &idx) in display_order(queue).iter().enumerate() {
        let item = &items[idx];
        if last_group != Some(item.agent.as_str()) {
            last_group = Some(item.agent.as_str());
            list_items.push(ListItem::new(Line::from(Span::styled(
                format!("── {} ", item.agent),
                theme.accent.add_modifier(Modifier::BOLD),
            ))));
        }
        let is_selected = pos == state.selected_index;
        if is_selected {
            list_selected = list_items.len();
        }
        let mut spans = vec![Span::raw(if is_selected { "> " } else { "  " })];
        spans.push(Span::styled(
            format!("{:?}: ", item.event),
            theme.warning.add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(&item.message, theme.text));
        if let Some(label) = actions.get(&item.id) {
            spans.push(Span::styled(
                format!("  {label}"),
                theme.success.add_modifier(Modifier::BOLD),
            ));
        }
        let row_style = if is_selected {
            theme.selection
        } else {
            Style::default()
        };
        list_items.push(ListItem::new(Line::from(spans)).style(row_style));
    }

    // Stateful render keeps the selected alert scrolled into view
    let list = List::new(list_items);
    let mut list_state = ListState::default().with_selected(Some(list_selected));
    frame.render_stateful_widget(list, inner, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tag_core::event::HookEvent;

    #[test]
    fn display_order_groups_by_source() {
        let mut q = AttentionQueue::new();
        q.push(
            None,
            "run a".into(),
            HookEvent::NeedsInput,
            "1".into(),
            None,
        );
        q.push(None, "claude".into(), HookEvent::TaskDone, "2".into(), None);
        q.push(
            None,
            "run a".into(),
            HookEvent::NeedsInput,
            "3".into(),
            None,
        );
        assert_eq!(display_order(&q), vec![0, 2, 1]);
        let st = AttentionOverlayState {
            is_open: true,
            selected_index: 1,
        };
        assert_eq!(selected_item(&q, &st).unwrap().message, "3");
    }
}
