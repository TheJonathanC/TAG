use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;
use tag_keys::Keymap;

pub fn render_help_overlay(frame: &mut Frame, total_area: Rect, _keymap: &Keymap, theme: &Theme) {
    let modal_width = (total_area.width * 7 / 10)
        .clamp(50, 110)
        .min(total_area.width);
    let modal_height = (total_area.height * 7 / 10)
        .clamp(16, 36)
        .min(total_area.height);
    let modal_x = (total_area.width.saturating_sub(modal_width)) / 2;
    let modal_y = (total_area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" 📖 TAG Shortcut Cheat-Sheet (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let sections = vec![
        (
            "Navigation & Focus",
            vec![
                ("Alt+h / j / k / l", "Move focus left / down / up / right"),
                ("Alt+1..9", "Switch to workspace 1..9"),
                ("Leader+b", "Open Tab Tray (cross-workspace manager)"),
                ("Leader+Space", "Quick Switcher (fuzzy jump to any pane)"),
            ],
        ),
        (
            "Tiling & Layout",
            vec![
                ("Leader+|", "Split right (vertical split)"),
                ("Leader+-", "Split down (horizontal split)"),
                ("Leader+x", "Close focused pane"),
                ("Alt+Enter / Leader+z", "Toggle zoom (fullscreen pane)"),
                ("Leader+Z", "Undo last layout change"),
            ],
        ),
        (
            "Agent Supervision & Notifications",
            vec![
                ("Leader+a", "Attention queue (view pending requests)"),
                ("Leader+u", "Token usage & cost dashboard"),
                ("Leader+n", "Toggle Do Not Disturb"),
            ],
        ),
        (
            "Tools & Worktrees",
            vec![
                ("Leader+g", "Git worktree manager & diff viewer"),
                ("Leader+s", "Snippet bank (templated macros)"),
                ("Leader+i", "Clipboard image bridge"),
                ("Leader+v", "Encrypted secrets vault"),
                ("Leader+p", "Command Palette (search all actions)"),
                ("Leader+Esc", "Toggle passthrough mode (direct keys)"),
            ],
        ),
    ];

    let mut lines = Vec::new();
    for (sec_title, shortcuts) in sections {
        lines.push(Line::from(vec![Span::styled(
            format!("── {} ──", sec_title),
            theme.accent.add_modifier(Modifier::BOLD),
        )]));

        for (keys, desc) in shortcuts {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<24}", keys), theme.title_focused),
                Span::styled(desc, theme.text),
            ]));
        }
        lines.push(Line::raw(""));
    }

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}
