use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

pub fn render_empty_state(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.width < 10 || area.height < 5 {
        return;
    }

    let ascii_art = [
        r"  ████████╗  █████╗   ██████╗ ",
        r"  ╚══██╔══╝ ██╔══██╗ ██╔════╝ ",
        r"     ██║    ███████║ ██║  ███╗",
        r"     ██║    ██╔══██║ ██║   ██║",
        r"     ██║    ██║  ██║ ╚██████╔╝",
        r"     ╚═╝    ╚═╝  ╚═╝  ╚═════╝ ",
    ];

    let mut lines = Vec::new();

    // Spacing before logo
    let logo_height = ascii_art.len() as u16;
    let card_height = 12u16;
    let total_content_height = logo_height + 2 + card_height;

    let top_pad = if area.height > total_content_height {
        (area.height - total_content_height) / 2
    } else {
        0
    };

    for _ in 0..top_pad {
        lines.push(Line::raw(""));
    }

    // ASCII Art Logo
    for row in ascii_art {
        lines.push(
            Line::from(vec![Span::styled(
                row,
                theme.accent.add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
        );
    }

    // Subtitle
    lines.push(
        Line::from(vec![
            Span::styled("// ", theme.accent),
            Span::styled(
                "TERMINAL AGENT ORCHESTRATOR & MULTIPLEXER",
                theme.title_focused.add_modifier(Modifier::BOLD),
            ),
            Span::styled(" //", theme.accent),
        ])
        .alignment(Alignment::Center),
    );
    lines.push(Line::raw(""));

    // Shortcuts Box
    let card_width = 56u16.min(area.width.saturating_sub(4));
    let card_x = area.x + (area.width.saturating_sub(card_width)) / 2;
    let card_y = area.y + top_pad + logo_height + 2;

    // Render text lines above card
    let header_p = Paragraph::new(lines);
    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: top_pad + logo_height + 2,
    };
    frame.render_widget(header_p, header_area);

    // Card widget
    if area.height >= card_y + card_height {
        let card_area = Rect {
            x: card_x,
            y: card_y,
            width: card_width,
            height: card_height,
        };

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border_focused)
            .title(" Workspace Empty - Quick Actions ");

        let inner = card_block.inner(card_area);
        frame.render_widget(card_block, card_area);

        let card_lines = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled("  [ c ] or [ Enter ]  ", theme.title_focused),
                Span::styled("New Terminal Pane", theme.text),
            ]),
            Line::from(vec![
                Span::styled("  [ ^Space w ]        ", theme.title_focused),
                Span::styled("New Workspace", theme.text),
            ]),
            Line::from(vec![
                Span::styled("  [ ^Space W ]        ", theme.title_focused),
                Span::styled("Close Workspace", theme.text),
            ]),
            Line::from(vec![
                Span::styled("  [ ^Space p ]        ", theme.title_focused),
                Span::styled("Command Palette", theme.text),
            ]),
            Line::from(vec![
                Span::styled("  [ ^Space b ]        ", theme.title_focused),
                Span::styled("Tab Tray Navigator", theme.text),
            ]),
            Line::from(vec![
                Span::styled("  [ ^Space ? ]        ", theme.title_focused),
                Span::styled("Guide & Keybindings", theme.text),
            ]),
            Line::raw(""),
            Line::from(vec![Span::styled(
                "  Click anywhere or press [c] / [Enter] to spawn shell",
                theme.info.add_modifier(Modifier::DIM),
            )]),
        ];

        let card_p = Paragraph::new(card_lines);
        frame.render_widget(card_p, inner);
    }
}
