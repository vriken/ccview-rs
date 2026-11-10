use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::session::TerminalSession;

pub struct TerminalWidget<'a> {
    host: &'a dyn TerminalSession,
    show_cursor: bool,
}

impl<'a> TerminalWidget<'a> {
    pub fn new(host: &'a dyn TerminalSession) -> Self {
        Self {
            host,
            show_cursor: true,
        }
    }

    pub fn show_cursor(mut self, show_cursor: bool) -> Self {
        self.show_cursor = show_cursor;
        self
    }

    pub fn cursor_position(&self, area: Rect) -> Option<Position> {
        let (row, col) = self.host.snapshot().cursor()?;
        (col < area.width && row < area.height).then(|| Position::new(area.x + col, area.y + row))
    }
}

impl Widget for TerminalWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let snapshot = self.host.snapshot();
        let screen = snapshot.screen();
        let size = snapshot.size();
        let visible_rows = area.height.min(size.rows);
        let visible_cols = area.width.min(size.cols);

        for row in 0..visible_rows {
            for col in 0..visible_cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let target = &mut buf[(area.x + col, area.y + row)];
                let clipped_wide = cell.is_wide() && col + 1 >= visible_cols;
                if !cell.has_contents() || clipped_wide {
                    target.set_symbol(" ");
                } else {
                    target.set_symbol(cell.contents());
                }
                target.set_style(cell_style(cell));
            }
        }

        if self.show_cursor
            && let Some((row, col)) = snapshot.cursor()
            && row < visible_rows
            && col < visible_cols
        {
            let target = &mut buf[(area.x + col, area.y + row)];
            target.set_style(target.style().add_modifier(Modifier::REVERSED));
        }
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut modifiers = Modifier::empty();
    if cell.bold() {
        modifiers |= Modifier::BOLD;
    }
    if cell.dim() {
        modifiers |= Modifier::DIM;
    }
    if cell.italic() {
        modifiers |= Modifier::ITALIC;
    }
    if cell.underline() {
        modifiers |= Modifier::UNDERLINED;
    }
    if cell.inverse() {
        modifiers |= Modifier::REVERSED;
    }
    Style::reset()
        .fg(convert_color(cell.fgcolor()))
        .bg(convert_color(cell.bgcolor()))
        .add_modifier(modifiers)
}

fn convert_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
