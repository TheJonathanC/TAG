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
    pub version: &'a str,
    pub available_update: Option<&'a str>,
}

pub fn render_status_bar(frame: &mut Frame, area: Rect, props: StatusBarProps) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let mut spans = Vec::new();

    // Workspaces tabs
    let total_ws = props.layout_mgr.workspaces().len();
    for (i, ws) in props.layout_mgr.workspaces().iter().enumerate() {
        let is_active = ws.id == props.layout_mgr.active_workspace_id();
        let ws_style = if is_active {
            props.theme.title_focused
        } else {
            props.theme.title_unfocused
        };

        let badge = if is_active && total_ws > 1 {
            format!(" [{}: {} x] ", i + 1, ws.name)
        } else if is_active {
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
        KeyState::LeaderPending => ("^SPC...", props.theme.warning.add_modifier(Modifier::BOLD)),
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

    // Right-aligned elements: update indicator (if any) and version
    let mut right_spans = Vec::new();
    if let Some(update_ver) = props.available_update {
        right_spans.push(Span::styled(
            format!(" [▲ v{}] ", update_ver),
            props.theme.warning.add_modifier(Modifier::BOLD),
        ));
    }
    right_spans.push(Span::styled(
        format!("v{} ", props.version),
        props.theme.text.add_modifier(Modifier::DIM),
    ));

    let right_width: usize = right_spans.iter().map(|s| s.width()).sum();
    let current_len: usize = spans.iter().map(|s| s.width()).sum();
    let available_middle = (area.width as usize).saturating_sub(current_len + right_width);

    let hints = if is_zoomed {
        if available_middle >= 52 {
            "^Space: m Restore · x Close · p Palette · Alt+Enter"
        } else if available_middle >= 30 {
            "^Space: m Restore · p Palette"
        } else {
            ""
        }
    } else if available_middle >= 48 {
        "^Space: m Max · x Close · , Rename · p Palette"
    } else if available_middle >= 28 {
        "^Space: p Palette · ? Help"
    } else {
        ""
    };

    let hints_width = Span::raw(hints).width();
    if available_middle > hints_width {
        let pad = " ".repeat(available_middle - hints_width);
        spans.push(Span::raw(pad));
    }
    if !hints.is_empty() {
        spans.push(Span::styled(
            hints,
            props.theme.text.add_modifier(Modifier::DIM),
        ));
    }

    spans.extend(right_spans);

    let line = Line::from(spans);
    let p = Paragraph::new(line);
    frame.render_widget(p, area);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarClickTarget {
    Workspace(usize),
    CloseWorkspace(usize),
    AddWorkspace,
}

pub fn hit_test_status_bar(
    workspaces: &[tag_layout::Workspace],
    active_id: tag_core::pane::WorkspaceId,
    col: u16,
) -> Option<StatusBarClickTarget> {
    let mut current_x: u16 = 0;
    let total_ws = workspaces.len();
    for (i, ws) in workspaces.iter().enumerate() {
        let is_active = ws.id == active_id;
        let has_close = is_active && total_ws > 1;
        let name_width = Span::raw(&ws.name).width();
        let badge_len = if has_close {
            (8 + (i + 1).to_string().len() + name_width) as u16
        } else {
            (6 + (i + 1).to_string().len() + name_width) as u16
        };

        if col >= current_x && col < current_x + badge_len {
            if has_close
                && col >= current_x + badge_len.saturating_sub(4)
                && col < current_x + badge_len.saturating_sub(1)
            {
                return Some(StatusBarClickTarget::CloseWorkspace(i));
            }
            return Some(StatusBarClickTarget::Workspace(i));
        }
        current_x += badge_len;
    }

    if col >= current_x && col < current_x + 5 {
        return Some(StatusBarClickTarget::AddWorkspace);
    }
    None
}
