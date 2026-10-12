use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

#[derive(Default)]
pub struct TourState {
    pub is_active: bool,
    pub step: usize,
}

pub struct TourStep {
    pub title: &'static str,
    pub body: &'static str,
    pub hint: &'static str,
}

pub const TOUR_STEPS: &[TourStep] = &[
    TourStep {
        title: "Welcome to TAG",
        body: "TAG is a developer multiplexer built for autonomous coding agents, Git worktree orchestration, and high-performance terminal workflows.",
        hint: "Press Space or Enter to continue (Esc to skip)",
    },
    TourStep {
        title: "The Leader Key",
        body: "TAG uses a non-conflicting leader key: Ctrl+Space.\nPress Ctrl+Space and wait a fraction of a second to view available chords in the Which-Key popup.",
        hint: "Press Space or Enter to continue",
    },
    TourStep {
        title: "Splits & Workspaces",
        body: "Split panes horizontally with Leader+- or vertically with Leader+|.\nSwitch workspaces with Alt+1..9 or navigate panes with Alt+h/j/k/l.",
        hint: "Press Space or Enter to continue",
    },
    TourStep {
        title: "Tab Tray & Command Palette",
        body: "Open the Tab Tray (Leader+b) to view all panes across all workspaces.\nPress Leader+p anytime to fuzzy search and run every command.",
        hint: "Press Space or Enter to continue",
    },
    TourStep {
        title: "Agent Supervision & Attention Queue",
        body: "When coding agents need permissions or finish turns, TAG notifies you.\nOpen the Attention Queue with Leader+a to respond immediately.",
        hint: "Press Space or Enter to finish the tour",
    },
];

pub fn render_tour(frame: &mut Frame, total_area: Rect, state: &TourState, theme: &Theme) {
    if !state.is_active || state.step >= TOUR_STEPS.len() {
        return;
    }

    let step_info = &TOUR_STEPS[state.step];

    let width = 64.min(total_area.width);
    let height = 12.min(total_area.height);
    let x = (total_area.width.saturating_sub(width)) / 2;
    let y = (total_area.height.saturating_sub(height)) / 2;

    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, area);

    let title = format!(
        " Quick Tour ({}/{}) - {} ",
        state.step + 1,
        TOUR_STEPS.len(),
        step_info.title
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(theme.accent);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    lines.push(Line::raw(""));
    for line in step_info.body.lines() {
        lines.push(Line::from(vec![Span::styled(line, theme.text)]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        step_info.hint,
        theme.title_focused.add_modifier(Modifier::BOLD),
    )]));

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}
