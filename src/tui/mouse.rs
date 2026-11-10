use super::{App, Focus, nav::ListFocus, tabs};
use crate::terminal::input;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use vt100::MouseProtocolMode;

const WHEEL_LINES: usize = 3;
const LIST_ROW_HEIGHT: u16 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Regions {
    pub tab_bar: Rect,
    pub list: Rect,
    pub divider: Rect,
    pub detail: Rect,
    pub child_inner: Option<Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Tabs,
    List,
    Divider,
    Detail { in_child: bool },
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    Transcript,
    Child {
        reporting: bool,
        alternate_screen: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    SelectTab,
    SelectRow,
    FocusDetail,
    ListWheel(Wheel),
    TranscriptWheel(Wheel),
    ForwardToChild,
    ChildWheelAsArrows(Wheel),
    ChildScrollback(Wheel),
    Ignore,
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

pub fn zone_at(regions: &Regions, column: u16, row: u16) -> Zone {
    if contains(regions.tab_bar, column, row) {
        Zone::Tabs
    } else if contains(regions.list, column, row) {
        Zone::List
    } else if contains(regions.divider, column, row) {
        Zone::Divider
    } else if contains(regions.detail, column, row) {
        Zone::Detail {
            in_child: regions
                .child_inner
                .is_some_and(|inner| contains(inner, column, row)),
        }
    } else {
        Zone::Outside
    }
}

fn wheel(kind: MouseEventKind) -> Option<Wheel> {
    match kind {
        MouseEventKind::ScrollUp => Some(Wheel::Up),
        MouseEventKind::ScrollDown => Some(Wheel::Down),
        _ => None,
    }
}

pub fn route(zone: Zone, kind: MouseEventKind, detail: Detail) -> Route {
    let left_press = kind == MouseEventKind::Down(MouseButton::Left);
    match zone {
        Zone::Tabs if left_press => Route::SelectTab,
        Zone::List if left_press => Route::SelectRow,
        Zone::List => wheel(kind).map_or(Route::Ignore, Route::ListWheel),
        Zone::Detail { in_child } => match detail {
            Detail::Transcript => match wheel(kind) {
                Some(direction) => Route::TranscriptWheel(direction),
                None if left_press => Route::FocusDetail,
                None => Route::Ignore,
            },
            Detail::Child {
                reporting: true, ..
            } if in_child => Route::ForwardToChild,
            Detail::Child { .. } if left_press => Route::FocusDetail,
            Detail::Child {
                alternate_screen, ..
            } => match (wheel(kind), in_child) {
                (Some(direction), true) if alternate_screen => Route::ChildWheelAsArrows(direction),
                (Some(direction), true) => Route::ChildScrollback(direction),
                _ => Route::Ignore,
            },
        },
        Zone::Tabs | Zone::Divider | Zone::Outside => Route::Ignore,
    }
}

pub fn list_row_at(
    list: Rect,
    selected: Option<usize>,
    row_count: usize,
    row: u16,
) -> Option<usize> {
    let inner_top = list.y + 2;
    let inner_height = list.height.saturating_sub(3);
    if row < inner_top || row >= inner_top + inner_height {
        return None;
    }
    let visible_rows = usize::from(inner_height / LIST_ROW_HEIGHT).max(1);
    let offset = selected.map_or(0, |selected| (selected + 1).saturating_sub(visible_rows));
    let index = offset + usize::from((row - inner_top) / LIST_ROW_HEIGHT);
    (index < row_count).then_some(index)
}

impl App {
    pub(super) fn handle_mouse(&mut self, event: MouseEvent) -> bool {
        if self.overlay.is_some() {
            return self.overlay_mouse(event);
        }
        if self.confirm.is_some() {
            return false;
        }
        let (consumed, saved) = self.split.handle_mouse(event.kind, event.column, event.row);
        if let Err(err) = saved {
            self.notice = Some(format!("could not save layout: {err}"));
        }
        if consumed {
            return true;
        }
        let zone = zone_at(&self.regions, event.column, event.row);
        let detail = self.detail_kind();
        match route(zone, event.kind, detail) {
            Route::Ignore => false,
            Route::SelectTab => self.click_tab(event.column),
            Route::SelectRow => self.click_row(event.row),
            Route::FocusDetail => self.focus_detail(),
            Route::ListWheel(direction) => self.wheel_list(direction),
            Route::TranscriptWheel(Wheel::Up) => {
                self.scroll_up(WHEEL_LINES);
                true
            }
            Route::TranscriptWheel(Wheel::Down) => {
                self.scroll_down(WHEEL_LINES);
                true
            }
            Route::ForwardToChild => {
                let focused =
                    event.kind == MouseEventKind::Down(MouseButton::Left) && self.focus_detail();
                self.forward_mouse_to_child(event);
                focused
            }
            Route::ChildWheelAsArrows(direction) => {
                self.wheel_child_as_arrows(direction);
                false
            }
            Route::ChildScrollback(direction) => {
                self.wheel_child_scrollback(direction);
                false
            }
        }
    }

    fn detail_kind(&self) -> Detail {
        let Some(session) = self
            .shown
            .and_then(|id| self.sessions.get(id))
            .filter(|session| session.is_running())
        else {
            return Detail::Transcript;
        };
        let snapshot = session.host().snapshot();
        let screen = snapshot.screen();
        Detail::Child {
            reporting: screen.mouse_protocol_mode() != MouseProtocolMode::None,
            alternate_screen: screen.alternate_screen(),
        }
    }

    fn focus_detail(&mut self) -> bool {
        let target = self.reading_focus();
        let changed = self.focus != target;
        self.focus = target;
        self.prefix.reset();
        changed
    }

    fn click_tab(&mut self, column: u16) -> bool {
        let width = self.regions.tab_bar.width as usize;
        let line = tabs::render_bar(&self.tabs, &self.active, width, false);
        let mut start = self.regions.tab_bar.x;
        let mut next_tab = 0;
        for span in &line.spans {
            let span_width = span.width() as u16;
            let tab_index = (next_tab..self.tabs.len())
                .find(|&index| span.content == format!(" {} ", self.tabs[index].label));
            if let Some(index) = tab_index {
                next_tab = index + 1;
                if column >= start && column < start + span_width {
                    self.focus = Focus::List;
                    self.list_focus = ListFocus::Tabs;
                    self.select_tab(index);
                    return true;
                }
            }
            start += span_width;
        }
        false
    }

    fn click_row(&mut self, row: u16) -> bool {
        let Some(index) = list_row_at(self.regions.list, self.selected_row(), self.rows.len(), row)
        else {
            return false;
        };
        self.focus = Focus::List;
        self.list_focus = ListFocus::Rows;
        self.prefix.reset();
        self.select_row(index);
        true
    }

    fn wheel_list(&mut self, direction: Wheel) -> bool {
        let Some(current) = self.selected_row() else {
            return false;
        };
        let target = match direction {
            Wheel::Up => current.checked_sub(1),
            Wheel::Down => Some(current + 1).filter(|&next| next < self.rows.len()),
        };
        let Some(target) = target else {
            return false;
        };
        self.select_row(target);
        true
    }

    fn running_shown(&self) -> Option<&super::sessions::Embedded> {
        self.shown
            .and_then(|id| self.sessions.get(id))
            .filter(|session| session.is_running())
    }

    fn forward_mouse_to_child(&self, event: MouseEvent) {
        let (Some(session), Some(inner)) = (self.running_shown(), self.regions.child_inner) else {
            return;
        };
        let _ = input::send_mouse(session.host(), event, (inner.x, inner.y));
    }

    fn wheel_child_as_arrows(&self, direction: Wheel) {
        let Some(session) = self.running_shown() else {
            return;
        };
        let code = if direction == Wheel::Up {
            KeyCode::Up
        } else {
            KeyCode::Down
        };
        for _ in 0..WHEEL_LINES {
            let _ = input::send_key(session.host(), KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    fn wheel_child_scrollback(&self, direction: Wheel) {
        let Some(session) = self.running_shown() else {
            return;
        };
        match direction {
            Wheel::Up => session.host().scroll_up(WHEEL_LINES),
            Wheel::Down => session.host().scroll_down(WHEEL_LINES),
        }
    }

    pub(super) fn wants_mouse_capture(&self) -> bool {
        if self.split.mouse_enabled() {
            return true;
        }
        let Some(session) = self.focused_running_embedded() else {
            return false;
        };
        let snapshot = session.host().snapshot();
        let screen = snapshot.screen();
        screen.mouse_protocol_mode() != MouseProtocolMode::None || screen.alternate_screen()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGIONS: Regions = Regions {
        tab_bar: Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 1,
        },
        list: Rect {
            x: 0,
            y: 2,
            width: 30,
            height: 20,
        },
        divider: Rect {
            x: 30,
            y: 2,
            width: 1,
            height: 20,
        },
        detail: Rect {
            x: 31,
            y: 2,
            width: 69,
            height: 20,
        },
        child_inner: Some(Rect {
            x: 32,
            y: 3,
            width: 67,
            height: 18,
        }),
    };
    const CHILD_MOUSE: Detail = Detail::Child {
        reporting: true,
        alternate_screen: false,
    };
    const CHILD_PLAIN: Detail = Detail::Child {
        reporting: false,
        alternate_screen: false,
    };
    const CHILD_ALT: Detail = Detail::Child {
        reporting: false,
        alternate_screen: true,
    };

    #[test]
    fn zones_follow_the_pane_geometry() {
        assert_eq!(zone_at(&REGIONS, 5, 0), Zone::Tabs);
        assert_eq!(zone_at(&REGIONS, 5, 5), Zone::List);
        assert_eq!(zone_at(&REGIONS, 30, 5), Zone::Divider);
        assert_eq!(zone_at(&REGIONS, 31, 5), Zone::Detail { in_child: false });
        assert_eq!(zone_at(&REGIONS, 40, 10), Zone::Detail { in_child: true });
        assert_eq!(zone_at(&REGIONS, 40, 30), Zone::Outside);
    }

    #[test]
    fn wheel_scrolls_the_pane_under_the_pointer() {
        let up = MouseEventKind::ScrollUp;
        let in_child = Zone::Detail { in_child: true };
        assert_eq!(
            route(Zone::List, up, CHILD_MOUSE),
            Route::ListWheel(Wheel::Up)
        );
        assert_eq!(
            route(in_child, up, Detail::Transcript),
            Route::TranscriptWheel(Wheel::Up)
        );
        assert_eq!(route(in_child, up, CHILD_MOUSE), Route::ForwardToChild);
        assert_eq!(
            route(in_child, up, CHILD_ALT),
            Route::ChildWheelAsArrows(Wheel::Up)
        );
        assert_eq!(
            route(in_child, up, CHILD_PLAIN),
            Route::ChildScrollback(Wheel::Up)
        );
        assert_eq!(route(Zone::Divider, up, CHILD_PLAIN), Route::Ignore);
    }

    #[test]
    fn clicks_select_focus_or_go_to_the_child() {
        let press = MouseEventKind::Down(MouseButton::Left);
        let in_child = Zone::Detail { in_child: true };
        assert_eq!(
            route(Zone::Tabs, press, Detail::Transcript),
            Route::SelectTab
        );
        assert_eq!(
            route(Zone::List, press, Detail::Transcript),
            Route::SelectRow
        );
        assert_eq!(
            route(Zone::Detail { in_child: false }, press, Detail::Transcript),
            Route::FocusDetail
        );
        assert_eq!(route(in_child, press, CHILD_PLAIN), Route::FocusDetail);
        assert_eq!(route(in_child, press, CHILD_MOUSE), Route::ForwardToChild);
        assert_eq!(
            route(
                in_child,
                MouseEventKind::Drag(MouseButton::Left),
                CHILD_MOUSE
            ),
            Route::ForwardToChild
        );
        assert_eq!(
            route(
                in_child,
                MouseEventKind::Drag(MouseButton::Left),
                CHILD_PLAIN
            ),
            Route::Ignore
        );
        assert_eq!(
            route(Zone::Detail { in_child: false }, press, CHILD_MOUSE),
            Route::FocusDetail
        );
    }

    #[test]
    fn list_rows_account_for_border_row_height_and_scroll_offset() {
        let list = REGIONS.list;
        assert_eq!(list_row_at(list, Some(0), 10, 3), None);
        assert_eq!(list_row_at(list, Some(0), 10, 4), Some(0));
        assert_eq!(list_row_at(list, Some(0), 10, 7), Some(1));
        assert_eq!(list_row_at(list, Some(0), 1, 7), None);
        assert_eq!(list_row_at(list, Some(9), 10, 4), Some(5));
        assert_eq!(list_row_at(list, Some(9), 10, 16), Some(9));
    }
}
