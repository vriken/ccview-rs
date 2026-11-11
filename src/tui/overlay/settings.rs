use crate::tui::{
    app::{App, Focus},
    input::split::Resize,
    overlay::Body,
    reader::{selection::View, verbosity::Level},
    style::{identity::Key, palette, theme},
    widgets::list,
};
use crate::{
    harness::{ClaudeCode, external_terminal},
    store::settings::{TerminalApp, UiSettings, max_age_label, step_max_age},
    terminal::session_host::autostart,
};
use crossterm::event::KeyCode;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

const LABEL_COLUMN: usize = 28;
const RESIZE_STEP: i16 = 2;
const MAX_AGE_NOTE: [&str; 2] = [
    "Older sessions are not loaded at all, not just hidden.",
    "Starred, important and running sessions always load.",
];
const SWATCH: &str = "██";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    IdentityColors,
    SessionColour,
    ViewLevel,
    Mouse,
    SplitRatio,
    SplitReset,
    TerminalApp,
    Hidden,
    DisableAgentView,
    SessionsStrip,
    MaxAge,
    AutoRestart,
    Autostart,
}

const ITEMS: [Item; 13] = [
    Item::IdentityColors,
    Item::SessionColour,
    Item::ViewLevel,
    Item::Mouse,
    Item::SplitRatio,
    Item::SplitReset,
    Item::TerminalApp,
    Item::Hidden,
    Item::DisableAgentView,
    Item::SessionsStrip,
    Item::MaxAge,
    Item::AutoRestart,
    Item::Autostart,
];

#[derive(Default)]
pub(crate) struct State {
    cursor: usize,
    hidden_list: Option<usize>,
    autostart: Option<bool>,
}

struct HiddenEntry {
    session_id: String,
    title: String,
    cwd: String,
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

impl App {
    pub(in crate::tui) fn apply_loaded_settings(&mut self) {
        let settings = self.split.settings();
        self.identity.set_enabled(settings.identity_colors);
        self.harness =
            Box::new(ClaudeCode::from_env().disable_agent_view(settings.disable_agent_view));
    }

    pub(in crate::tui) fn terminal_app(&self) -> TerminalApp {
        self.split.settings().terminal_app
    }

    pub(in crate::tui) fn set_view_level(&mut self, level: Level) {
        if !self.follow_bottom {
            let line = self.current_scroll();
            if let View::Content(conversation) = self.transcript.view() {
                self.scroll_anchor = Some(conversation.turn_at_line(line));
            }
        }
        if let Err(err) = self.view.set(level) {
            self.notice = Some(format!("could not save view level: {err}"));
        }
    }

    fn update_ui(&mut self, change: impl FnOnce(&mut UiSettings)) {
        if let Err(err) = self.split.update_settings(change) {
            self.notice = Some(format!("could not save settings: {err}"));
        }
    }

    fn colour_target(&self) -> Option<Key> {
        if self.focus == Focus::Embedded
            && let Some(session) = self.shown.and_then(|id| self.sessions.get(id))
        {
            return Some(session.identity_key());
        }
        self.selected_entry()
            .map(|entry| Key::Session(entry.session_id.clone()))
    }

    fn hidden_entries(&self) -> Vec<HiddenEntry> {
        self.catalog
            .entries()
            .iter()
            .filter(|entry| self.hidden.contains(&entry.session_id))
            .map(|entry| HiddenEntry {
                session_id: entry.session_id.clone(),
                title: entry.title.clone(),
                cwd: entry.cwd.clone(),
            })
            .collect()
    }

    fn change_setting(&mut self, item: Item, delta: isize) {
        let settings = self.split.settings();
        match item {
            Item::IdentityColors => {
                let enabled = !settings.identity_colors;
                self.identity.set_enabled(enabled);
                self.update_ui(|ui| ui.identity_colors = enabled);
            }
            Item::SessionColour => {
                if let Some(key) = self.colour_target() {
                    self.sync_identity();
                    if let Err(err) = self.identity.shift(&key, delta) {
                        self.notice = Some(format!("could not save colours: {err}"));
                    }
                }
            }
            Item::ViewLevel => self.set_view_level(self.view.level.step(delta)),
            Item::Mouse => self.update_ui(|ui| ui.mouse = !settings.mouse),
            Item::SplitRatio => self.apply_resize(Resize::Delta(RESIZE_STEP * delta as i16)),
            Item::SplitReset => {}
            Item::TerminalApp => {
                if external_terminal::app_override().is_none() {
                    self.update_ui(|ui| ui.terminal_app = settings.terminal_app.step(delta));
                }
            }
            Item::Hidden | Item::Autostart => {}
            Item::MaxAge => {
                let days = step_max_age(settings.max_age_days, delta);
                self.update_ui(|ui| ui.max_age_days = days);
                self.publish_age_filter();
            }
            Item::SessionsStrip => {
                self.update_ui(|ui| ui.sessions_strip = !settings.sessions_strip)
            }
            Item::AutoRestart => {
                self.update_ui(|ui| ui.auto_restart_on_update = !settings.auto_restart_on_update)
            }
            Item::DisableAgentView => {
                let disable = !settings.disable_agent_view;
                self.harness = Box::new(ClaudeCode::from_env().disable_agent_view(disable));
                self.update_ui(|ui| ui.disable_agent_view = disable);
            }
        }
    }

    fn activate_setting(&mut self, state: &mut State, item: Item) {
        match item {
            Item::SplitReset => self.apply_resize(Resize::Reset),
            Item::SplitRatio => {}
            Item::Hidden => state.hidden_list = Some(0),
            other => self.change_setting(other, 1),
        }
    }

    pub(in crate::tui) fn settings_key(&mut self, state: &mut State, code: KeyCode) -> bool {
        if state.hidden_list.is_some() {
            return self.hidden_list_key(state, code);
        }
        let item = ITEMS[state.cursor];
        let toggles = matches!(
            code,
            KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ')
        );
        if item == Item::Autostart && toggles {
            self.toggle_autostart(state);
            return true;
        }
        match code {
            KeyCode::Esc => return false,
            KeyCode::Up => state.cursor = state.cursor.saturating_sub(1),
            KeyCode::Down => state.cursor = (state.cursor + 1).min(ITEMS.len() - 1),
            KeyCode::Left => self.change_setting(item, -1),
            KeyCode::Right => self.change_setting(item, 1),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_setting(state, item),
            _ => {}
        }
        true
    }

    fn toggle_autostart(&mut self, state: &mut State) {
        let enabled = !state.autostart.unwrap_or_else(autostart::is_enabled);
        match autostart::set_enabled(enabled) {
            Ok(()) => state.autostart = Some(enabled),
            Err(err) => self.notice = Some(format!("could not change restore after reboot: {err}")),
        }
    }

    fn hidden_list_key(&mut self, state: &mut State, code: KeyCode) -> bool {
        let entries = self.hidden_entries();
        let cursor = state
            .hidden_list
            .unwrap_or(0)
            .min(entries.len().saturating_sub(1));
        state.hidden_list = Some(match code {
            KeyCode::Esc => {
                state.hidden_list = None;
                return true;
            }
            KeyCode::Up => cursor.saturating_sub(1),
            KeyCode::Down => (cursor + 1).min(entries.len().saturating_sub(1)),
            KeyCode::Enter => {
                if let Some(entry) = entries.get(cursor) {
                    if let Err(err) = self.hidden.toggle(&entry.session_id) {
                        self.notice = Some(format!("could not save hidden sessions: {err}"));
                    }
                    self.rebuild_view();
                }
                cursor.saturating_sub(1).min(cursor)
            }
            _ => cursor,
        });
        true
    }

    fn setting_label(&self, item: Item) -> String {
        match item {
            Item::IdentityColors => "Identity colours".to_owned(),
            Item::SessionColour => "Selected session colour".to_owned(),
            Item::ViewLevel => "View level".to_owned(),
            Item::Mouse => "Mouse capture".to_owned(),
            Item::SplitRatio => "Split (list width)".to_owned(),
            Item::SplitReset => "Reset split to default".to_owned(),
            Item::TerminalApp => "Terminal app for o / O".to_owned(),
            Item::Hidden => format!("Hidden sessions ({})", self.hidden_entries().len()),
            Item::MaxAge => "Load sessions from the last".to_owned(),
            Item::DisableAgentView => "Disable Claude agent view".to_owned(),
            Item::SessionsStrip => "Sessions strip".to_owned(),
            Item::AutoRestart => "Restart on update".to_owned(),
            Item::Autostart => "Restore sessions after reboot".to_owned(),
        }
    }

    fn setting_value(&self, item: Item) -> Vec<Span<'static>> {
        let settings = self.split.settings();
        let plain = |text: String| vec![Span::raw(text)];
        match item {
            Item::IdentityColors => plain(on_off(settings.identity_colors).to_owned()),
            Item::SessionColour => match self.colour_target() {
                Some(key) => {
                    let index = self.identity.index(&key);
                    vec![
                        Span::styled(SWATCH, Style::default().fg(palette::color(index))),
                        Span::raw(format!(" {}/{}", index + 1, palette::SIZE)),
                    ]
                }
                None => plain("no session selected".to_owned()),
            },
            Item::ViewLevel => plain(self.view.level.label().to_owned()),
            Item::Mouse => plain(on_off(settings.mouse).to_owned()),
            Item::SplitRatio => plain(format!("{}%", (self.split.ratio() * 100.0).round())),
            Item::SplitReset => plain("Enter".to_owned()),
            Item::TerminalApp => match external_terminal::app_override() {
                Some(app) => plain(format!("{app} (locked by CCVIEW_TERMINAL_APP)")),
                None => plain(settings.terminal_app.label().to_owned()),
            },
            Item::Hidden => plain("Enter ›".to_owned()),
            Item::MaxAge => plain(max_age_label(settings.max_age_days)),
            Item::DisableAgentView => plain(on_off(settings.disable_agent_view).to_owned()),
            Item::SessionsStrip => plain(on_off(settings.sessions_strip).to_owned()),
            Item::AutoRestart => plain(on_off(settings.auto_restart_on_update).to_owned()),
            Item::Autostart => Vec::new(),
        }
    }

    pub(in crate::tui) fn settings_body(&self, state: &mut State, width: usize) -> Body {
        match state.hidden_list {
            Some(cursor) => self.hidden_body(state, cursor, width),
            None => {
                state.autostart.get_or_insert_with(autostart::is_enabled);
                self.items_body(state)
            }
        }
    }

    fn items_body(&self, state: &State) -> Body {
        let mut lines: Vec<Line<'static>> = ITEMS
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let selected = index == state.cursor;
                let marker = if selected { "▶ " } else { "  " };
                let mut spans = vec![Span::raw(format!(
                    "{marker}{:<LABEL_COLUMN$}",
                    self.setting_label(*item)
                ))];
                if *item == Item::Autostart {
                    spans.push(Span::raw(on_off(state.autostart.unwrap_or(false))));
                } else {
                    spans.extend(self.setting_value(*item));
                }
                let line = Line::from(spans);
                if selected {
                    line.style(theme::row_selected(true))
                } else {
                    line
                }
            })
            .collect();
        if ITEMS[state.cursor] == Item::MaxAge {
            lines.push(Line::raw(""));
            lines.extend(MAX_AGE_NOTE.map(|note| Line::raw(format!("  {note}"))));
        }
        Body {
            title: "Settings".to_owned(),
            lines,
            hint: "↑/↓ select · ←/→ change · Enter toggle · Esc close".to_owned(),
            follow: Some(state.cursor),
            fit: true,
        }
    }

    fn hidden_body(&self, state: &mut State, cursor: usize, width: usize) -> Body {
        let entries = self.hidden_entries();
        let cursor = cursor.min(entries.len().saturating_sub(1));
        state.hidden_list = Some(cursor);
        let lines = if entries.is_empty() {
            vec![Line::raw("  No hidden sessions.")]
        } else {
            entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let selected = index == cursor;
                    let marker = if selected { "▶ " } else { "  " };
                    let directory = list::truncate(&entry.cwd, width / 2);
                    let title_width =
                        width.saturating_sub(marker.len() + directory.chars().count() + 3);
                    let line = Line::raw(format!(
                        "{marker}{} · {directory}",
                        list::truncate(&entry.title, title_width)
                    ));
                    if selected {
                        line.style(theme::row_selected(true))
                    } else {
                        line
                    }
                })
                .collect()
        };
        Body {
            title: "Settings › Hidden sessions".to_owned(),
            lines,
            hint: "↑/↓ select · Enter show again · Esc back".to_owned(),
            follow: Some(cursor),
            fit: true,
        }
    }
}
