//! Run view overlay (lazygit-style): task tree of an orchestration run, a live tail of the
//! selected task's pane, and a bottom panel with its diff stat or last test output.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use tag_core::theme::Theme;

/// One task row.
#[derive(Debug, Clone, PartialEq)]
pub struct RunTaskRow {
    pub id: String,
    pub title: String,
    pub profile: String,
    /// snake_case task state (`working`, `review`, ...).
    pub state: String,
    pub elapsed: String,
    pub has_pane: bool,
}

/// Everything the view shows, computed by the app each frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunViewData {
    /// e.g. `Run: add-oauth [running] $0.84 / $5.00 (1/3)`.
    pub header: String,
    pub tasks: Vec<RunTaskRow>,
    /// Last lines of the selected task's pane.
    pub tail: String,
    /// Diff stat or test output for the selected task.
    pub bottom: String,
}

/// Overlay state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunViewState {
    pub is_open: bool,
    /// Index into the list of active runs.
    pub run_index: usize,
    /// Selected task row.
    pub selected: usize,
    /// Bottom panel shows the test output instead of the diff stat.
    pub show_tests: bool,
}

/// State glyph for a task (§7.3).
pub fn task_glyph(state: &str) -> &'static str {
    match state {
        "planned" => "○",
        "queued" => "◔",
        "working" => "●",
        "needs_input" => "?",
        "review" => "◐",
        "changes_requested" => "↻",
        "approved" => "◑",
        "merged" => "✓",
        "failed" => "✗",
        "skipped" => "–",
        _ => "·",
    }
}

/// The overlay's area: nearly the whole screen.
pub fn run_view_area(total: Rect) -> Rect {
    let w = total.width.saturating_sub(4).max(20).min(total.width);
    let h = total.height.saturating_sub(2).max(8).min(total.height);
    Rect {
        x: total.x + (total.width - w) / 2,
        y: total.y + (total.height - h) / 2,
        width: w,
        height: h,
    }
}

pub fn render_run_view(
    frame: &mut Frame,
    total: Rect,
    data: &RunViewData,
    state: &RunViewState,
    theme: &Theme,
) {
    let area = run_view_area(total);
    frame.render_widget(Clear, area);
    let outer = Block::default()
        .title(format!(" {} ", data.header))
        .borders(Borders::ALL)
        .border_style(theme.border_focused);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length((inner.height / 3).clamp(3, 12)),
            Constraint::Length(1),
        ])
        .split(inner);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[0]);

    // Task tree
    let items: Vec<ListItem> = if data.tasks.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            " (no tasks yet; the conductor is planning)",
            theme.text.add_modifier(Modifier::DIM),
        )))]
    } else {
        data.tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let glyph_style = match t.state.as_str() {
                    "merged" => theme.success,
                    "failed" => theme.error,
                    "needs_input" | "review" => theme.warning,
                    _ => theme.accent,
                };
                let line = Line::from(vec![
                    Span::raw(if i == state.selected { "> " } else { "  " }),
                    Span::styled(format!("{} ", task_glyph(&t.state)), glyph_style),
                    Span::styled(
                        format!("{:<4}", t.id),
                        theme.text.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{:<22} ", clip(&t.title, 22)), theme.text),
                    Span::styled(format!("{:<10} ", clip(&t.profile, 10)), theme.accent),
                    Span::styled(format!("{:<17} ", t.state), theme.text),
                    Span::styled(t.elapsed.clone(), theme.text.add_modifier(Modifier::DIM)),
                ]);
                let style = if i == state.selected {
                    theme.selection
                } else {
                    Style::default()
                };
                ListItem::new(line).style(style)
            })
            .collect()
    };
    let mut ls = ListState::default().with_selected(Some(state.selected));
    frame.render_stateful_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(theme.border_unfocused),
        ),
        cols[0],
        &mut ls,
    );

    // Live tail
    let tail_lines: Vec<&str> = data.tail.lines().collect();
    let keep = cols[1].height as usize;
    let start = tail_lines.len().saturating_sub(keep);
    let tail = tail_lines[start..].join("\n");
    frame.render_widget(Paragraph::new(tail).style(theme.text), cols[1]);

    // Bottom panel
    let bottom_title = if state.show_tests {
        " Last test run [Tab: diff stat] "
    } else {
        " Diff stat [Tab: tests] "
    };
    frame.render_widget(
        Paragraph::new(data.bottom.clone())
            .wrap(Wrap { trim: false })
            .style(theme.text)
            .block(
                Block::default()
                    .title(bottom_title)
                    .borders(Borders::TOP)
                    .border_style(theme.border_unfocused),
            ),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " [Enter] open  [e] peek  [d] diff  [t] test  [r] reply  [m] merge  [k] kill  [a] approve  [ / ] run  [Esc] close",
            theme.text.add_modifier(Modifier::DIM),
        ))),
        rows[2],
    );
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn renders_tasks_tail_and_bottom() {
        let data = RunViewData {
            header: "Run: add-oauth [running] $0.84 / $5.00".into(),
            tasks: vec![
                RunTaskRow {
                    id: "t1".into(),
                    title: "api-routes".into(),
                    profile: "agy-neon".into(),
                    state: "working".into(),
                    elapsed: "4m".into(),
                    has_pane: true,
                },
                RunTaskRow {
                    id: "t2".into(),
                    title: "ui-login".into(),
                    profile: "claude".into(),
                    state: "merged".into(),
                    elapsed: "9m".into(),
                    has_pane: false,
                },
            ],
            tail: "> writing src/auth/routes.rs\n> running cargo test".into(),
            bottom: " src/auth/routes.rs | 40 ++++".into(),
        };
        let state = RunViewState {
            is_open: true,
            ..Default::default()
        };
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| render_run_view(f, f.area(), &data, &state, &Theme::default()))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for needle in [
            "Run: add-oauth",
            "● t1",
            "✓ t2",
            "running cargo test",
            "routes.rs | 40",
            "[m] merge",
        ] {
            assert!(text.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn glyphs_match_spec() {
        assert_eq!(task_glyph("planned"), "○");
        assert_eq!(task_glyph("review"), "◐");
        assert_eq!(task_glyph("failed"), "✗");
    }
}
