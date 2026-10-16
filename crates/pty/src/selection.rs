#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Normal,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coord {
    pub col: u16,
    pub row: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub start: Coord,
    pub end: Coord,
    pub mode: SelectionMode,
    pub is_active: bool,
}

impl Selection {
    pub fn new(start: Coord, mode: SelectionMode) -> Self {
        Self {
            start,
            end: start,
            mode,
            is_active: true,
        }
    }

    pub fn update(&mut self, current: Coord) {
        self.end = current;
    }

    pub fn normalized_bounds(&self) -> (Coord, Coord) {
        if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    pub fn contains(&self, col: u16, row: u16) -> bool {
        if !self.is_active {
            return false;
        }
        let (min, max) = self.normalized_bounds();
        if row < min.row || row > max.row {
            return false;
        }
        if self.mode == SelectionMode::Line {
            return true;
        }
        if min.row == max.row {
            return col >= min.col && col <= max.col;
        }
        if row == min.row {
            col >= min.col
        } else if row == max.row {
            col <= max.col
        } else {
            true
        }
    }

    pub fn extract_text(&self, lines: &[String]) -> String {
        if !self.is_active || lines.is_empty() {
            return String::new();
        }
        let (min, max) = self.normalized_bounds();
        let mut result = Vec::new();

        for row in min.row..=max.row {
            if let Some(line) = lines.get(row as usize) {
                let chars: Vec<char> = line.chars().collect();
                if self.mode == SelectionMode::Line {
                    result.push(line.clone());
                    continue;
                }

                let start_col = if row == min.row { min.col as usize } else { 0 };
                let end_col = if row == max.row {
                    (max.col as usize).min(chars.len())
                } else {
                    chars.len()
                };

                if start_col < chars.len() {
                    let slice_end = (end_col + 1).min(chars.len());
                    let selected_chars: String = chars[start_col..slice_end].iter().collect();
                    result.push(selected_chars);
                } else {
                    result.push(String::new());
                }
            }
        }

        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_single_line() {
        let mut sel = Selection::new(Coord { col: 2, row: 0 }, SelectionMode::Normal);
        sel.update(Coord { col: 5, row: 0 });
        let lines = vec!["hello world".to_string()];
        assert_eq!(sel.extract_text(&lines), "llo ");
    }

    #[test]
    fn test_selection_multiline() {
        let mut sel = Selection::new(Coord { col: 3, row: 0 }, SelectionMode::Normal);
        sel.update(Coord { col: 2, row: 1 });
        let lines = vec!["abcde".to_string(), "12345".to_string()];
        let text = sel.extract_text(&lines);
        assert_eq!(text, "de\n123");
    }
}
