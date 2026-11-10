use super::{
    App, Confirm, Focus,
    identity::Identity,
    list,
    prefix::{Action, PREFIX},
    sessions::Embedded,
    theme,
};
use super::badges;
use crate::{
    activity::ActivityMap,
    terminal::{Size, TerminalWidget, input},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::Color,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use vt100::MouseProtocolMode;

const WHEEL_LINES: usize = 3;
const STRIP_TITLE_MAX: usize = 24;
const STRIP_TITLE_MIN: usize = 6;
const STRIP_ENTRY_OVERHEAD: usize = 16;
const FALLBACK_SIZE: Size = Size { rows: 24, cols: 80 };

#[derive(Clone, Copy)]
pub struct PaneStyle {
    pub focused: bool,
    pub tint: Option<Color>,
}

pub fn pane_block(title: Option<(&str, PaneStyle)>) -> Block<'static> {
    let style = title.map_or(PaneStyle { focused: false, tint: None }, |(_, style)| style);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::zone_border(style.focused, style.tint));
    if let Some((title, style)) = title {
        let marker = if style.focused { "▶ " } else { "" };
        block = block.title(Span::styled(
            format!(" {marker}{title} "),
            theme::zone_title(style.focused, style.tint),
        ));
    }
    block
}

pub fn inner_size(area: Rect) -> Size {
    let inner = pane_block(None).inner(area);
    Size {
        rows: inner.height,
        cols: inner.width,
    }
}

pub fn draw_pane(frame: &mut Frame, area: Rect, session: &Embedded, style: PaneStyle) {
    let focused = style.focused;
    let exit = session.exit_status();
    let title = list::truncate(&session.title, area.width.saturating_sub(16) as usize);
    let mut block = pane_block(Some((&format!("SESSION · {title}"), style)));
    if let Some(exit) = &exit {
        let reason = match &exit.signal {
            Some(signal) => format!("exited ({signal})"),
            None => format!("exited (code {})", exit.code),
        };
        block = block.title_bottom(Span::styled(
            format!(" {reason} — press any key "),
            theme::notice(),
        ));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let widget = TerminalWidget::new(session.host()).show_cursor(false);
    if focused && exit.is_none() {
        if let Some(position) = widget.cursor_position(inner) {
            frame.set_cursor_position(position);
        }
    }
    frame.render_widget(widget, inner);
    if !focused {
        frame.buffer_mut().set_style(inner, theme::dimmed());
    }
}

pub fn draw_strip(
    frame: &mut Frame,
    area: Rect,
    sessions: &[Embedded],
    shown: Option<u64>,
    focused: bool,
    activity: &ActivityMap,
    identity: &Identity,
) {
    let mut spans = vec![Span::styled(" sessions ", theme::tab_inactive())];
    let mut used = spans[0].width();
    let per_session = (area.width as usize).saturating_sub(used) / sessions.len().max(1);
    let title_width = per_session
        .saturating_sub(STRIP_ENTRY_OVERHEAD)
        .clamp(STRIP_TITLE_MIN, STRIP_TITLE_MAX);
    for (index, session) in sessions.iter().enumerate() {
        let selected = shown == Some(session.id);
        let session_activity = session
            .session_id
            .as_deref()
            .and_then(|session_id| activity.get(session_id));
        let (state, state_style) = match (session.exit_status(), session_activity, &session.status) {
            (Some(exit), _, _) => (format!("exited {}", exit.code), theme::notice()),
            (None, Some(activity), _) => (activity.state.label().to_owned(), theme::activity_dot(&activity.state)),
            (None, None, Some(status)) => (status.label().to_owned(), theme::status_badge(status)),
            (None, None, None) => ("running".to_owned(), theme::secondary()),
        };
        let badge_spans = match (session.exit_status(), session_activity) {
            (None, Some(activity)) => badges::spans(activity),
            _ => Vec::new(),
        };
        let lead = if selected && focused { "▶" } else { " " };
        let label = format!("{lead}{} {} ", index + 1, list::truncate(&session.title, title_width));
        let width = label.chars().count() + state.chars().count() + 2 + badges::width(&badge_spans);
        if used + width > area.width as usize {
            spans.push(Span::styled("…", theme::tab_overflow()));
            break;
        }
        used += width;
        spans.push(Span::styled(
            label,
            theme::identity_chip(identity.color(&session.identity_key()), selected),
        ));
        spans.push(Span::styled(format!("{state} "), state_style));
        spans.extend(badge_spans);
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

impl App {
    pub(super) fn handle_embedded_key(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.shown else {
            self.focus = Focus::List;
            return true;
        };
        let Some(session) = self.sessions.get(id) else {
            self.shown = None;
            self.focus = Focus::List;
            return true;
        };
        if !session.is_running() {
            self.dismiss_exited(id);
            return true;
        }
        let armed_before = self.prefix.is_armed();
        let action = self.prefix.feed(key);
        match action {
            Action::Forward(key) => {
                self.send_to_child(id, key);
                false
            }
            Action::SendPrefix => {
                self.send_to_child(id, PREFIX);
                armed_before
            }
            Action::Armed | Action::Cancelled => true,
            Action::ReturnToList => {
                self.focus = Focus::List;
                true
            }
            Action::CycleColor => {
                self.cycle_shown_color(id);
                true
            }
            Action::Close => {
                self.confirm = Some(Confirm::Close(id));
                true
            }
            Action::Previous => self.switch_embedded(id, -1),
            Action::Next => self.switch_embedded(id, 1),
            Action::ScrollUp => self.scroll_child(id, true),
            Action::ScrollDown => self.scroll_child(id, false),
        }
    }

    fn send_to_child(&self, id: u64, key: KeyEvent) {
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let host = session.host();
        if host.snapshot().scrollback_offset() > 0 {
            host.reset_scroll();
        }
        let _ = input::send_key(host, key);
    }

    fn scroll_child(&self, id: u64, up: bool) -> bool {
        let Some(session) = self.sessions.get(id) else {
            return false;
        };
        let page = (self.pane_size().rows as usize).saturating_sub(1).max(1);
        if up {
            session.host().scroll_up(page);
        } else {
            session.host().scroll_down(page);
        }
        false
    }

    fn switch_embedded(&mut self, from: u64, step: isize) -> bool {
        let Some(target) = self.sessions.neighbor(from, step) else {
            return false;
        };
        self.shown = Some(target);
        let linked_path = self
            .sessions
            .get(target)
            .and_then(|session| session.session_id.as_deref())
            .and_then(|session_id| {
                self.rows
                    .iter()
                    .map(|&index| &self.library.entries()[index])
                    .find(|entry| entry.session_id == session_id)
                    .map(|entry| entry.path.clone())
            });
        if linked_path.is_some() {
            self.select_path(linked_path);
        }
        true
    }

    pub(super) fn dismiss_exited(&mut self, id: u64) {
        self.sessions.remove(id);
        self.shown = self
            .selected_entry()
            .and_then(|entry| self.sessions.for_session_id(&entry.session_id));
        self.focus = match (self.shown, self.selected_path.is_some()) {
            (Some(_), _) => Focus::Embedded,
            (None, true) => Focus::Conversation,
            (None, false) => Focus::List,
        };
    }

    pub(super) fn handle_paste(&mut self, text: &str) {
        let Some(session) = self.focused_running_embedded() else {
            return;
        };
        let _ = input::send_paste(session.host(), text);
    }

    pub(super) fn handle_mouse(&mut self, event: MouseEvent) {
        let Some(session) = self.focused_running_embedded() else {
            return;
        };
        let inner = self.pane_inner;
        let inside = event.column >= inner.x
            && event.column < inner.x + inner.width
            && event.row >= inner.y
            && event.row < inner.y + inner.height;
        if !inside {
            return;
        }
        let host = session.host();
        let (mode, alternate_screen) = {
            let snapshot = host.snapshot();
            let screen = snapshot.screen();
            (screen.mouse_protocol_mode(), screen.alternate_screen())
        };
        if mode != MouseProtocolMode::None {
            let _ = input::send_mouse(host, event, (inner.x, inner.y));
            return;
        }
        let up = match event.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return,
        };
        if alternate_screen {
            let code = if up { KeyCode::Up } else { KeyCode::Down };
            for _ in 0..WHEEL_LINES {
                let _ = input::send_key(host, KeyEvent::new(code, KeyModifiers::NONE));
            }
        } else if up {
            host.scroll_up(WHEEL_LINES);
        } else {
            host.scroll_down(WHEEL_LINES);
        }
    }

    pub(super) fn wants_mouse_capture(&self) -> bool {
        let Some(session) = self.focused_running_embedded() else {
            return false;
        };
        let snapshot = session.host().snapshot();
        let screen = snapshot.screen();
        screen.mouse_protocol_mode() != MouseProtocolMode::None || screen.alternate_screen()
    }

    fn focused_running_embedded(&self) -> Option<&Embedded> {
        if self.focus != Focus::Embedded {
            return None;
        }
        self.shown
            .and_then(|id| self.sessions.get(id))
            .filter(|session| session.is_running())
    }

    pub(super) fn pane_size(&self) -> Size {
        let inner = self.pane_inner;
        if inner.width == 0 || inner.height == 0 {
            FALLBACK_SIZE
        } else {
            Size {
                rows: inner.height,
                cols: inner.width,
            }
        }
    }
}
