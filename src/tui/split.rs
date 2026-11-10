use super::theme;
use crate::ui_settings::{DEFAULT_SPLIT_RATIO, UiSettings, UiSettingsStore};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::Widget,
};
use std::time::{Duration, Instant};

pub const MIN_LIST_WIDTH: u16 = 24;
pub const MIN_DETAIL_WIDTH: u16 = 30;
pub const DIVIDER_WIDTH: u16 = 1;
const NARROW_STEP: i16 = 2;
const WIDE_STEP: i16 = 5;
const DRAG_RESIZE_INTERVAL: Duration = Duration::from_millis(60);
const DIVIDER_IDLE: &str = "┊";
const DIVIDER_ACTIVE: &str = "┃";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    Delta(i16),
    Reset,
}

impl Resize {
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let step = if key.kind == KeyEventKind::Repeat
            || key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
        {
            WIDE_STEP
        } else {
            NARROW_STEP
        };
        match key.code {
            KeyCode::Char('<') => Some(Self::Delta(-step)),
            KeyCode::Char('>') => Some(Self::Delta(step)),
            KeyCode::Char('=') => Some(Self::Reset),
            _ => None,
        }
    }
}

fn list_width_bounds(usable: u16) -> (u16, u16) {
    if usable >= MIN_LIST_WIDTH + MIN_DETAIL_WIDTH {
        (MIN_LIST_WIDTH, usable - MIN_DETAIL_WIDTH)
    } else {
        let proportional = u32::from(usable) * u32::from(MIN_LIST_WIDTH)
            / u32::from(MIN_LIST_WIDTH + MIN_DETAIL_WIDTH);
        (proportional as u16, proportional as u16)
    }
}

pub fn list_width(body_width: u16, ratio: f64) -> u16 {
    let (min, max) = list_width_bounds(body_width.saturating_sub(DIVIDER_WIDTH));
    ((ratio * f64::from(body_width)).round() as u16).clamp(min, max)
}

pub fn ratio_for_list_width(body_width: u16, width: u16) -> f64 {
    if body_width == 0 {
        return DEFAULT_SPLIT_RATIO;
    }
    f64::from(width) / f64::from(body_width)
}

pub struct Panes {
    pub list: Rect,
    pub divider: Rect,
    pub detail: Rect,
}

pub fn panes(body: Rect, ratio: f64) -> Panes {
    let width = list_width(body.width, ratio);
    let divider_width = DIVIDER_WIDTH.min(body.width.saturating_sub(width));
    let detail_x = body.x + width + divider_width;
    Panes {
        list: Rect { width, ..body },
        divider: Rect {
            x: body.x + width,
            width: divider_width,
            ..body
        },
        detail: Rect {
            x: detail_x,
            width: body.width.saturating_sub(width + divider_width),
            ..body
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Drag {
    #[default]
    Idle,
    Dragging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragStep {
    Ignored,
    Started,
    Moved(u16),
    Ended(u16),
}

pub fn drag_step(drag: Drag, kind: MouseEventKind, column: u16, on_divider: bool) -> (Drag, DragStep) {
    match (drag, kind) {
        (Drag::Idle, MouseEventKind::Down(MouseButton::Left)) if on_divider => {
            (Drag::Dragging, DragStep::Started)
        }
        (Drag::Dragging, MouseEventKind::Drag(MouseButton::Left)) => {
            (Drag::Dragging, DragStep::Moved(column))
        }
        (Drag::Dragging, MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Moved) => {
            (Drag::Idle, DragStep::Ended(column))
        }
        (Drag::Dragging, _) => (Drag::Dragging, DragStep::Ignored),
        (Drag::Idle, _) => (Drag::Idle, DragStep::Ignored),
    }
}

pub struct SplitState {
    store: UiSettingsStore,
    ratio: f64,
    body: Rect,
    drag: Drag,
    hover: bool,
    last_pane_resize: Option<Instant>,
}

impl SplitState {
    pub fn new(store: UiSettingsStore) -> Self {
        Self {
            ratio: store.settings().split_ratio,
            store,
            body: Rect::default(),
            drag: Drag::Idle,
            hover: false,
            last_pane_resize: None,
        }
    }

    pub fn mouse_enabled(&self) -> bool {
        self.store.settings().mouse
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn settings(&self) -> UiSettings {
        self.store.settings()
    }

    pub fn update_settings(&mut self, change: impl FnOnce(&mut UiSettings)) -> std::io::Result<()> {
        self.store.update(change)
    }

    pub fn is_dragging(&self) -> bool {
        self.drag == Drag::Dragging
    }

    pub fn layout(&mut self, body: Rect) -> Panes {
        self.body = body;
        panes(body, self.ratio)
    }

    pub fn divider_hit(&self, column: u16, row: u16) -> bool {
        let divider = panes(self.body, self.ratio).divider;
        divider.width > 0
            && column >= divider.x
            && column < divider.x + divider.width
            && row >= divider.y
            && row < divider.y + divider.height
    }

    pub fn pane_resize_due(&mut self) -> bool {
        let due = !self.is_dragging()
            || self
                .last_pane_resize
                .is_none_or(|at| at.elapsed() >= DRAG_RESIZE_INTERVAL);
        if due {
            self.last_pane_resize = Some(Instant::now());
        }
        due
    }

    pub fn resize(&mut self, resize: Resize) -> std::io::Result<bool> {
        let before = self.ratio;
        match resize {
            Resize::Reset => self.ratio = DEFAULT_SPLIT_RATIO,
            Resize::Delta(columns) => {
                let current = list_width(self.body.width, self.ratio);
                let target = (i32::from(current) + i32::from(columns)).max(0) as u16;
                self.set_list_width(target);
            }
        }
        if self.body.width == 0 {
            self.ratio = before;
            return Ok(false);
        }
        self.persist(before)
    }

    fn set_list_width(&mut self, width: u16) {
        let clamped = list_width(self.body.width, ratio_for_list_width(self.body.width, width));
        self.ratio = ratio_for_list_width(self.body.width, clamped);
    }

    fn persist(&mut self, before: f64) -> std::io::Result<bool> {
        if self.ratio == before {
            return Ok(false);
        }
        self.store.set_split_ratio(self.ratio).map(|()| true)
    }

    pub fn handle_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
    ) -> (bool, std::io::Result<()>) {
        let on_divider = self.divider_hit(column, row);
        let (drag, step) = drag_step(self.drag, kind, column, on_divider);
        let hover = on_divider || drag == Drag::Dragging;
        let mut consumed = step != DragStep::Ignored;
        let mut saved = Ok(());
        self.drag = drag;
        match step {
            DragStep::Started | DragStep::Ignored => {}
            DragStep::Moved(column) => self.set_list_width(column.saturating_sub(self.body.x)),
            DragStep::Ended(column) => {
                let before = self.ratio;
                self.set_list_width(column.saturating_sub(self.body.x));
                saved = self.persist(before).map(|_| ());
                if saved.is_ok() && self.ratio == before {
                    saved = self.store.set_split_ratio(self.ratio);
                }
            }
        }
        consumed |= hover != self.hover;
        self.hover = hover;
        (consumed, saved)
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Divider {
                active: self.hover || self.is_dragging(),
            },
            area,
        );
    }
}

struct Divider {
    active: bool,
}

impl Widget for Divider {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (glyph, style): (&str, Style) = if self.active {
            (DIVIDER_ACTIVE, theme::zone_border(true, None))
        } else {
            (DIVIDER_IDLE, theme::zone_border(false, None))
        };
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_symbol(glyph).set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: Rect = Rect { x: 0, y: 2, width: 120, height: 30 };

    fn state_in(name: &str) -> (SplitState, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("ccview-split-{}-{name}/ui.json", std::process::id()));
        let mut state = SplitState::new(UiSettingsStore::load(path.clone()));
        state.layout(BODY);
        (state, path)
    }

    #[test]
    fn list_width_is_clamped_at_both_extremes_and_degrades_on_narrow_terminals() {
        assert_eq!(list_width(120, 0.0), MIN_LIST_WIDTH);
        assert_eq!(list_width(120, 1.0), 120 - DIVIDER_WIDTH - MIN_DETAIL_WIDTH);
        assert_eq!(list_width(120, 0.35), 42);
        let narrow = list_width(40, 0.9);
        assert_eq!(narrow, list_width(40, 0.0));
        assert!(narrow + DIVIDER_WIDTH < 40);
        assert_eq!(list_width(0, 0.5), 0);
        let panes = panes(Rect::new(0, 0, 40, 10), 0.5);
        assert_eq!(panes.list.width + panes.divider.width + panes.detail.width, 40);
    }

    #[test]
    fn dragging_the_divider_sets_the_ratio_and_persists_on_release() {
        let (mut state, path) = state_in("drag");
        let divider = state.layout(BODY).divider.x;
        let down = MouseEventKind::Down(MouseButton::Left);
        assert!(state.handle_mouse(down, divider, 10).0);
        assert!(state.is_dragging());
        let _ = state.handle_mouse(MouseEventKind::Drag(MouseButton::Left), 60, 10);
        assert_eq!(list_width(120, state.ratio), 60);
        assert!(!path.exists());
        let _ = state.handle_mouse(MouseEventKind::Up(MouseButton::Left), 70, 10).1.unwrap();
        assert!(!state.is_dragging());
        assert_eq!(UiSettingsStore::load(path.clone()).settings().split_ratio, state.ratio);
        assert_eq!(list_width(120, state.ratio), 70);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn dragging_past_either_edge_stops_at_the_minimum_widths() {
        let (mut state, path) = state_in("edges");
        let divider = state.layout(BODY).divider.x;
        let _ = state.handle_mouse(MouseEventKind::Down(MouseButton::Left), divider, 10);
        let _ = state.handle_mouse(MouseEventKind::Drag(MouseButton::Left), 0, 10);
        assert_eq!(list_width(120, state.ratio), MIN_LIST_WIDTH);
        let _ = state.handle_mouse(MouseEventKind::Drag(MouseButton::Left), 119, 10);
        assert_eq!(list_width(120, state.ratio), 120 - DIVIDER_WIDTH - MIN_DETAIL_WIDTH);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

}
