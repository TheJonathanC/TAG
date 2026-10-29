use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use tag_core::event::{AppEvent, EventResult};
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_core::theme::Theme;

pub struct PokerPane {
    id: PaneId,
    title: String,
    pot: u32,
    player_chips: u32,
    player_hand: Vec<String>,
    community_cards: Vec<String>,
    current_bet: u32,
}

impl PokerPane {
    pub fn new(id: PaneId) -> Self {
        Self {
            id,
            title: "Texas Hold'em".into(),
            pot: 150,
            player_chips: 1000,
            player_hand: vec!["As".into(), "Ks".into()],
            community_cards: vec![
                "Qs".into(),
                "Js".into(),
                "10d".into(),
                "2c".into(),
                "7h".into(),
            ],
            current_bet: 50,
        }
    }
}

impl Pane for PokerPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn kind(&self) -> PaneKind {
        PaneKind::Applet
    }

    fn applet_name(&self) -> Option<&'static str> {
        Some("poker")
    }

    fn is_active(&self) -> bool {
        true
    }

    fn send_input(&mut self, data: &[u8]) {
        if let Ok(text) = std::str::from_utf8(data) {
            for c in text.chars() {
                match c.to_ascii_lowercase() {
                    'c' => {
                        let call = self.current_bet.min(self.player_chips);
                        self.player_chips -= call;
                        self.pot += call;
                        self.current_bet = 0;
                    }
                    'b' => {
                        let raise = 50.min(self.player_chips);
                        self.player_chips -= raise;
                        self.pot += raise;
                        self.current_bet += raise;
                    }
                    'f' => {
                        self.pot = 100;
                        self.current_bet = 25;
                    }
                    _ => {}
                }
            }
        }
    }

    fn on_resize(&mut self, _width: u16, _height: u16) {}

    fn handle_event(&mut self, _event: &AppEvent) -> EventResult {
        EventResult::Ignored
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool, theme: &Theme) {
        let border_style = if is_focused {
            theme.border_focused
        } else {
            theme.border_unfocused
        };

        let block = Block::default()
            .title(" Texas Hold'em Applet ")
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = Vec::new();
        lines.push(Line::raw(""));

        let pot_line = format!(
            "  Pot: ${}  |  Current Bet: ${}",
            self.pot, self.current_bet
        );
        lines.push(Line::from(vec![Span::styled(
            pot_line,
            theme.accent.add_modifier(Modifier::BOLD),
        )]));

        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("  Community: ", theme.text),
            Span::styled(
                self.community_cards.join("  "),
                theme.title_focused.add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("  Your Hand: ", theme.text),
            Span::styled(
                self.player_hand.join("  "),
                theme.accent.add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  (Chips: ${})", self.player_chips), theme.text),
        ]));

        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled(
            "  Actions: [C] Check/Call · [B] Bet/Raise · [F] Fold",
            theme.text.add_modifier(Modifier::DIM),
        )]));

        let p = Paragraph::new(lines);
        frame.render_widget(p, inner);
    }
}
