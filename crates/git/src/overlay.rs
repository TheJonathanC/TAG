use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use crate::backend::{RepoStatus, WorktreeInfo};
use crate::diff::format_diff_lines;

#[derive(Default)]
pub struct GitOverlayState {
    pub is_open: bool,
    pub selected_section: usize, // 0: worktrees, 1: files, 2: diff
    pub selected_index: usize,
    pub diff_scroll: usize,
}

pub fn render_git_overlay(
    frame: &mut Frame,
    total_area: Rect,
    worktrees: &[WorktreeInfo],
    status: &RepoStatus,
    diff_text: &str,
    state: &GitOverlayState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let width = (total_area.width * 8 / 10)
        .clamp(60, 130)
        .min(total_area.width);
    let height = (total_area.height * 8 / 10)
        .clamp(18, 40)
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

    let block = Block::default()
        .title(" Git Worktrees & Diff (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[0]);

    // 1. Worktrees list
    let wt_block = Block::default()
        .title(" Worktrees ")
        .borders(Borders::ALL)
        .border_style(if state.selected_section == 0 {
            theme.accent
        } else {
            theme.border_unfocused
        });

    let mut wt_items = Vec::new();
    for (i, wt) in worktrees.iter().enumerate() {
        let is_sel = state.selected_section == 0 && i == state.selected_index;
        let prefix = if is_sel { "> " } else { "  " };
        let branch_name = wt.branch.as_deref().unwrap_or("detached");
        let main_badge = if wt.is_main { " [main]" } else { "" };

        let span = Span::styled(
            format!("{}{}: git: {}{}", prefix, wt.name, branch_name, main_badge),
            if is_sel {
                theme.title_focused
            } else {
                theme.text
            },
        );
        wt_items.push(ListItem::new(Line::from(vec![span])));
    }
    frame.render_widget(List::new(wt_items).block(wt_block), left_chunks[0]);

    // 2. Changed files list
    let files_block = Block::default()
        .title(format!(" Changes ({}) ", status.changed_files.len()))
        .borders(Borders::ALL)
        .border_style(if state.selected_section == 1 {
            theme.accent
        } else {
            theme.border_unfocused
        });

    let mut file_items = Vec::new();
    for (i, f) in status.changed_files.iter().enumerate() {
        let is_sel = state.selected_section == 1 && i == state.selected_index;
        let prefix = if is_sel { "> " } else { "  " };
        let staged_badge = if f.is_staged { "[S] " } else { "[U] " };

        let stat_color = match f.status {
            "modified" => Color::Yellow,
            "added" | "untracked" => Color::Green,
            "deleted" => Color::Red,
            _ => Color::White,
        };

        file_items.push(ListItem::new(Line::from(vec![
            Span::raw(prefix),
            Span::styled(staged_badge, Style::default().fg(stat_color)),
            Span::styled(
                format!("{:<10} ", f.status),
                Style::default().fg(stat_color),
            ),
            Span::styled(&f.path, theme.text),
        ])));
    }
    frame.render_widget(List::new(file_items).block(files_block), left_chunks[1]);

    // 3. Diff viewer
    let diff_block = Block::default()
        .title(" Diff View ")
        .borders(Borders::ALL)
        .border_style(if state.selected_section == 2 {
            theme.accent
        } else {
            theme.border_unfocused
        });

    let diff_inner = diff_block.inner(chunks[1]);
    frame.render_widget(diff_block, chunks[1]);

    let formatted_lines = format_diff_lines(diff_text);
    let scrolled_lines: Vec<Line<'static>> = formatted_lines
        .into_iter()
        .skip(state.diff_scroll)
        .take(diff_inner.height as usize)
        .collect();

    let p = Paragraph::new(scrolled_lines);
    frame.render_widget(p, diff_inner);
}
