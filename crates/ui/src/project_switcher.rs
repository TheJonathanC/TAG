//! Project switcher overlay (pure widget): a fuzzy-searchable tree of
//! projects -> workspaces -> panes with a live preview of the highlighted pane.

use crate::card_strip::status_glyph;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::HashSet;
use tag_core::pane::{PaneId, WorkspaceId};
use tag_core::theme::Theme;

/// A pane under a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherPane {
    pub id: PaneId,
    pub title: String,
    /// Agent status or empty for plain shells.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub panes: Vec<SwitcherPane>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherProject {
    pub id: String,
    pub name: String,
    pub root: String,
    pub active: bool,
    pub workspaces: Vec<SwitcherWorkspace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitcherTarget {
    Project(String),
    Workspace(String, WorkspaceId),
    Pane(String, WorkspaceId, PaneId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherRow {
    pub target: SwitcherTarget,
    pub depth: u8,
    pub label: String,
    pub status: String,
    pub active: bool,
    pub expandable: bool,
    pub expanded: bool,
}

#[derive(Debug, Default)]
pub struct ProjectSwitcherState {
    pub is_open: bool,
    pub query: String,
    pub selected: usize,
    pub scroll: usize,
    /// Project ids whose children are shown.
    pub expanded: HashSet<String>,
    /// Path being typed for "open folder as new project".
    pub folder_prompt: Option<String>,
}

impl ProjectSwitcherState {
    pub fn open(&mut self) {
        *self = ProjectSwitcherState {
            is_open: true,
            ..Default::default()
        };
    }

    pub fn move_by(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, count as isize - 1) as usize;
    }

    pub fn clamp(&mut self, count: usize) {
        self.selected = self.selected.min(count.saturating_sub(1));
    }
}

/// Flattened visible rows: projects (filtered by `query`, fuzzy-ranked when non-empty,
/// else in the given order) with expanded children following.
pub fn switcher_rows(
    projects: &[SwitcherProject],
    state: &ProjectSwitcherState,
) -> Vec<SwitcherRow> {
    let matcher = SkimMatcherV2::default();
    let mut order: Vec<(i64, &SwitcherProject)> = projects
        .iter()
        .filter_map(|p| {
            if state.query.is_empty() {
                Some((0, p))
            } else {
                matcher
                    .fuzzy_match(&p.name, &state.query)
                    .or_else(|| matcher.fuzzy_match(&p.root, &state.query))
                    .map(|s| (s, p))
            }
        })
        .collect();
    if !state.query.is_empty() {
        order.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    }
    let mut rows = Vec::new();
    for (_, p) in order {
        let expanded = state.expanded.contains(&p.id);
        rows.push(SwitcherRow {
            target: SwitcherTarget::Project(p.id.clone()),
            depth: 0,
            label: p.name.clone(),
            status: String::new(),
            active: p.active,
            expandable: !p.workspaces.is_empty(),
            expanded,
        });
        if !expanded {
            continue;
        }
        for w in &p.workspaces {
            rows.push(SwitcherRow {
                target: SwitcherTarget::Workspace(p.id.clone(), w.id),
                depth: 1,
                label: w.name.clone(),
                status: String::new(),
                active: false,
                expandable: false,
                expanded: false,
            });
            for pane in &w.panes {
                rows.push(SwitcherRow {
                    target: SwitcherTarget::Pane(p.id.clone(), w.id, pane.id),
                    depth: 2,
                    label: pane.title.clone(),
                    status: pane.status.clone(),
                    active: false,
                    expandable: false,
                    expanded: false,
                });
            }
        }
    }
    rows
}

/// The overlay rectangle (~90% x 80%, centered).
pub fn switcher_area(screen: Rect) -> Rect {
    let w = (screen.width * 9 / 10).max(40).min(screen.width);
    let h = (screen.height * 4 / 5).max(8).min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

fn split(screen: Rect) -> (Rect, Rect, Rect) {
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(switcher_area(screen));
    // top line: query; bottom line: footer
    let body = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let left_w = (body.width * 2 / 5).max(1).min(body.width);
    let left = Rect::new(body.x, body.y, left_w, body.height);
    let right = Rect::new(
        body.x + left_w + 1,
        body.y,
        body.width.saturating_sub(left_w + 1),
        body.height,
    );
    (inner, left, right)
}

/// Area (inside the overlay) where the preview is drawn (title row included); the app
/// sizes `read_text` to its height minus one.
pub fn preview_area(screen: Rect) -> Rect {
    split(screen).2
}

fn fit(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    let mut o: String = s.chars().take(w.saturating_sub(1)).collect();
    o.push('…');
    o
}

/// Render the tree on the left and `preview` (lines of the highlighted pane) on the right.
pub fn render_project_switcher(
    frame: &mut Frame,
    screen: Rect,
    projects: &[SwitcherProject],
    state: &mut ProjectSwitcherState,
    preview: &[String],
    preview_title: &str,
    theme: &Theme,
) {
    let area = switcher_area(screen);
    frame.render_widget(Clear, area);
    let rows = switcher_rows(projects, state);
    state.clamp(rows.len());
    let block = Block::default()
        .title(format!(" Projects ({}) ", projects.len()))
        .borders(Borders::ALL)
        .border_style(theme.border_focused);
    frame.render_widget(block, area);
    let (inner, left, right) = split(screen);
    if inner.height < 4 || inner.width < 20 {
        return;
    }
    frame.render_widget(
        Paragraph::new(format!("> {}", state.query)).style(theme.accent),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let h = left.height as usize;
    if state.selected < state.scroll {
        state.scroll = state.selected;
    } else if h > 0 && state.selected >= state.scroll + h {
        state.scroll = state.selected + 1 - h;
    }
    let mut lines = Vec::new();
    for (i, r) in rows.iter().enumerate().skip(state.scroll).take(h) {
        let sel = i == state.selected;
        let base = if sel { theme.selection } else { theme.text };
        let marker = match (r.expandable, r.expanded) {
            (true, true) => "▾ ",
            (true, false) => "▸ ",
            _ => "  ",
        };
        let indent = "  ".repeat(r.depth as usize);
        let glyph = if r.status.is_empty() {
            String::new()
        } else {
            format!("{} ", status_glyph(&r.status))
        };
        let star = if r.active { " *" } else { "" };
        let text = fit(
            &format!("{indent}{marker}{glyph}{}{star}", r.label),
            left.width as usize,
        );
        let pad = (left.width as usize).saturating_sub(text.chars().count());
        lines.push(Line::from(vec![
            Span::styled(text, base),
            Span::styled(" ".repeat(pad), base),
        ]));
    }
    if rows.is_empty() {
        lines.push(Line::styled("  no matching projects", theme.text));
    }
    frame.render_widget(Paragraph::new(lines), left);
    if right.width > 2 && right.height > 1 {
        frame.render_widget(
            Paragraph::new(fit(preview_title, right.width as usize))
                .style(theme.text.add_modifier(Modifier::BOLD)),
            Rect::new(right.x, right.y, right.width, 1),
        );
        let body = Rect::new(right.x, right.y + 1, right.width, right.height - 1);
        let text: Vec<Line> = preview
            .iter()
            .map(|l| Line::from(fit(l, body.width as usize)))
            .collect();
        frame.render_widget(Paragraph::new(text).style(theme.text), body);
    }
    frame.render_widget(
        Paragraph::new("Enter switch · →/Tab expand · ← collapse · o open folder · Esc close")
            .style(theme.text.add_modifier(Modifier::DIM)),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn sample() -> Vec<SwitcherProject> {
        let pane = |id, t: &str, s: &str| SwitcherPane {
            id: PaneId(id),
            title: t.into(),
            status: s.into(),
        };
        vec![
            SwitcherProject {
                id: "a".into(),
                name: "alpha".into(),
                root: "/src/alpha".into(),
                active: true,
                workspaces: vec![SwitcherWorkspace {
                    id: WorkspaceId(1),
                    name: "main".into(),
                    panes: vec![pane(1, "claude", "running")],
                }],
            },
            SwitcherProject {
                id: "b".into(),
                name: "beta".into(),
                root: "/src/beta".into(),
                active: false,
                workspaces: vec![],
            },
        ]
    }

    #[test]
    fn rows_filter_and_expand() {
        let p = sample();
        let mut st = ProjectSwitcherState::default();
        assert_eq!(switcher_rows(&p, &st).len(), 2);
        st.expanded.insert("a".into());
        assert_eq!(switcher_rows(&p, &st).len(), 4);
        st.query = "bet".into();
        let r = switcher_rows(&p, &st);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].target, SwitcherTarget::Project("b".into()));
    }

    #[test]
    fn renders_tree_and_preview() {
        let p = sample();
        let mut st = ProjectSwitcherState::default();
        st.open();
        st.expanded.insert("a".into());
        let mut t = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let theme = Theme::default();
        let prev = vec!["hello from pane".to_string()];
        t.draw(|f| {
            render_project_switcher(f, f.area(), &p, &mut st, &prev, "claude", &theme);
        })
        .unwrap();
        let buf = t.backend().buffer();
        let all: String = (0..24)
            .flat_map(|y| (0..120).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        for needle in ["alpha", "beta", "main", "claude", "hello from pane"] {
            assert!(all.contains(needle), "missing {needle}");
        }
    }
}
