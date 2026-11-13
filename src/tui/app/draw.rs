use crate::{
    session::{activity, entry::SessionEntry, harness_id::HarnessId},
    tui::{
        app::{App, Focus},
        embedded::pane,
        input::{copy::Target, mouse},
        reader::selection::View,
        style::theme,
        widgets::{footer, list, list::ListView, tabs::TabKey},
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Padding, Paragraph},
};
use std::time::SystemTime;

const LOADING_MARK: &str = " · loading";
const SEARCH_LABEL: &str = "Search";

impl App {
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        self.reap_clean_exits();
        self.sync_identity();
        let strip_height =
            u16::from(self.split.settings().sessions_strip && !self.sessions.is_empty());
        let [strip_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(strip_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let panes = self.split.layout(body_area);
        let (list_area, right_area) = (panes.list, panes.detail);
        if strip_height > 0 {
            pane::draw_strip(
                frame,
                strip_area,
                self.sessions.all(),
                self.shown,
                self.focus == Focus::Embedded,
                self.catalog.activity(),
                &self.identity,
            );
        }
        self.pane_inner = pane::pane_block(None).inner(right_area);
        if self.split.pane_resize_due() {
            self.sessions.resize_all(pane::inner_size(right_area));
        }
        let tab_area = Rect {
            x: list_area.x + 1,
            y: list_area.y + 1,
            width: list_area.width.saturating_sub(2),
            height: 1,
        };
        self.regions = mouse::Regions {
            tab_bar: tab_area,
            list: list_area,
            divider: panes.divider,
            detail: right_area,
            child_inner: self.shown.map(|_| self.pane_inner),
        };
        self.split.draw(frame, panes.divider);
        self.draw_list(frame, list_area);
        match self.shown.and_then(|id| self.sessions.get(id)) {
            Some(session) => {
                let style = pane::PaneStyle {
                    focused: self.focus == Focus::Embedded,
                    tint: self.shown_color(),
                };
                pane::draw_pane(frame, right_area, session, style);
                self.keep_selection_for(Some(Target::Child));
                self.paint_child_selection(frame);
            }
            None => self.draw_conversation(frame, right_area),
        }
        self.draw_footer(frame, footer_area);
        self.draw_overlay(frame);
    }

    pub(in crate::tui) fn draw_list(&self, frame: &mut Frame, area: Rect) {
        let entries = self.catalog.entries();
        let rows: Vec<&SessionEntry> = self.rows.iter().map(|&i| &entries[i]).collect();
        let label = match self.search {
            Some(_) => SEARCH_LABEL,
            None => self
                .tabs
                .iter()
                .find(|tab| tab.key == self.active)
                .map_or("", |tab| tab.label.as_str()),
        };
        let empty_message = match (&self.active, self.scanning) {
            _ if self.search_empty_message().is_some() => {
                self.search_empty_message().unwrap_or_default()
            }
            (TabKey::Favorites, _) => {
                "No favorites yet.\n\nSelect a session and press s to star it."
            }
            (TabKey::Active, _) => "Nothing is busy, blocked or running agents.",
            (_, true) => "Scanning sessions…",
            _ => "No sessions.",
        };
        list::draw(
            frame,
            area,
            &ListView {
                title: format!("{label} · {}", rows.len()),
                tabs: &self.tabs,
                active: &self.active,
                identity: &self.identity,
                selected: self.selected_row(),
                entries: rows,
                show_directory: !matches!(self.active, TabKey::Dir(_)),
                focused: self.focus == Focus::List,
                tab_bar_focused: self.tab_bar_has_focus(),
                empty_message,
                favorites: &self.favorites,
                important: &self.important,
                on_hold: &self.on_hold,
                hidden: &self.hidden,
                activity: self.catalog.activity(),
                dismissed: self.catalog.dismissed(),
                shared: self.catalog.shared_checkouts(),
                now: SystemTime::now(),
                drag: self.drag_view(),
                search: self.search_view(),
            },
        );
    }

    pub(in crate::tui) fn draw_conversation(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Conversation;
        let loading_mark = if self.transcript.is_stale() {
            LOADING_MARK
        } else {
            ""
        };
        let title = self
            .selected_entry()
            .map(|entry| {
                let harness = entry.harness.as_str();
                let room = (area.width.saturating_sub(9) as usize)
                    .saturating_sub(loading_mark.chars().count() + harness.len() + 3);
                format!(
                    "{harness} · {}{loading_mark}",
                    list::truncate(&entry.title, room)
                )
            })
            .unwrap_or_else(|| "Conversation".to_owned());
        let style = pane::PaneStyle {
            focused,
            tint: self.selected_color(),
        };
        let block = pane::pane_block(Some((&format!("TRANSCRIPT · {title}"), style)))
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        let cwd = self
            .selected_entry()
            .map(|entry| entry.cwd.clone())
            .unwrap_or_default();
        self.viewport_height = inner.height as usize;
        self.text_area = Rect::default();
        if !matches!(self.transcript.view(), View::Content(_)) {
            self.selection = None;
        }
        let conversation = match self.transcript.view() {
            View::Idle => {
                let hint = "Select a session to read it here";
                frame.render_widget(
                    Paragraph::new(hint)
                        .style(theme::empty_state())
                        .block(block),
                    area,
                );
                return;
            }
            View::Loading => {
                frame.render_widget(
                    Paragraph::new("Loading…")
                        .style(theme::empty_state())
                        .block(block),
                    area,
                );
                return;
            }
            View::Error(error) => {
                frame.render_widget(
                    Paragraph::new(error.to_owned())
                        .style(theme::notice())
                        .block(block),
                    area,
                );
                return;
            }
            View::Content(conversation) => conversation,
        };
        let text_width = inner.width as usize;
        conversation.set_cwd(&cwd);
        conversation.lines(text_width, self.view.level);
        let prepended = conversation.take_prepend_shift();
        if let Some(turn) = self.scroll_anchor.take() {
            self.scroll = conversation.first_line_of_turn(turn);
        } else {
            self.scroll = self.scroll.saturating_add_signed(prepended);
        }
        let lines = conversation.lines(text_width, self.view.level);
        self.text_area = Rect {
            x: inner.x,
            y: inner.y,
            width: text_width as u16,
            height: inner.height,
        };
        self.max_scroll = lines.len().saturating_sub(self.viewport_height);
        self.scroll = if self.follow_bottom {
            self.max_scroll
        } else {
            self.scroll.min(self.max_scroll)
        };
        self.near_top = self.scroll <= self.viewport_height;
        let visible: Vec<Line> = lines
            .iter()
            .skip(self.scroll)
            .take(self.viewport_height)
            .cloned()
            .collect();
        frame.render_widget(
            Paragraph::new(visible)
                .style(theme::zone_text(focused))
                .block(block),
            area,
        );
        self.keep_selection_for(Some(Target::Transcript {
            width: text_width,
            level: self.view.level,
        }));
        self.paint_transcript_selection(frame);
    }

    pub(in crate::tui) fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let prompt = self
            .confirm
            .as_ref()
            .map(|confirm| self.confirm_prompt(confirm));
        let alert = self.alert.as_ref().map(|(text, _)| text);
        let host = self.host_unreachable.as_ref().map(ToString::to_string);
        let update = self.update_notice.map(|notice| notice.text().to_owned());
        let shown_notice = self
            .notice
            .as_ref()
            .or(alert)
            .or(host.as_ref())
            .or(update.as_ref());
        let mode = match (&prompt, shown_notice, self.focus) {
            (Some(prompt), _, _) => footer::Mode::Confirm(prompt),
            (None, Some(notice), _) => footer::Mode::Notice(notice),
            (None, None, Focus::List) if self.search.is_some() => footer::Mode::Search,
            (None, None, Focus::List) if self.tab_bar_has_focus() => footer::Mode::ListTabs,
            (None, None, Focus::List) => footer::Mode::List,
            (None, None, Focus::Conversation) => footer::Mode::Transcript,
            (None, None, Focus::Embedded) => self.embedded_footer_mode(),
        };
        let summary = activity::summarize(self.catalog.activity().values());
        let tint = match self.focus {
            Focus::Embedded => self.shown_color(),
            _ => None,
        };
        footer::draw(
            frame,
            area,
            footer::FooterView {
                mode: &mode,
                scanning: self.scanning,
                summary: &summary,
                chip: theme::focus_chip(tint),
                view: self.view.level,
                attach_available: self.attach_available(),
                mouse: self.split.mouse_enabled(),
                extras: footer::Extras {
                    hidden_note: self.hidden_note(),
                    harness: self.shown_harness().map(HarnessId::as_str),
                },
            },
        );
    }

    pub(in crate::tui) fn shown_harness(&self) -> Option<HarnessId> {
        self.shown
            .and_then(|id| self.sessions.get(id))
            .map(|session| session.harness)
    }

    pub(in crate::tui) fn embedded_footer_mode(&self) -> footer::Mode<'static> {
        let running = self
            .shown
            .and_then(|id| self.sessions.get(id))
            .is_some_and(|session| session.is_running());
        match (running, self.prefix.is_armed()) {
            (false, _) => footer::Mode::EmbeddedExited,
            (true, true) => footer::Mode::EmbeddedPrefix,
            (true, false) => footer::Mode::Embedded,
        }
    }
}
