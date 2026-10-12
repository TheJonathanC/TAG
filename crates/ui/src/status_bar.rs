use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use tag_core::theme::Theme;
use tag_keys::KeyState;
use tag_layout::LayoutManager;

pub struct StatusBarProps<'a> {
    pub layout_mgr: &'a LayoutManager,
    pub key_state: KeyState,
    pub dnd_active: bool,
    pub attention_count: usize,
    pub token_usage_summary: Option<&'a str>,
    pub theme: &'a Theme,
}

pub fn render_status_bar(frame: &mut Frame, area: Rect, props: StatusBarProps) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let mut spans = Vec::new();

    // Workspaces tabs
    for (i, ws) in props.layout_mgr.workspaces().iter().enumerate() {
        let is_active = ws.id == props.layout_mgr.active_workspace_id();
        let ws_style = if is_active {
            props.theme.title_focused
        } else {
            props.theme.title_unfocused
        };

        let badge = if is_active {
            format!(" [{}: {}] ", i + 1, ws.name)
        } else {
            format!("  {}: {}  ", i + 1, ws.name)
        };
        spans.push(Span::styled(badge, ws_style));
    }

    spans.push(Span::styled(
        " [+] ",
        props.theme.text.add_modifier(Modifier::DIM),
    ));
    spans.push(Span::raw(" | "));

    // Mode indicator
    let (mode_text, mode_style) = match props.key_state {
        KeyState::Normal => ("NORMAL", props.theme.info),
        KeyState::LeaderPending => (
            "LEADER...",
            props.theme.warning.add_modifier(Modifier::BOLD),
        ),
        KeyState::Passthrough => (
            "PASSTHROUGH",
            props.theme.error.add_modifier(Modifier::BOLD),
        ),
    };
    spans.push(Span::styled(format!(" [{}] ", mode_text), mode_style));

    // DND indicator
    if props.dnd_active {
        spans.push(Span::styled(
            " [DND] ",
            props.theme.error.add_modifier(Modifier::BOLD),
        ));
    }

    // Attention indicator
    if props.attention_count > 0 {
        spans.push(Span::styled(
            format!(" [!] {} pending ", props.attention_count),
            props.theme.warning.add_modifier(Modifier::BOLD),
        ));
    }

    // Token summary
    if let Some(summary) = props.token_usage_summary {
        spans.push(Span::styled(
            format!(" [*] {} ", summary),
            props.theme.accent,
        ));
    }

    // Maximized indicator
    let is_zoomed = props.layout_mgr.active_workspace().zoomed_pane.is_some();
    if is_zoomed {
        spans.push(Span::styled(
            " [MAXIMIZED] ",
            props.theme.accent.add_modifier(Modifier::BOLD),
        ));
    }

    // Fill space before hints
    let current_len: usize = spans.iter().map(|s| s.width()).sum();
    let hints = if is_zoomed {
        "Alt+Enter / Leader+m Restore · Leader+x Close · Leader+p Palette"
    } else {
        "Leader+m Max · Leader+x Close · Leader+, Rename · Leader+p Palette"
    };
    let spaces = area.width as usize - current_len.min(area.width as usize);

    if spaces > hints.len() + 2 {
        let pad = " ".repeat(spaces - hints.len() - 1);
        spans.push(Span::raw(pad));
        spans.push(Span::styled(
            hints,
            props.theme.text.add_modifier(Modifier::DIM),
        ));
    }

    let line = Line::from(spans);
    let p = Paragraph::new(line);
    frame.render_widget(p, area);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarClickTarget {
    Workspace(usize),
    AddWorkspace,
}

pub fn hit_test_status_bar(
    workspaces: &[tag_layout::Workspace],
    _active_id: tag_core::pane::WorkspaceId,
    col: u16,
) -> Option<StatusBarClickTarget> {
    let mut current_x: u16 = 0;
    for (i, ws) in workspaces.iter().enumerate() {
        let badge_len = (6 + (i + 1).to_string().len() + ws.name.len()) as u16;
        if col >= current_x && col < current_x + badge_len {
            return Some(StatusBarClickTarget::Workspace(i));
        }
        current_x += badge_len;
    }

    if col >= current_x && col < current_x + 5 {
        return Some(StatusBarClickTarget::AddWorkspace);
    }
    None
}
