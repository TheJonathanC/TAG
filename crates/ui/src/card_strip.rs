//! One-row strip of "card" workers (minimized panes) above the status bar.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tag_core::pane::PaneId;
use tag_core::theme::Theme;

/// Data for one card; built by the app from pane state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardItem {
    pub pane_id: PaneId,
    /// Agent status: `idle|running|awaiting-input|done|exited|failed` (or empty).
    pub status: String,
    /// Task id when bound to a run, else a short title.
    pub label: String,
    pub profile: Option<String>,
    /// Pre-formatted time since the last status change (e.g. `4m`).
    pub elapsed: String,
}

/// Status glyph shared by cards and the workers table.
pub fn status_glyph(status: &str) -> char {
    match status {
        "running" => '●',
        "awaiting-input" | "awaiting_input" => '?',
        "done" => '✓',
        "failed" | "exited" => '✗',
        _ => '○',
    }
}

/// Format seconds as `12s`, `4m`, `1h05m`.
pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// The bracketed text of a card, e.g. `[● t1 agy 4m]`.
pub fn card_text(item: &CardItem) -> String {
    let g = status_glyph(&item.status);
    let body = match item.status.as_str() {
        "awaiting-input" | "awaiting_input" => format!("{g} {} needs input", item.label),
        "done" | "failed" | "exited" => format!("{g} {}", item.label),
        _ => {
            let mut s = format!("{g} {}", item.label);
            if let Some(p) = item.profile.as_deref().filter(|p| !p.is_empty()) {
                s.push(' ');
                s.push_str(p);
            }
            if !item.elapsed.is_empty() {
                s.push(' ');
                s.push_str(&item.elapsed);
            }
            s
        }
    };
    format!("[{body}]")
}

/// `(start_col, width)` of every card that fits in `width` columns (cards separated by a space).
fn layout(items: &[CardItem], width: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let mut x: u16 = 0;
    for item in items {
        let w = card_text(item).chars().count() as u16;
        if x >= width {
            break;
        }
        out.push((x, w.min(width - x)));
        x = x.saturating_add(w).saturating_add(1);
    }
    out
}

/// The card at screen position (`col`, `row`) for a strip drawn in `area`.
pub fn card_at(area: Rect, items: &[CardItem], col: u16, row: u16) -> Option<PaneId> {
    if row < area.y || row >= area.y + area.height || col < area.x {
        return None;
    }
    let rel = col - area.x;
    layout(items, area.width)
        .iter()
        .zip(items)
        .find(|((x, w), _)| rel >= *x && rel < x + w)
        .map(|(_, it)| it.pane_id)
}

pub fn render_card_strip(frame: &mut Frame, area: Rect, items: &[CardItem], theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let mut spans = Vec::new();
    for ((_, w), item) in layout(items, area.width).iter().zip(items) {
        let style = match item.status.as_str() {
            "running" => theme.info,
            "awaiting-input" | "awaiting_input" => theme.warning,
            "done" => theme.success,
            "failed" | "exited" => theme.error,
            _ => theme.text,
        };
        let text: String = card_text(item).chars().take(*w as usize).collect();
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn items() -> Vec<CardItem> {
        vec![
            CardItem {
                pane_id: PaneId(1),
                status: "running".into(),
                label: "t1".into(),
                profile: Some("agy".into()),
                elapsed: "4m".into(),
            },
            CardItem {
                pane_id: PaneId(2),
                status: "awaiting-input".into(),
                label: "t2".into(),
                profile: None,
                elapsed: "1m".into(),
            },
            CardItem {
                pane_id: PaneId(3),
                status: "done".into(),
                label: "t3".into(),
                profile: None,
                elapsed: String::new(),
            },
        ]
    }

    #[test]
    fn renders_cards() {
        let mut t = Terminal::new(TestBackend::new(60, 1)).unwrap();
        let theme = Theme::default();
        t.draw(|f| render_card_strip(f, f.area(), &items(), &theme))
            .unwrap();
        let line: String = (0..60)
            .map(|x| t.backend().buffer()[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            line.starts_with("[● t1 agy 4m] [? t2 needs input] [✓ t3]"),
            "{line}"
        );
    }

    #[test]
    fn hit_testing() {
        let area = Rect::new(0, 5, 60, 1);
        let it = items();
        assert_eq!(card_at(area, &it, 0, 5), Some(PaneId(1)));
        assert_eq!(card_at(area, &it, 12, 5), Some(PaneId(1)));
        assert_eq!(card_at(area, &it, 13, 5), None); // gap
        assert_eq!(card_at(area, &it, 14, 5), Some(PaneId(2)));
        assert_eq!(card_at(area, &it, 14, 4), None);
        assert_eq!(card_at(area, &it, 59, 5), None);
        assert_eq!(format_elapsed(250), "4m");
    }
}
