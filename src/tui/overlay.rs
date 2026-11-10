pub(super) mod help;
pub(super) mod settings;

use crate::tui::{
    App,
    embedded::pane::{self, PaneStyle},
    style::theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Clear, Paragraph},
};

const MAX_WIDTH: u16 = 76;
const MIN_WIDTH: u16 = 20;
const SCREEN_MARGIN: u16 = 2;
const CHROME_ROWS: u16 = 3;
const WHEEL_LINES: usize = 3;

pub enum Kind {
    Settings(settings::State),
    Help,
}

pub struct Overlay {
    pub kind: Kind,
    pub scroll: usize,
    area: Rect,
    page: usize,
    max_scroll: usize,
}

impl Overlay {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            scroll: 0,
            area: Rect::default(),
            page: 1,
            max_scroll: 0,
        }
    }
}

pub struct Body {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    pub hint: String,
    pub follow: Option<usize>,
    pub fit: bool,
}

pub fn width_for(screen: Rect) -> u16 {
    screen
        .width
        .saturating_sub(2 * SCREEN_MARGIN)
        .clamp(MIN_WIDTH.min(screen.width), MAX_WIDTH)
}

fn centered(screen: Rect, width: u16, wanted_rows: Option<usize>) -> Rect {
    let max_height = screen.height.saturating_sub(SCREEN_MARGIN).max(1);
    let height = wanted_rows.map_or(max_height, |rows| {
        (rows as u16).saturating_add(CHROME_ROWS).min(max_height)
    });
    let width = width.min(screen.width);
    Rect {
        x: screen.x + (screen.width - width) / 2,
        y: screen.y + (screen.height - height) / 2,
        width,
        height,
    }
}

impl App {
    pub(super) fn open_settings(&mut self) {
        self.open_overlay(Kind::Settings(settings::State::default()));
    }

    pub(super) fn open_help(&mut self) {
        self.open_overlay(Kind::Help);
    }

    fn open_overlay(&mut self, kind: Kind) {
        self.overlay = Some(Overlay::new(kind));
        self.forwarded_presses.clear();
        self.prefix.reset();
    }

    pub(super) fn overlay_key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let Some(mut overlay) = self.overlay.take() else {
            return false;
        };
        let keep = match &mut overlay.kind {
            Kind::Settings(state) => self.settings_key(state, key.code),
            Kind::Help => help::key(&mut overlay, key.code),
        };
        if keep {
            self.overlay = Some(overlay);
        }
        true
    }

    pub(super) fn overlay_mouse(&mut self, event: MouseEvent) -> bool {
        let Some(mut overlay) = self.overlay.take() else {
            return false;
        };
        let inside = overlay.area.contains((event.column, event.row).into());
        let mut changed = false;
        let mut keep = true;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if !inside => {
                keep = false;
                changed = true;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = event.kind == MouseEventKind::ScrollUp;
                match &mut overlay.kind {
                    Kind::Help => scroll(&mut overlay, up, WHEEL_LINES),
                    Kind::Settings(state) => {
                        self.settings_key(state, if up { KeyCode::Up } else { KeyCode::Down });
                    }
                }
                changed = true;
            }
            _ => {}
        }
        if keep {
            self.overlay = Some(overlay);
        }
        changed
    }

    pub(super) fn draw_overlay(&mut self, frame: &mut Frame) {
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        let screen = frame.area();
        let width = width_for(screen);
        let body = match &mut overlay.kind {
            Kind::Settings(state) => self.settings_body(state, width.saturating_sub(4) as usize),
            Kind::Help => help::body(width.saturating_sub(4) as usize),
        };
        let area = centered(screen, width, body.fit.then_some(body.lines.len()));
        let block = pane::pane_block(Some((
            &body.title,
            PaneStyle {
                focused: true,
                tint: None,
            },
        )));
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        let rows = (inner.height as usize).saturating_sub(1);
        overlay.page = rows.saturating_sub(1).max(1);
        overlay.max_scroll = body.lines.len().saturating_sub(rows);
        let offset = match body.follow {
            Some(line) => (line + 1).saturating_sub(rows),
            None => overlay.scroll.min(overlay.max_scroll),
        };
        overlay.scroll = offset.min(overlay.max_scroll);
        overlay.area = area;
        let body_area = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            height: rows as u16,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(
                body.lines
                    .into_iter()
                    .skip(offset)
                    .take(rows)
                    .collect::<Vec<_>>(),
            ),
            body_area,
        );
        let hint_area = Rect {
            x: body_area.x,
            y: inner.y + rows as u16,
            width: body_area.width,
            height: inner.height.min(1),
        };
        frame.render_widget(
            Paragraph::new(body.hint).style(theme::footer_text()),
            hint_area,
        );
        self.overlay = Some(overlay);
    }
}

pub fn scroll(overlay: &mut Overlay, up: bool, lines: usize) {
    overlay.scroll = if up {
        overlay.scroll.saturating_sub(lines)
    } else {
        overlay.scroll.saturating_add(lines).min(overlay.max_scroll)
    };
}

pub fn page(overlay: &Overlay) -> usize {
    overlay.page
}
