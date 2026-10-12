use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub name: String,
    pub border_focused: String,
    pub border_unfocused: String,
    pub accent: String,
    pub background: Option<String>,
    pub foreground: String,
    pub warning: String,
    pub error: String,
    pub success: String,
    pub info: String,
    pub nerd_fonts: bool,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "default-dark".into(),
            border_focused: "#61afef".into(),
            border_unfocused: "#3e4451".into(),
            accent: "#98c379".into(),
            background: None,
            foreground: "#abb2bf".into(),
            warning: "#e5c07b".into(),
            error: "#e06c75".into(),
            success: "#98c379".into(),
            info: "#56b6c2".into(),
            nerd_fonts: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub config: ThemeConfig,
    pub border_focused: Style,
    pub border_unfocused: Style,
    pub accent: Style,
    pub text: Style,
    pub warning: Style,
    pub error: Style,
    pub success: Style,
    pub info: Style,
    pub badge: Style,
    pub title_focused: Style,
    pub title_unfocused: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_config(ThemeConfig::default())
    }
}

impl Theme {
    pub fn high_contrast() -> Self {
        let config = ThemeConfig {
            name: "high-contrast".into(),
            border_focused: "#ffffff".into(),
            border_unfocused: "#666666".into(),
            accent: "#ffff00".into(),
            background: Some("#000000".into()),
            foreground: "#ffffff".into(),
            warning: "#ffff00".into(),
            error: "#ff0000".into(),
            success: "#00ff00".into(),
            info: "#00ffff".into(),
            nerd_fonts: false,
        };
        Self::from_config(config)
    }

    pub fn from_config(config: ThemeConfig) -> Self {
        let parse_color = |hex: &str, fallback: Color| -> Color {
            if hex.starts_with('#') && hex.len() == 7 {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[1..3], 16),
                    u8::from_str_radix(&hex[3..5], 16),
                    u8::from_str_radix(&hex[5..7], 16),
                ) {
                    return Color::Rgb(r, g, b);
                }
            }
            fallback
        };

        let col_focused = parse_color(&config.border_focused, Color::Cyan);
        let col_unfocused = parse_color(&config.border_unfocused, Color::DarkGray);
        let col_accent = parse_color(&config.accent, Color::Green);
        let col_fg = parse_color(&config.foreground, Color::White);
        let col_warn = parse_color(&config.warning, Color::Yellow);
        let col_err = parse_color(&config.error, Color::Red);
        let col_succ = parse_color(&config.success, Color::Green);
        let col_info = parse_color(&config.info, Color::Cyan);

        Self {
            border_focused: Style::default().fg(col_focused),
            border_unfocused: Style::default().fg(col_unfocused),
            accent: Style::default().fg(col_accent),
            text: Style::default().fg(col_fg),
            warning: Style::default().fg(col_warn),
            error: Style::default().fg(col_err),
            success: Style::default().fg(col_succ),
            info: Style::default().fg(col_info),
            badge: Style::default().fg(Color::Black).bg(col_accent),
            title_focused: Style::default()
                .fg(col_focused)
                .add_modifier(Modifier::BOLD),
            title_unfocused: Style::default().fg(col_unfocused),
            config,
        }
    }
}
