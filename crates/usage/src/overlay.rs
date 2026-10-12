use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;

use crate::store::{UsageAggregate, UsageStore};

#[derive(Default)]
pub struct UsageOverlayState {
    pub is_open: bool,
}

pub fn render_usage_overlay(
    frame: &mut Frame,
    total_area: Rect,
    store: &UsageStore,
    state: &UsageOverlayState,
    theme: &Theme,
) {
    if !state.is_open {
        return;
    }

    let width = (total_area.width * 7 / 10)
        .clamp(50, 100)
        .min(total_area.width);
    let height = (total_area.height * 6 / 10)
        .clamp(14, 28)
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
        .title(" Token Usage & Estimated Cost (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(theme.accent);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let today = store.query_today().unwrap_or_default();
    let rolling_5h = store.query_rolling_5h().unwrap_or_default();

    let mut lines = Vec::new();

    lines.push(Line::from(vec![Span::styled(
        "── Today's Totals ──",
        theme.title_focused,
    )]));
    format_aggregate_lines(&today, &mut lines, theme);

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        "── Rolling 5-Hour Window ──",
        theme.title_focused,
    )]));
    format_aggregate_lines(&rolling_5h, &mut lines, theme);

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        "Note: All costs are calculated using pricing.toml and are labeled as ESTIMATES.",
        theme.text.add_modifier(Modifier::DIM),
    )]));

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}

fn format_aggregate_lines(agg: &UsageAggregate, lines: &mut Vec<Line<'static>>, theme: &Theme) {
    let in_fmt = format_number(agg.input_tokens);
    let out_fmt = format_number(agg.output_tokens);
    let cache_fmt = format_number(agg.cache_read);
    let cost_fmt = format!("${:.4}", agg.estimated_cost_usd);

    lines.push(Line::from(vec![
        Span::styled(format!("  Input Tokens:      {:<15}", in_fmt), theme.text),
        Span::styled(format!("Output Tokens:     {:<15}", out_fmt), theme.text),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  Cache Read Tokens: {:<15}", cache_fmt),
            theme.text,
        ),
        Span::styled(
            format!("Est. Cost:         {:<15}", cost_fmt),
            theme.accent.add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![Span::styled(
        format!("  Total Events:      {:<15}", agg.event_count),
        theme.text,
    )]));
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
