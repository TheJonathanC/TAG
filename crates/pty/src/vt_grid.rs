use ratatui::style::{Color as RatColor, Modifier, Style};
use ratatui::text::{Line as RatLine, Span};
use std::sync::Mutex;
use vt100::{Color as VtColor, Screen};

use crate::ring_buffer::ScrollbackRing;
use crate::selection::Selection;

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
                }
            }
            if !current_text.is_empty() {
                if let Some(st) = current_style {
                    spans.push(Span::styled(current_text, st));
                }
            }
            lines.push(RatLine::from(spans));
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

    pub fn get_row_text(&self, row: u16) -> String {
        let screen = self.parser.screen();
        let (_num_rows, cols) = screen.size();
        let mut row_text = String::with_capacity(cols as usize);
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
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
