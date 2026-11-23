//! Brain browser overlay (`Leader+B`): live search box, results list and a
//! preview with outgoing links and backlinks. Pure widget: the host fills
//! the data-only structs; this crate does not depend on `tag-brain`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use tag_core::theme::Theme;

/// One search result row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrainResult {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub project: Option<String>,
    pub snippet: String,
}

/// Preview of one document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrainPreview {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub project: Option<String>,
    pub body: String,
    /// Outgoing link targets.
    pub links: Vec<String>,
    /// Ids of documents linking here.
    pub backlinks: Vec<String>,
}

/// Which list has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrainFocus {
    #[default]
    Results,
    Links,
}

/// State of the brain overlay.
#[derive(Debug, Default)]
pub struct BrainOverlayState {
    pub is_open: bool,
    pub query: String,
    pub results: Vec<BrainResult>,
    pub selected: usize,
    pub preview: Option<BrainPreview>,
    pub focus: BrainFocus,
    /// Selected row in the combined links + backlinks list.
    pub link_selected: usize,
    /// One-line status (errors, "no such document", ...).
    pub status: Option<String>,
}

impl BrainOverlayState {
    /// Reset to a fresh open overlay.
    pub fn open(&mut self) {
        *self = BrainOverlayState {
            is_open: true,
            ..Default::default()
        };
    }

    /// Close the overlay (keeps nothing).
    pub fn close(&mut self) {
        *self = BrainOverlayState::default();
    }

    /// The highlighted search result.
    pub fn selected_result(&self) -> Option<&BrainResult> {
        self.results.get(self.selected)
    }

    /// Links followed by backlinks of the preview: the navigable targets.
    pub fn nav_targets(&self) -> Vec<String> {
        match &self.preview {
            Some(p) => p.links.iter().chain(p.backlinks.iter()).cloned().collect(),
            None => Vec::new(),
        }
    }

    /// The link row under the cursor.
    pub fn selected_link(&self) -> Option<String> {
        self.nav_targets().into_iter().nth(self.link_selected)
    }

    /// Move the selection in the focused list by `delta`, clamped.
    /// Returns true if the results selection changed (preview must reload).
    pub fn move_selection(&mut self, delta: isize) -> bool {
        match self.focus {
            BrainFocus::Results => {
                let n = self.results.len();
                let old = self.selected;
                self.selected = step(self.selected, delta, n);
                self.selected != old
            }
            BrainFocus::Links => {
                let n = self.nav_targets().len();
                self.link_selected = step(self.link_selected, delta, n);
                false
            }
        }
    }

    /// Toggle focus between results and links.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            BrainFocus::Results => BrainFocus::Links,
            BrainFocus::Links => BrainFocus::Results,
        };
    }

    /// Replace results (keeps the selection in range).
    pub fn set_results(&mut self, results: Vec<BrainResult>) {
        self.results = results;
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
    }

    /// Replace the preview and reset the link cursor.
    pub fn set_preview(&mut self, preview: Option<BrainPreview>) {
        self.preview = preview;
        self.link_selected = 0;
    }
}

fn step(cur: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as isize + delta).clamp(0, len as isize - 1) as usize
}

fn centered(area: Rect) -> Rect {
    let w = (area.width * 90 / 100).max(40).min(area.width);
    let h = (area.height * 85 / 100).max(10).min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Render the overlay centered in `area`.
pub fn render_brain_overlay(
    frame: &mut Frame,
    area: Rect,
    state: &BrainOverlayState,
    theme: &Theme,
) {
    if !state.is_open || area.width < 20 || area.height < 6 {
        return;
    }
    let rect = centered(area);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Brain (Ctrl+Space B) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Search: ", theme.accent),
            Span::styled(format!("{}_", state.query), theme.text),
        ])),
        rows[0],
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[1]);

    // Results list.
    let items: Vec<ListItem> = if state.results.is_empty() {
        vec![ListItem::new(Span::styled(
            if state.query.is_empty() {
                "(type to search the brain)"
            } else {
                "(no results)"
            },
            theme.text,
        ))]
    } else {
        state
            .results
            .iter()
            .map(|r| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("[{}] ", r.kind), theme.info),
                    Span::styled(r.title.clone(), theme.text),
                ]))
            })
            .collect()
    };
    let focus_results = state.focus == BrainFocus::Results;
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(theme.border_unfocused),
        )
        .highlight_style(if focus_results {
            theme.selection
        } else {
            theme.selection.add_modifier(Modifier::DIM)
        })
        .highlight_symbol("> ");
    let mut ls = ListState::default();
    if !state.results.is_empty() {
        ls.select(Some(state.selected));
    }
    frame.render_stateful_widget(list, cols[0], &mut ls);

    // Preview.
    match &state.preview {
        None => frame.render_widget(
            Paragraph::new(Span::styled("(no document selected)", theme.text)),
            cols[1],
        ),
        Some(p) => {
            let right = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(8)])
                .split(cols[1]);
            let head = format!(
                "{}  [{} | {} | {}]",
                p.title,
                p.kind,
                p.project.as_deref().unwrap_or("global"),
                p.id
            );
            let mut lines = vec![
                Line::from(Span::styled(
                    head,
                    theme.title_focused.add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            lines.extend(p.body.lines().map(|l| Line::from(Span::raw(l.to_string()))));
            frame.render_widget(
                Paragraph::new(lines)
                    .style(theme.text)
                    .wrap(Wrap { trim: false }),
                right[0],
            );

            let mut nav: Vec<ListItem> = Vec::new();
            for l in &p.links {
                nav.push(ListItem::new(Line::from(vec![
                    Span::styled("-> ", theme.accent),
                    Span::styled(l.clone(), theme.text),
                ])));
            }
            for b in &p.backlinks {
                nav.push(ListItem::new(Line::from(vec![
                    Span::styled("<- ", theme.info),
                    Span::styled(b.clone(), theme.text),
                ])));
            }
            if nav.is_empty() {
                nav.push(ListItem::new(Span::styled("(no links)", theme.text)));
            }
            let nav_list = List::new(nav)
                .block(
                    Block::default()
                        .title(" Links / Backlinks (Tab, Enter) ")
                        .borders(Borders::TOP)
                        .border_style(theme.border_unfocused),
                )
                .highlight_style(if state.focus == BrainFocus::Links {
                    theme.selection
                } else {
                    theme.text
                })
                .highlight_symbol("> ");
            let mut ns = ListState::default();
            if state.focus == BrainFocus::Links && !state.nav_targets().is_empty() {
                ns.select(Some(state.link_selected));
            }
            frame.render_stateful_widget(nav_list, right[1], &mut ns);
        }
    }

    let hint = state.status.clone().unwrap_or_else(|| {
        "type: search  Up/Down: select  Tab: links  Enter: follow link  Esc: close".into()
    });
    frame.render_widget(Paragraph::new(Span::styled(hint, theme.info)), rows[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn dump(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    fn sample() -> BrainOverlayState {
        let mut st = BrainOverlayState::default();
        st.open();
        st.query = "jwt".into();
        st.set_results(vec![
            BrainResult {
                id: "pat_jwt".into(),
                kind: "pattern".into(),
                title: "JWT middleware".into(),
                project: Some("p1".into()),
                snippet: "...".into(),
            },
            BrainResult {
                id: "dec_x".into(),
                kind: "decision".into(),
                title: "Session storage".into(),
                project: None,
                snippet: "...".into(),
            },
        ]);
        st.set_preview(Some(BrainPreview {
            id: "pat_jwt".into(),
            kind: "pattern".into(),
            title: "JWT middleware".into(),
            project: Some("p1".into()),
            body: "Validates tokens.\nSecond line.".into(),
            links: vec!["pat_refresh".into()],
            backlinks: vec!["dec_x".into()],
        }));
        st
    }

    #[test]
    fn renders_search_results_preview_and_links() {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let st = sample();
        term.draw(|f| render_brain_overlay(f, f.area(), &st, &Theme::default()))
            .unwrap();
        let out = dump(&term);
        assert!(out.contains("Search: jwt_"));
        assert!(out.contains("[pattern] JWT middleware"));
        assert!(out.contains("[decision] Session storage"));
        assert!(out.contains("Validates tokens."));
        assert!(out.contains("-> pat_refresh"));
        assert!(out.contains("<- dec_x"));
    }

    #[test]
    fn closed_overlay_draws_nothing() {
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let st = BrainOverlayState::default();
        term.draw(|f| render_brain_overlay(f, f.area(), &st, &Theme::default()))
            .unwrap();
        assert!(dump(&term).trim().is_empty());
    }

    #[test]
    fn empty_results_message_and_tiny_area_ok() {
        let mut st = BrainOverlayState::default();
        st.open();
        st.query = "zzz".into();
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| render_brain_overlay(f, f.area(), &st, &Theme::default()))
            .unwrap();
        assert!(dump(&term).contains("(no results)"));
        let mut tiny = Terminal::new(TestBackend::new(10, 3)).unwrap();
        tiny.draw(|f| render_brain_overlay(f, f.area(), &st, &Theme::default()))
            .unwrap();
    }

    #[test]
    fn selection_and_links_navigation() {
        let mut st = sample();
        assert!(st.move_selection(1));
        assert!(!st.move_selection(5), "clamped at last");
        assert_eq!(st.selected, 1);
        st.toggle_focus();
        st.move_selection(1);
        assert_eq!(st.selected_link().as_deref(), Some("dec_x"));
        st.move_selection(10);
        assert_eq!(st.link_selected, 1);
    }
}
