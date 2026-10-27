use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use tag_core::event::{AppEvent, EventResult};
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_core::theme::Theme;

pub struct PeerChatPane {
    id: PaneId,
    title: String,
    messages: Vec<(String, String)>, // (sender, text)
    input: String,
}

impl PeerChatPane {
    pub fn new(id: PaneId) -> Self {
        let mut pane = Self {
            id,
            title: "Peer Chat".into(),
            messages: Vec::new(),
            input: String::new(),
        };
        pane.messages.push((
            "System".into(),
            "Local scratch chat: messages stay in this pane and are not sent over the network."
                .into(),
        ));
        pane
    }
}

impl Pane for PeerChatPane {
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
        Some("chat")
    }

    fn is_active(&self) -> bool {
        true
    }

    fn send_input(&mut self, data: &[u8]) {
        if let Ok(text) = std::str::from_utf8(data) {
            // Pasted text arrives wrapped in bracketed-paste markers
            let text = text.replace("\x1b[200~", "").replace("\x1b[201~", "");
            for c in text.chars() {
                if c == '\r' || c == '\n' {
                    if !self.input.is_empty() {
                        let msg = std::mem::take(&mut self.input);
                        self.messages.push(("You".into(), msg));
                    }
                } else if c == '\x08' || c == '\x7f' {
                    self.input.pop();
                } else if !c.is_control() {
                    self.input.push(c);
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
            .title(" Peer Chat (local only) ")
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let msg_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        let input_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };

        let mut items = Vec::new();
        for (sender, text) in &self.messages {
            let sender_style = if sender == "System" {
                theme.info
            } else {
                theme.accent.add_modifier(Modifier::BOLD)
            };

            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!("[{}] ", sender), sender_style),
                Span::styled(text.clone(), theme.text),
            ])));
        }

        let list = List::new(items);
        frame.render_widget(list, msg_area);

        let input_line = format!("Chat > {}_", self.input);
        let p = Paragraph::new(input_line).style(theme.text);
        frame.render_widget(p, input_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paste_markers_and_backspace() {
        let mut pane = PeerChatPane::new(PaneId(1));
        pane.send_input(b"\x1b[200~hello\x1b[201~");
        assert_eq!(pane.input, "hello");
        pane.send_input(&[0x7f]);
        assert_eq!(pane.input, "hell");
        pane.send_input(b"\r");
        assert_eq!(pane.messages.last().map(|m| m.1.as_str()), Some("hell"));
    }
}
