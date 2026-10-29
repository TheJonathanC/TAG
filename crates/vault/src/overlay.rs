use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use crate::store::VaultStore;
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VaultMode {
    #[default]
    Locked,
    Viewing,
    AddingKey,
    AddingValue,
}

#[derive(Default)]
pub struct VaultOverlayState {
    pub is_open: bool,
    pub mode: VaultMode,
    pub input_buffer: String,
    pub pending_key: String,
    pub selected_index: usize,
    pub status_message: Option<String>,
    /// First entry of a new vault's passphrase, awaiting confirmation
    pub pending_new_passphrase: Option<String>,
}

impl VaultOverlayState {
    pub fn clear_input(&mut self) {
        self.input_buffer.zeroize();
    }

    pub fn clear_pending_passphrase(&mut self) {
        if let Some(mut p) = self.pending_new_passphrase.take() {
            p.zeroize();
        }
    }
}

impl Drop for VaultOverlayState {
    fn drop(&mut self) {
        self.input_buffer.zeroize();
        self.pending_key.zeroize();
        self.clear_pending_passphrase();
    }
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

    let width = (total_area.width * 7 / 10)
        .clamp(50, 85)
        .min(total_area.width);
    let height = (total_area.height * 6 / 10)
        .clamp(14, 24)
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

    if !is_unlocked {
        let creating = !vault.exists();
        let (title, prompt, hint) = if !creating {
            (
                " Secrets Vault [LOCKED] (Enter passphrase) ",
                "Enter Master Passphrase:",
                "Press Enter to unlock, Esc to close",
            )
        } else if state.pending_new_passphrase.is_none() {
            (
                " Secrets Vault [NEW] (Choose a passphrase) ",
                "Choose a Master Passphrase for the new vault:",
                "Press Enter to continue, Esc to close",
            )
        } else {
            (
                " Secrets Vault [NEW] (Confirm passphrase) ",
                "Re-enter the Master Passphrase to confirm:",
                "Press Enter to create the vault, Esc to start over",
            )
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(theme.warning);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = vec![
            Line::raw(""),
            Line::from(vec![Span::styled(prompt, theme.title_focused)]),
            Line::from(vec![Span::styled(
                format!(" > {}_", "*".repeat(state.input_buffer.len())),
                theme.accent.add_modifier(Modifier::BOLD),
            )]),
            Line::raw(""),
            Line::from(vec![Span::styled(
                hint,
                theme.text.add_modifier(Modifier::DIM),
            )]),
        ];

        if let Some(msg) = &state.status_message {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![Span::styled(msg, theme.error)]));
        }

        let p = Paragraph::new(lines);
        frame.render_widget(p, inner);
        return;
    }

    // Unlocked states
    match state.mode {
        VaultMode::AddingKey => {
            let block = Block::default()
                .title(" Secrets Vault - Add Secret [Step 1/2: Name] ")
                .borders(Borders::ALL)
                .border_style(theme.accent);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let mut lines = vec![
                Line::raw(""),
                Line::from(vec![Span::styled(
                    "Enter Secret / Environment Variable Name:",
                    theme.title_focused,
                )]),
                Line::from(vec![Span::styled(
                    format!(" > {}_", state.input_buffer),
                    theme.accent.add_modifier(Modifier::BOLD),
                )]),
                Line::raw(""),
                Line::from(vec![Span::styled(
                    "Examples: ANTHROPIC_API_KEY, OPENAI_API_KEY, GITHUB_TOKEN",
                    theme.text.add_modifier(Modifier::DIM),
                )]),
                Line::from(vec![Span::styled(
                    "Press Enter to proceed, Esc to cancel",
                    theme.info.add_modifier(Modifier::DIM),
                )]),
            ];

            if let Some(msg) = &state.status_message {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![Span::styled(msg, theme.error)]));
            }

            let p = Paragraph::new(lines);
            frame.render_widget(p, inner);
        }
        VaultMode::AddingValue => {
            let block = Block::default()
                .title(" Secrets Vault - Add Secret [Step 2/2: Value] ")
                .borders(Borders::ALL)
                .border_style(theme.accent);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let masked = "*".repeat(state.input_buffer.len());
            let mut lines = vec![
                Line::raw(""),
                Line::from(vec![
                    Span::styled("Enter Secret Value for ", theme.title_focused),
                    Span::styled(
                        format!("'{}'", state.pending_key),
                        theme.accent.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":", theme.title_focused),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!(" > {}_", masked),
                        theme.accent.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" ({} chars)", state.input_buffer.len()),
                        theme.text.add_modifier(Modifier::DIM),
                    ),
                ]),
                Line::raw(""),
                Line::from(vec![Span::styled(
                    "Tip: Paste clipboard using Ctrl+Shift+V or Ctrl+Space v",
                    theme.info.add_modifier(Modifier::DIM),
                )]),
                Line::from(vec![Span::styled(
                    "Press Enter to save & encrypt, Esc to cancel",
                    theme.text.add_modifier(Modifier::DIM),
                )]),
            ];

            if let Some(msg) = &state.status_message {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![Span::styled(msg, theme.error)]));
            }

            let p = Paragraph::new(lines);
            frame.render_widget(p, inner);
        }
        VaultMode::Locked | VaultMode::Viewing => {
            let block = Block::default()
                .title(" Secrets Vault [UNLOCKED] (Esc to close) ")
                .borders(Borders::ALL)
                .border_style(theme.success);

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(3)])
                .split(inner);

            let keys = vault.list_keys();

            if keys.is_empty() {
                let lines = vec![
                    Line::raw(""),
                    Line::from(vec![Span::styled(
                        "No secrets stored yet.",
                        theme.text.add_modifier(Modifier::BOLD),
                    )]),
                    Line::raw(""),
                    Line::from(vec![Span::styled(
                        "Secrets stored in TAG are encrypted with ChaCha20-Poly1305 and",
                        theme.text.add_modifier(Modifier::DIM),
                    )]),
                    Line::from(vec![Span::styled(
                        "automatically injected into agent and terminal environments.",
                        theme.text.add_modifier(Modifier::DIM),
                    )]),
                ];
                let p = Paragraph::new(lines);
                frame.render_widget(p, chunks[0]);
            } else {
                let mut list_items = Vec::new();
                for (idx, key) in keys.iter().enumerate() {
                    let is_sel = idx == state.selected_index;
                    let prefix = if is_sel { " ▶ " } else { "   " };
                    let key_style = if is_sel {
                        theme.accent.add_modifier(Modifier::BOLD)
                    } else {
                        theme.title_focused
                    };
                    list_items.push(ListItem::new(Line::from(vec![
                        Span::styled(prefix, theme.accent.add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{:<28}", key), key_style),
                        Span::styled(" [••••••••••••] ", theme.info.add_modifier(Modifier::DIM)),
                        Span::styled(
                            " (Auto-injected into agents)",
                            theme.text.add_modifier(Modifier::DIM),
                        ),
                    ])));
                }
                let list = List::new(list_items);
                frame.render_widget(list, chunks[0]);
            }

            let mut footer_lines = vec![Line::from(vec![
                Span::styled(" [a] ", theme.accent.add_modifier(Modifier::BOLD)),
                Span::styled("Add Secret   ", theme.title_focused),
                Span::styled(" [d] ", theme.error.add_modifier(Modifier::BOLD)),
                Span::styled("Delete   ", theme.title_focused),
                Span::styled(" [↑/↓] ", theme.border_focused.add_modifier(Modifier::BOLD)),
                Span::styled("Navigate   ", theme.text),
                Span::styled(" [Esc] ", theme.border_focused.add_modifier(Modifier::BOLD)),
                Span::styled("Close", theme.text),
            ])];

            if let Some(msg) = &state.status_message {
                footer_lines.push(Line::from(vec![Span::styled(msg, theme.info)]));
            }

            let footer = Paragraph::new(footer_lines);
            frame.render_widget(footer, chunks[1]);
        }
    }
}
