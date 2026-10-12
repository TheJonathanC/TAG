use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use crate::store::VaultStore;

#[derive(Default)]
pub struct VaultOverlayState {
    pub is_open: bool,
    pub input_buffer: String,
    pub status_message: Option<String>,
}

pub fn render_vault_overlay(
    frame: &mut Frame,
    total_area: Rect,
    vault: &VaultStore,
    state: &VaultOverlayState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let width = (total_area.width * 6 / 10)
        .clamp(40, 80)
        .min(total_area.width);
    let height = (total_area.height * 5 / 10)
        .clamp(12, 22)
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

    let is_unlocked = vault.is_unlocked();
    let border_style = if is_unlocked {
        theme.success
    } else {
        theme.warning
    };

    let title = if is_unlocked {
        " 🔒 Secrets Vault [UNLOCKED] (Esc to close) "
    } else {
        " 🔒 Secrets Vault [LOCKED] (Enter passphrase) "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !is_unlocked {
        let mut lines = Vec::new();
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled(
            "Enter Master Passphrase:",
            theme.title_focused,
        )]));

        let masked = "*".repeat(state.input_buffer.len());
        lines.push(Line::from(vec![Span::styled(
            format!(" > {}_", masked),
            theme.accent.add_modifier(Modifier::BOLD),
        )]));

        if let Some(msg) = &state.status_message {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![Span::styled(msg, theme.error)]));
        }

        let p = Paragraph::new(lines);
        frame.render_widget(p, inner);
    } else {
        let mut list_items = Vec::new();
        let keys = vault.list_keys();

        if keys.is_empty() {
            list_items.push(ListItem::new(Line::from(vec![Span::styled(
                "No secrets stored yet. Add secrets to inject them into agent environments.",
                theme.text.add_modifier(Modifier::DIM),
            )])));
        } else {
            for key in keys {
                list_items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("  🔑 {:<24}", key), theme.title_focused),
                    Span::styled(" [••••••••••••] ", theme.info.add_modifier(Modifier::DIM)),
                    Span::styled(" (Auto-injected into agents)", theme.text),
                ])));
            }
        }

        let list = List::new(list_items);
        frame.render_widget(list, inner);
    }
}
