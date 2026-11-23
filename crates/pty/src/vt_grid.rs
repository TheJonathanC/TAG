use ratatui::style::{Color as RatColor, Modifier, Style};
use ratatui::text::{Line as RatLine, Span};
use std::sync::Mutex;
use vt100::{Color as VtColor, Screen};

use crate::ring_buffer::ScrollbackRing;
use crate::selection::Selection;

/// Placeholder for the second column of a double-width character in `get_row_text`.
pub const WIDE_CONTINUATION: char = '\0';

type ViewportCache = Option<(u64, usize, Option<Selection>, Vec<RatLine<'static>>)>;

pub struct VtGrid {
    parser: vt100::Parser,
    cols: u16,
    rows: u16,
    last_bell: bool,
    generation: u64,
    cached_viewport: Mutex<ViewportCache>,
}

impl VtGrid {
    pub fn new(cols: u16, rows: u16, scrollback_limit: usize) -> Self {
        let parser = vt100::Parser::new(rows, cols, scrollback_limit);
        Self {
            parser,
            cols,
            rows,
            last_bell: false,
            generation: 0,
            cached_viewport: Mutex::new(None),
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        if self.cols == cols && self.rows == rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.parser.set_size(rows, cols);
        self.generation = self.generation.wrapping_add(1);
        if let Ok(mut cache) = self.cached_viewport.lock() {
            *cache = None;
        }
    }

    pub fn process(&mut self, data: &[u8], _ring: &mut ScrollbackRing) {
        if data.is_empty() {
            return;
        }
        // Feed into vt100 parser
        self.parser.process(data);
        self.generation = self.generation.wrapping_add(1);

        // Check for BELL in stream
        if data.contains(&0x07) {
            self.last_bell = true;
        }
    }

    pub fn screen(&self) -> &Screen {
        self.parser.screen()
    }

    pub fn is_alternate(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn mouse_protocol_mode(&self) -> vt100::MouseProtocolMode {
        self.parser.screen().mouse_protocol_mode()
    }

    pub fn mouse_protocol_encoding(&self) -> vt100::MouseProtocolEncoding {
        self.parser.screen().mouse_protocol_encoding()
    }

    pub fn set_scrollback(&mut self, offset: usize) -> usize {
        self.parser.set_scrollback(offset);
        self.generation = self.generation.wrapping_add(1);
        self.parser.screen().scrollback()
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        let (row, col) = self.parser.screen().cursor_position();
        (col, row)
    }

    pub fn has_bell(&mut self) -> bool {
        let b = self.last_bell;
        self.last_bell = false;
        b
    }

    pub fn render_viewport(
        &self,
        scroll_offset: usize,
        _ring: &ScrollbackRing,
        selection: Option<&Selection>,
    ) -> Vec<RatLine<'static>> {
        if let Ok(cache) = self.cached_viewport.lock() {
            if let Some((gen, cached_scroll, ref cached_sel, ref lines)) = *cache {
                if gen == self.generation
                    && cached_scroll == scroll_offset
                    && cached_sel.as_ref() == selection
                {
                    return lines.clone();
                }
            }
        }

        let screen = self.parser.screen();
        let (_num_rows, cols) = screen.size();
        let visible_rows = self.rows as usize;

        let mut lines = Vec::with_capacity(visible_rows);
        for row in 0..visible_rows {
            let mut spans = Vec::new();
            let mut current_style: Option<Style> = None;
            let mut current_text = String::with_capacity(cols as usize);

            for col in 0..cols {
                if let Some(cell) = screen.cell(row as u16, col) {
                    // Wide character continuation cells are visually part of the previous cell;
                    // outputting extra spaces here pushes and corrupts line layout.
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let mut style = convert_vt_style(cell);
                    if let Some(sel) = selection {
                        if sel.contains(col, row as u16) {
                            style = style.add_modifier(Modifier::REVERSED);
                        }
                    }
                    let s = cell.contents();
                    let ch = if !s.is_empty() { s.as_str() } else { " " };

                    if current_style == Some(style) {
                        current_text.push_str(ch);
                    } else {
                        if !current_text.is_empty() {
                            if let Some(st) = current_style {
                                spans.push(Span::styled(std::mem::take(&mut current_text), st));
                            }
                        }
                        current_style = Some(style);
                        current_text.push_str(ch);
                    }
                } else {
                    let style = Style::default();
                    if current_style == Some(style) {
                        current_text.push(' ');
                    } else {
                        if !current_text.is_empty() {
                            if let Some(st) = current_style {
                                spans.push(Span::styled(std::mem::take(&mut current_text), st));
                            }
                        }
                        current_style = Some(style);
                        current_text.push(' ');
                    }
                }
            }
            if !current_text.is_empty() {
                if let Some(st) = current_style {
                    spans.push(Span::styled(current_text, st));
                }
            }
            lines.push(RatLine::from(spans));
        }

        while lines.len() < visible_rows {
            lines.push(RatLine::raw(""));
        }

        if let Ok(mut cache) = self.cached_viewport.lock() {
            *cache = Some((
                self.generation,
                scroll_offset,
                selection.cloned(),
                lines.clone(),
            ));
        }

        lines
    }

    /// The last `lines` lines of scrollback + screen as plain text (no ANSI; carriage-return
    /// progress lines are already collapsed by the emulator). Trailing blank screen rows are
    /// dropped. The user's scroll position is preserved.
    ///
    /// vt100 0.15 overflows when the scrollback offset exceeds the screen height, so at most
    /// one screen-height of scrollback is returned on top of the screen itself.
    pub fn text_tail(&mut self, lines: usize) -> String {
        let saved = self.parser.screen().scrollback();
        let (rows, cols) = self.parser.screen().size();
        self.parser.set_scrollback(0);
        let mut screen_lines: Vec<String> = self.parser.screen().rows(0, cols).collect();
        while screen_lines.last().is_some_and(|l| l.trim().is_empty()) {
            screen_lines.pop();
        }
        let mut out = Vec::new();
        let want_extra = lines.saturating_sub(screen_lines.len());
        if want_extra > 0 {
            self.parser.set_scrollback(rows as usize);
            let available = self.parser.screen().scrollback();
            // With scrollback offset `o`, screen row 0 shows the line `o` lines above the top.
            for o in (1..=want_extra.min(available)).rev() {
                self.parser.set_scrollback(o);
                if let Some(line) = self.parser.screen().rows(0, cols).next() {
                    out.push(line);
                }
            }
        }
        out.extend(screen_lines);
        self.parser.set_scrollback(saved);
        let skip = out.len().saturating_sub(lines);
        out.into_iter()
            .skip(skip)
            .map(|l| l.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn get_row_text(&self, row: u16) -> String {
        let screen = self.parser.screen();
        let (_num_rows, cols) = screen.size();
        let mut row_text = String::with_capacity(cols as usize);
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                if cell.is_wide_continuation() {
                    // Keep one char per column for selection math; stripped when copying.
                    row_text.push(WIDE_CONTINUATION);
                    continue;
                }
                let s = cell.contents();
                if s.is_empty() {
                    row_text.push(' ');
                } else {
                    row_text.push_str(&s);
                }
            }
        }
        row_text.trim_end().to_string()
    }
}

fn convert_vt_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    if let Some(fg) = convert_color(cell.fgcolor()) {
        style = style.fg(fg);
    }
    if let Some(bg) = convert_color(cell.bgcolor()) {
        style = style.bg(bg);
    }

    style
}

fn convert_color(color: VtColor) -> Option<RatColor> {
    match color {
        VtColor::Default => None,
        VtColor::Idx(i) => Some(RatColor::Indexed(i)),
        VtColor::Rgb(r, g, b) => Some(RatColor::Rgb(r, g, b)),
    }
}

#[cfg(test)]
mod text_tail_tests {
    use super::*;

    #[test]
    fn text_tail_includes_scrollback_and_preserves_scroll() {
        let mut g = VtGrid::new(20, 3, 100);
        let mut ring = ScrollbackRing::new(100);
        for i in 0..10 {
            g.process(format!("line {i}\r\n").as_bytes(), &mut ring);
        }
        g.process(b"\x1b[31mdl\x1b[0m  50%\rdl 100%", &mut ring);
        g.set_scrollback(2);
        let t = g.text_tail(4);
        assert_eq!(t, "line 7\nline 8\nline 9\ndl 100%");
        assert_eq!(g.scrollback(), 2);
        // Screen (3 rows, last one blank-trimmed) + at most 3 rows of scrollback.
        assert!(
            g.text_tail(100).starts_with("line 5\n"),
            "{}",
            g.text_tail(100)
        );
    }
}
