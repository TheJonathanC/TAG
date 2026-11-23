//! k9s-style workers table overlay (pure widget: data in, rendering and hit-testing out).

use crate::card_strip::status_glyph;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::time::Instant;
use tag_core::pane::PaneId;
use tag_core::theme::Theme;

/// One worker row; built by the app every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRow {
    pub pane_id: PaneId,
    /// Agent status (`running`, `awaiting-input`, ...).
    pub status: String,
    pub name: String,
    pub profile: String,
    /// `task@run` when bound to a run, else empty.
    pub run_task: String,
    /// Task state if bound to a run, else the agent status.
    pub state: String,
    pub elapsed: String,
    pub last_line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkersFilter {
    #[default]
    All,
    NeedsMe,
    Running,
    Done,
}

impl WorkersFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::NeedsMe,
            Self::NeedsMe => Self::Running,
            Self::Running => Self::Done,
            Self::Done => Self::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::NeedsMe => "needs me",
            Self::Running => "running",
            Self::Done => "done",
        }
    }
    fn matches(self, status: &str) -> bool {
        match self {
            Self::All => true,
            Self::NeedsMe => is_needs_input(status),
            Self::Running => status == "running",
            Self::Done => matches!(status, "done" | "exited" | "failed"),
        }
    }
}

fn is_needs_input(status: &str) -> bool {
    matches!(status, "awaiting-input" | "awaiting_input")
}

#[derive(Debug, Default)]
pub struct WorkersTableState {
    pub is_open: bool,
    pub selected: usize,
    /// Selection is tracked by pane so re-sorting never moves it to another worker.
    pub selected_pane: Option<PaneId>,
    pub scroll: usize,
    pub filter: WorkersFilter,
    pub filter_text: String,
    pub is_filtering: bool,
    /// Pane awaiting `y/n` kill confirmation, with its display name.
    pub confirm_kill: Option<(PaneId, String)>,
    /// Inline reply: target pane, display name and typed text.
    pub reply: Option<(PaneId, String, String)>,
    /// Last click (time, pane) for double-click detection.
    pub last_click: Option<(Instant, PaneId)>,
}

impl WorkersTableState {
    pub fn open(&mut self) {
        *self = WorkersTableState {
            is_open: true,
            ..Default::default()
        };
    }

    /// Re-anchor the selection on `visible` (follow the selected pane, else clamp).
    pub fn sync(&mut self, visible: &[&WorkerRow]) {
        if visible.is_empty() {
            self.selected = 0;
            self.selected_pane = None;
            return;
        }
        if let Some(pos) = self
            .selected_pane
            .and_then(|p| visible.iter().position(|r| r.pane_id == p))
        {
            self.selected = pos;
        } else {
            self.selected = self.selected.min(visible.len() - 1);
            self.selected_pane = Some(visible[self.selected].pane_id);
        }
    }

    /// Move the selection by `delta`, clamped to the list.
    pub fn move_by(&mut self, delta: isize, visible: &[&WorkerRow]) {
        self.sync(visible);
        if visible.is_empty() {
            return;
        }
        let n = visible.len() as isize;
        self.selected = (self.selected as isize + delta).clamp(0, n - 1) as usize;
        self.selected_pane = Some(visible[self.selected].pane_id);
    }

    pub fn select_index(&mut self, idx: usize, visible: &[&WorkerRow]) {
        if let Some(r) = visible.get(idx) {
            self.selected = idx;
            self.selected_pane = Some(r.pane_id);
        }
    }

    pub fn selected_row<'a>(&self, visible: &[&'a WorkerRow]) -> Option<&'a WorkerRow> {
        self.selected_pane
            .and_then(|p| visible.iter().find(|r| r.pane_id == p).copied())
            .or_else(|| visible.get(self.selected).copied())
    }
}

fn rank(status: &str) -> u8 {
    if is_needs_input(status) {
        0
    } else if status == "running" {
        1
    } else {
        2
    }
}

/// Rows after applying the status filter and text filter, sorted: needs input, running, rest
/// (stable within each group).
pub fn visible_rows<'a>(
    rows: &'a [WorkerRow],
    filter: WorkersFilter,
    text: &str,
) -> Vec<&'a WorkerRow> {
    let needle = text.to_lowercase();
    let mut v: Vec<&WorkerRow> = rows
        .iter()
        .filter(|r| filter.matches(&r.status))
        .filter(|r| {
            needle.is_empty()
                || [&r.name, &r.profile, &r.run_task, &r.state, &r.last_line]
                    .iter()
                    .any(|f| f.to_lowercase().contains(&needle))
        })
        .collect();
    v.sort_by_key(|r| rank(&r.status));
    v
}

/// The overlay rectangle (~90% x 80%, centered).
pub fn workers_area(screen: Rect) -> Rect {
    let w = (screen.width * 9 / 10).max(40).min(screen.width);
    let h = (screen.height * 4 / 5).max(8).min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

/// Rows area: below the header line, above the footer line.
fn list_area(screen: Rect) -> Rect {
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(workers_area(screen));
    Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(2),
    )
}

/// Index into the visible rows for a click at screen (`col`, `row`), or `None` outside.
/// A click outside the overlay is reported by [`inside_workers`].
pub fn worker_row_at(
    screen: Rect,
    state: &WorkersTableState,
    count: usize,
    col: u16,
    row: u16,
) -> Option<usize> {
    let l = list_area(screen);
    if col < l.x || col >= l.x + l.width || row < l.y || row >= l.y + l.height {
        return None;
    }
    let idx = state.scroll + (row - l.y) as usize;
    (idx < count).then_some(idx)
}

pub fn inside_workers(screen: Rect, col: u16, row: u16) -> bool {
    let a = workers_area(screen);
    col >= a.x && col < a.x + a.width && row >= a.y && row < a.y + a.height
}

fn fit(s: &str, w: usize) -> String {
    let mut out: String = s.chars().take(w).collect();
    let n = out.chars().count();
    if s.chars().count() > w && w > 0 {
        out.pop();
        out.push('…');
    } else {
        out.extend(std::iter::repeat_n(' ', w - n));
    }
    out
}

pub fn render_workers_table(
    frame: &mut Frame,
    screen: Rect,
    rows: &[WorkerRow],
    state: &mut WorkersTableState,
    theme: &Theme,
) {
    let area = workers_area(screen);
    frame.render_widget(Clear, area);
    let vis = visible_rows(rows, state.filter, &state.filter_text);
    state.sync(&vis);

    let title = format!(
        " Workers ({}/{}) · filter: {}{} ",
        vis.len(),
        rows.len(),
        state.filter.label(),
        if state.filter_text.is_empty() {
            String::new()
        } else {
            format!(" /{}", state.filter_text)
        }
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(theme.border_focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 20 {
        return;
    }

    // columns: glyph, name, profile, run/task, state, elapsed, last line
    let (cn, cp, cr, cs, ce) = (18usize, 10usize, 14usize, 14usize, 7usize);
    let fixed = 2 + cn + cp + cr + cs + ce + 6;
    let cl = (inner.width as usize).saturating_sub(fixed);
    let header = format!(
        "  {} {} {} {} {} {}",
        fit("NAME", cn),
        fit("PROFILE", cp),
        fit("RUN/TASK", cr),
        fit("STATE", cs),
        fit("AGE", ce),
        fit("LAST OUTPUT", cl)
    );
    frame.render_widget(
        Paragraph::new(header).style(theme.text.add_modifier(Modifier::BOLD | Modifier::DIM)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let l = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 2);
    let visible_h = l.height as usize;
    if state.selected < state.scroll {
        state.scroll = state.selected;
    } else if visible_h > 0 && state.selected >= state.scroll + visible_h {
        state.scroll = state.selected + 1 - visible_h;
    }
    state.scroll = state.scroll.min(vis.len().saturating_sub(visible_h.max(1)));

    let mut lines = Vec::new();
    for (i, r) in vis.iter().enumerate().skip(state.scroll).take(visible_h) {
        let sel = i == state.selected;
        let gstyle = match r.status.as_str() {
            "running" => theme.info,
            "awaiting-input" | "awaiting_input" => theme.warning,
            "done" => theme.success,
            "failed" | "exited" => theme.error,
            _ => theme.text,
        };
        let base: Style = if sel { theme.selection } else { theme.text };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", status_glyph(&r.status)),
                if sel {
                    gstyle.patch(theme.selection)
                } else {
                    gstyle
                },
            ),
            Span::styled(
                format!(
                    " {} {} {} {} {} {}",
                    fit(&r.name, cn),
                    fit(&r.profile, cp),
                    fit(&r.run_task, cr),
                    fit(&r.state, cs),
                    fit(&r.elapsed, ce),
                    fit(&r.last_line, cl)
                ),
                base,
            ),
        ]));
    }
    if vis.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no workers match",
            theme.text.add_modifier(Modifier::DIM),
        )));
    }
    frame.render_widget(Paragraph::new(lines), l);

    // footer: prompts or key hints
    let footer = if let Some((_, name)) = &state.confirm_kill {
        Span::styled(format!(" kill {name}? y/n"), theme.warning)
    } else if let Some((_, name, text)) = &state.reply {
        Span::styled(format!(" reply to {name}: {text}_"), theme.accent)
    } else if state.is_filtering {
        Span::styled(format!(" filter: {}_", state.filter_text), theme.accent)
    } else {
        Span::styled(
            " ↑↓ move · Enter open · e peek · h hide · k kill · r reply · / filter · f cycle · Esc close",
            theme.text.add_modifier(Modifier::DIM),
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(footer)),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn row(id: u32, name: &str, status: &str) -> WorkerRow {
        WorkerRow {
            pane_id: PaneId(id),
            status: status.into(),
            name: name.into(),
            profile: "claude".into(),
            run_task: String::new(),
            state: status.into(),
            elapsed: "1m".into(),
            last_line: format!("out-{name}"),
        }
    }

    fn rows() -> Vec<WorkerRow> {
        vec![
            row(1, "idle-one", "idle"),
            row(2, "run-one", "running"),
            row(3, "ask-one", "awaiting-input"),
            row(4, "done-one", "done"),
        ]
    }

    #[test]
    fn sorting_and_filters() {
        let r = rows();
        let names: Vec<_> = visible_rows(&r, WorkersFilter::All, "")
            .iter()
            .map(|x| x.name.clone())
            .collect();
        assert_eq!(names, ["ask-one", "run-one", "idle-one", "done-one"]);
        assert_eq!(visible_rows(&r, WorkersFilter::NeedsMe, "").len(), 1);
        assert_eq!(visible_rows(&r, WorkersFilter::Running, "").len(), 1);
        assert_eq!(
            visible_rows(&r, WorkersFilter::Done, "")[0].name,
            "done-one"
        );
        assert_eq!(visible_rows(&r, WorkersFilter::All, "RUN").len(), 1);
        assert_eq!(WorkersFilter::Done.next(), WorkersFilter::All);
    }

    #[test]
    fn selection_bounds_and_follow() {
        let r = rows();
        let vis = visible_rows(&r, WorkersFilter::All, "");
        let mut st = WorkersTableState::default();
        st.move_by(-5, &vis);
        assert_eq!(st.selected, 0);
        st.move_by(99, &vis);
        assert_eq!(st.selected, 3);
        assert_eq!(st.selected_pane, Some(PaneId(4)));
    }

    #[test]
    fn renders_sorted_rows() {
        let r = rows();
        let mut st = WorkersTableState::default();
        st.open();
        let mut t = Terminal::new(TestBackend::new(140, 20)).unwrap();
        let theme = Theme::default();
        t.draw(|f| render_workers_table(f, f.area(), &r, &mut st, &theme))
            .unwrap();
        let buf = t.backend().buffer();
        let text =
            |y: u16| -> String { (0..140).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        let l = list_area(Rect::new(0, 0, 140, 20));
        assert!(text(l.y).contains("ask-one"), "{}", text(l.y));
        assert!(text(l.y).contains("out-ask-one"));
        assert!(text(l.y + 1).contains("run-one"));
        assert!(text(l.y + 3).contains("done-one"));
        assert_eq!(
            worker_row_at(Rect::new(0, 0, 140, 20), &st, 4, l.x + 3, l.y + 2),
            Some(2)
        );
        assert_eq!(
            worker_row_at(Rect::new(0, 0, 140, 20), &st, 4, l.x, l.y + 9),
            None
        );
    }
}
