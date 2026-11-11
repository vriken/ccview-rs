use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Debug)]
pub(crate) struct Theme {
    pub(crate) h1: Style,
    pub(crate) h2: Style,
    pub(crate) h3: Style,
    pub(crate) h4: Style,
    pub(crate) strong: Style,
    pub(crate) emphasis: Style,
    pub(crate) strikethrough: Style,
    pub(crate) inline_code: Style,
    pub(crate) code_block: Style,
    pub(crate) quote: Style,
    pub(crate) link: Style,
    pub(crate) list_marker: Style,
    pub(crate) rule: Style,
    pub(crate) table_border: Style,
    pub(crate) dim: Style,
}

impl Default for Theme {
    fn default() -> Self {
        let muted = Color::Indexed(244);
        Self {
            h1: Style::new()
                .fg(Color::Indexed(213))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            h2: Style::new()
                .fg(Color::Indexed(81))
                .add_modifier(Modifier::BOLD),
            h3: Style::new()
                .fg(Color::Indexed(114))
                .add_modifier(Modifier::BOLD),
            h4: Style::new().add_modifier(Modifier::BOLD),
            strong: Style::new().add_modifier(Modifier::BOLD),
            emphasis: Style::new().add_modifier(Modifier::ITALIC),
            strikethrough: Style::new().add_modifier(Modifier::CROSSED_OUT),
            inline_code: Style::new().fg(Color::Indexed(216)).bg(Color::Indexed(236)),
            code_block: Style::new().fg(Color::Indexed(252)).bg(Color::Indexed(235)),
            quote: Style::new().fg(Color::Indexed(246)),
            link: Style::new()
                .fg(Color::Indexed(75))
                .add_modifier(Modifier::UNDERLINED),
            list_marker: Style::new().fg(Color::Indexed(109)),
            rule: Style::new().fg(Color::Indexed(240)),
            table_border: Style::new().fg(Color::Indexed(240)),
            dim: Style::new().fg(muted),
        }
    }
}
