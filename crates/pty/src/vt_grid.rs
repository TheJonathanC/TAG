use ratatui::style::{Color as RatColor, Modifier, Style};
use ratatui::text::{Line as RatLine, Span};
use vt100::{Color as VtColor, Screen};

use crate::ring_buffer::ScrollbackRing;

pub struct VtGrid {
    parser: vt100::Parser,
    cols: u16,
    rows: u16,
    last_bell: bool,
}

impl VtGrid {
    pub fn new(cols: u16, rows: u16, scrollback_limit: usize) -> Self {
        let parser = vt100::Parser::new(rows, cols, scrollback_limit);
        Self {
            parser,
            cols,
            rows,
            last_bell: false,
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.parser.set_size(rows, cols);
    }

    pub fn process(&mut self, data: &[u8], _ring: &mut ScrollbackRing) {
        // Feed into vt100 parser
        self.parser.process(data);

        // Check for BELL in stream
        if data.contains(&0x07) {
            self.last_bell = true;
        }

        // Keep scrollback ring updated with any lines that scrolled off in standard screen
        // vt100 retains history in its internal parser, but our independent ring buffer
        // holds formatted lines up to the configurable limit (e.g. 50k lines).
    }

    pub fn screen(&self) -> &Screen {
        self.parser.screen()
    }

    pub fn is_alternate(&self) -> bool {
        // vt100 does not have a direct screen.alternate() boolean in all versions,
        // but screen.hide_cursor() or parser inspection can be used.
        false
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
        ring: &ScrollbackRing,
    ) -> Vec<RatLine<'static>> {
        let screen = self.parser.screen();
        let (_num_rows, cols) = screen.size();
        let visible_rows = self.rows as usize;

        if scroll_offset == 0 {
            // Render directly from current active screen
            let mut lines = Vec::with_capacity(visible_rows);
            for row in 0..visible_rows {
                let mut spans = Vec::new();
                for col in 0..cols {
                    if let Some(cell) = screen.cell(row as u16, col) {
                        let style = convert_vt_style(cell);
                        let s = cell.contents();
                        if !s.is_empty() {
                            spans.push(Span::styled(s.to_string(), style));
                        } else {
                            spans.push(Span::styled(" ", style));
                        }
                    }
                }
                lines.push(RatLine::from(spans));
            }
            lines
        } else {
            // Render from scrollback history
            let ring_len = ring.len();
            let mut lines = Vec::with_capacity(visible_rows);

            let end_idx = ring_len.saturating_sub(scroll_offset);
            let start_idx = end_idx.saturating_sub(visible_rows);

            for i in start_idx..end_idx {
                if let Some(line) = ring.get(i) {
                    lines.push(RatLine::raw(line.text.clone()));
                } else {
                    lines.push(RatLine::raw(""));
                }
            }

            // Fill remainder with empty lines if any
            while lines.len() < visible_rows {
                lines.push(RatLine::raw(""));
            }

            lines
        }
    }

    pub fn get_row_text(&self, row: u16) -> String {
        let screen = self.parser.screen();
        let (_num_rows, cols) = screen.size();
        let mut row_text = String::new();
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                row_text.push_str(&cell.contents());
            }
        }
        row_text
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
