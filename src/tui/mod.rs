mod back_to_list;
mod badges;
mod colors;
mod conversation;
mod diff;
mod embedded;
mod follow;
mod footer;
mod hide;
mod identity;
mod launch;
mod list;
mod mouse;
mod nav;
mod outer_terminal;
mod palette;
mod prefix;
mod runtime;
mod sessions;
mod split;
mod tabs;
mod terminal_events;
mod theme;
mod timefmt;
mod tool_block;
mod transcript;
mod verbosity;

use crate::{
    activity,
    discover, external,
    favorites::Favorites,
    hidden::HiddenSessions,
    harness::{self, Harness, LaunchKind},
    library::{self, Library, SessionEntry, Update},
    sources::HarnessId,
};
use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
};
use follow::Follower;
use identity::Identity;
use launch::{PendingLaunch, Target};
use list::ListView;
use nav::ListFocus;
use prefix::Prefix;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Padding, Paragraph},
};
pub use runtime::{Message, event_loop};
use sessions::SessionManager;
use split::{Resize, SplitState};
use crate::ui_settings::UiSettingsStore;
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Instant, SystemTime},
};
use tabs::{Tab, TabKey};
use transcript::{Transcript, View};
use verbosity::ViewSetting;

const LOADING_MARK: &str = " · loading";
const PAGE_SCROLL_OVERLAP: usize = 2;
const GUTTER: &str = "▌ ";
const GUTTER_WIDTH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Conversation,
    Embedded,
}

enum Confirm {
    Duplicate(PendingLaunch),
    Close(u64),
    Quit,
}

pub struct App {
    library: Library,
    favorites: Favorites,
    hidden: HiddenSessions,
    show_hidden: bool,
    list_focus: ListFocus,
    identity: Identity,
    updates: Option<Receiver<Update>>,
    scanning: bool,
    tabs: Vec<Tab>,
    active: TabKey,
    rows: Vec<usize>,
    selected_path: Option<PathBuf>,
    focus: Focus,
    transcript: Transcript,
    scroll: usize,
    max_scroll: usize,
    follow_bottom: bool,
    viewport_height: usize,
    notice: Option<String>,
    should_quit: bool,
    harness: Box<dyn Harness>,
    sessions: SessionManager,
    shown: Option<u64>,
    prefix: Prefix,
    confirm: Option<Confirm>,
    pane_inner: Rect,
    view: ViewSetting,
    scroll_anchor: Option<usize>,
    split: SplitState,
    regions: mouse::Regions,
    terminal_focused: bool,
    focused_child: Option<u64>,
    outer_output: Vec<u8>,
    alert: Option<(String, Instant)>,
    ignore_ctrl_c_until: Option<Instant>,
    forwarded_presses: Vec<KeyCode>,
}

pub fn run() -> io::Result<()> {
    let projects_dir =
        discover::claude_projects_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
    external::remove_stale_scripts();
    let (sender, messages) = mpsc::channel();
    let wake = sender.clone();
    let updates = library::spawn(projects_dir, move || {
        let _ = wake.send(Message::Updated);
    });
    let mut app = App::new(
        Favorites::load_default(),
        Identity::load_default(),
        Some(updates),
        sender.clone(),
    )
    .with_ui_settings(UiSettingsStore::load_default())
    .with_hidden(HiddenSessions::load_default());
    app.view = ViewSetting::load_default();
    let mut terminal = ratatui::init();
    let capabilities = outer_terminal::detect();
    outer_terminal::install_panic_hook(capabilities);
    outer_terminal::enter(&mut io::stdout(), capabilities)?;
    if !capabilities.keyboard_enhancement {
        app.notice = Some("shift+enter sends plain enter here (terminal lacks kitty keyboard protocol)".to_owned());
    }
    runtime::spawn_input_reader(sender);
    let result = runtime::event_loop(
        &mut app,
        &messages,
        |app| terminal.draw(|frame| app.draw(frame)).map(|_| ()),
        runtime::set_mouse_capture,
        runtime::write_outer,
    );
    let _ = outer_terminal::leave(&mut io::stdout(), capabilities);
    ratatui::restore();
    drop(app);
    result
}

impl App {
    pub fn new(favorites: Favorites, identity: Identity, updates: Option<Receiver<Update>>, wake: Sender<Message>) -> Self {
        let mut app = Self {
            library: Library::default(),
            favorites,
            hidden: HiddenSessions::default(),
            show_hidden: false,
            list_focus: ListFocus::default(),
            identity,
            updates,
            scanning: true,
            tabs: Vec::new(),
            active: TabKey::All,
            rows: Vec::new(),
            selected_path: None,
            focus: Focus::List,
            transcript: Transcript::default(),
            scroll: 0,
            max_scroll: 0,
            follow_bottom: true,
            viewport_height: 0,
            notice: None,
            should_quit: false,
            harness: harness::default_harness(),
            sessions: SessionManager::new(wake),
            shown: None,
            prefix: Prefix::default(),
            confirm: None,
            pane_inner: Rect::default(),
            view: ViewSetting::default(),
            scroll_anchor: None,
            split: SplitState::new(UiSettingsStore::default()),
            regions: mouse::Regions::default(),
            terminal_focused: true,
            focused_child: None,
            outer_output: Vec::new(),
            alert: None,
            ignore_ctrl_c_until: None,
            forwarded_presses: Vec::new(),
        };
        app.drain_updates();
        app.rebuild_view();
        app
    }

    pub fn with_ui_settings(mut self, store: UiSettingsStore) -> Self {
        self.split = SplitState::new(store);
        self
    }

    pub fn with_hidden(mut self, hidden: HiddenSessions) -> Self {
        self.hidden = hidden;
        self.rebuild_view();
        self
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn output_ready(&mut self, id: u64) -> bool {
        let newly_exited = self.sessions.acknowledge_output(id);
        let events_changed = self.drain_terminal_events(id);
        let reaped = self.reap_clean_exits();
        reaped || newly_exited || events_changed || self.shown == Some(id)
    }

    pub fn handle_event(&mut self, event: Event) -> bool {
        let redraw = match event {
            Event::Key(key) => self.handle_key(key),
            Event::Paste(text) => {
                self.handle_paste(&text);
                false
            }
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::FocusGained => {
                self.terminal_focused = true;
                false
            }
            Event::FocusLost => {
                self.terminal_focused = false;
                false
            }
            Event::Resize(..) => true,
        };
        self.sync_child_focus();
        redraw
    }

    pub fn apply(&mut self, update: Update) {
        self.absorb(update);
        self.rebuild_view();
    }

    fn absorb(&mut self, update: Update) {
        match update {
            Update::Scanned(entries) => self.library.merge(entries),
            Update::Removed(paths) => self.library.remove(&paths),
            Update::Live(live) => {
                self.sync_identity();
                for (host, session_id) in self.sessions.bind_live(&live) {
                    self.identity.adopt(host, &session_id);
                }
                self.library.set_live(live);
            }
            Update::Activity(activity) => self.library.set_activity(activity),
            Update::InitialScanDone => self.scanning = false,
        }
    }

    pub fn selected_entry(&self) -> Option<&SessionEntry> {
        let path = self.selected_path.as_ref()?;
        self.library.entries().iter().find(|entry| &entry.path == path)
    }

    pub fn tick(&mut self) -> bool {
        let mut changed = self.reap_clean_exits();
        changed |= self.drain_updates();
        changed |= self.drain_conversation();
        changed |= self.expire_alert();
        changed
    }

    fn drain_updates(&mut self) -> bool {
        let Some(receiver) = &self.updates else {
            return false;
        };
        let pending: Vec<Update> = receiver.try_iter().collect();
        if pending.is_empty() {
            return false;
        }
        for update in pending {
            self.absorb(update);
        }
        self.rebuild_view();
        true
    }

    fn drain_conversation(&mut self) -> bool {
        let entries = self.library.entries();
        let changed = self.transcript.tick(Instant::now(), &mut |key| {
            let entry = entries.iter().find(|entry| &entry.path == key)?;
            Some(Follower::start(entry.harness, entry.handle.clone()))
        });
        self.follow_content_switch();
        changed
    }

    fn follow_content_switch(&mut self) {
        if self.transcript.take_content_switched() {
            self.scroll = 0;
            self.follow_bottom = true;
        }
    }

    fn rebuild_view(&mut self) {
        let previous_row = self.selected_row();
        let previous_tab = self.active.clone();
        self.tabs = tabs::build_tabs(self.library.entries(), self.library.activity(), &|entry| self.is_listed(entry));
        if !self.tabs.iter().any(|tab| tab.key == self.active) {
            self.active = TabKey::All;
        }
        self.refresh_rows();
        let still_present = self
            .selected_path
            .as_ref()
            .is_some_and(|path| self.rows.iter().any(|&i| &self.library.entries()[i].path == path));
        if !still_present {
            let neighbour = previous_row.filter(|_| self.active == previous_tab);
            let target = neighbour.unwrap_or(0).min(self.rows.len().saturating_sub(1));
            self.select_row(target);
        }
    }

    fn refresh_rows(&mut self) {
        self.rows = tabs::session_indices(
            &self.active,
            self.library.entries(),
            &self.favorites,
            self.library.activity(),
            &|entry| self.is_listed(entry),
        );
    }

    fn selected_row(&self) -> Option<usize> {
        let path = self.selected_path.as_ref()?;
        self.rows
            .iter()
            .position(|&i| &self.library.entries()[i].path == path)
    }

    fn select_row(&mut self, row: usize) {
        let path = self
            .rows
            .get(row)
            .map(|&i| self.library.entries()[i].path.clone());
        self.select_path(path);
    }

    fn select_path(&mut self, path: Option<PathBuf>) {
        if self.selected_path == path {
            return;
        }
        self.selected_path = path.clone();
        if self.focus != Focus::Embedded {
            self.shown = path
                .as_ref()
                .and_then(|path| self.library.entries().iter().find(|entry| &entry.path == path))
                .and_then(|entry| self.sessions.for_session_id(&entry.session_id));
            if self.shown.is_some() && self.focus == Focus::Conversation {
                self.focus = Focus::Embedded;
            }
        }
        self.transcript.select(path, Instant::now());
        self.follow_content_switch();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return self.forward_release(key);
        }
        if self.focus != Focus::List {
            self.list_focus = ListFocus::Rows;
        }
        if let Some(confirm) = self.confirm.take() {
            return self.answer_confirm(confirm, key);
        }
        if self.focus == Focus::Embedded {
            return self.handle_embedded_key(key);
        }
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.ignore_ctrl_c_until.is_some_and(|until| Instant::now() < until) {
                return false;
            }
            self.request_quit();
            return true;
        }
        if let Some(resize) = Resize::from_key(key) {
            self.apply_resize(resize);
            return true;
        }
        match key.code {
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('r') => self.request_launch(LaunchKind::Resume, Target::Embedded),
            KeyCode::Char('f') => self.request_launch(LaunchKind::Fork, Target::Embedded),
            KeyCode::Char('n') => self.request_launch(LaunchKind::New, Target::Embedded),
            KeyCode::Char('c') => self.request_launch(LaunchKind::ContinueLatest, Target::Embedded),
            KeyCode::Char('a') => self.request_launch(LaunchKind::Attach, Target::Embedded),
            KeyCode::Char('o') => self.request_launch(LaunchKind::Resume, Target::ExternalTerminal),
            KeyCode::Char('O') => self.request_launch(LaunchKind::New, Target::ExternalTerminal),
            KeyCode::Char('s' | ' ') => self.toggle_star(),
            KeyCode::Char('C') => self.cycle_selected_color(),
            KeyCode::Char('v') => self.cycle_view(),
            KeyCode::PageDown => self.scroll_down(self.page_size()),
            KeyCode::PageUp => self.scroll_up(self.page_size()),
            KeyCode::Char('g') => {
                self.scroll = 0;
                self.follow_bottom = false;
            }
            KeyCode::Char('G') => self.follow_bottom = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            code if self.focus == Focus::List => self.handle_list_only_key(code),
            code => self.handle_transcript_only_key(code),
        }
        true
    }

    fn cycle_view(&mut self) {
        if !self.follow_bottom {
            let line = self.current_scroll();
            if let View::Content(conversation) = self.transcript.view() {
                self.scroll_anchor = Some(conversation.turn_at_line(line));
            }
        }
        if let Err(err) = self.view.cycle() {
            self.notice = Some(format!("could not save view level: {err}"));
        }
    }

    fn handle_transcript_only_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc | KeyCode::Tab => {
                self.focus = Focus::List
            }
            _ => {}
        }
    }

    fn reading_focus(&self) -> Focus {
        if self.shown.is_some() {
            Focus::Embedded
        } else {
            Focus::Conversation
        }
    }

    fn page_size(&self) -> usize {
        self.viewport_height.saturating_sub(PAGE_SCROLL_OVERLAP).max(1)
    }

    fn current_scroll(&self) -> usize {
        if self.follow_bottom {
            self.max_scroll
        } else {
            self.scroll.min(self.max_scroll)
        }
    }

    fn scroll_down(&mut self, amount: usize) {
        let target = self.current_scroll().saturating_add(amount);
        self.follow_bottom = target >= self.max_scroll;
        self.scroll = target.min(self.max_scroll);
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.current_scroll().saturating_sub(amount);
        self.follow_bottom = false;
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Conversation | Focus::Embedded => self.scroll_down(1),
            Focus::List => self.list_down(),
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Conversation | Focus::Embedded => self.scroll_up(1),
            Focus::List => self.list_up(),
        }
    }

    fn apply_resize(&mut self, resize: Resize) {
        if let Err(err) = self.split.resize(resize) {
            self.notice = Some(format!("could not save layout: {err}"));
        }
    }

    fn switch_tab(&mut self, step: isize) {
        let count = self.tabs.len() as isize;
        let current = self
            .tabs
            .iter()
            .position(|tab| tab.key == self.active)
            .unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(count) as usize;
        self.select_tab(next);
    }

    fn select_tab(&mut self, next: usize) {
        self.active = self.tabs[next].key.clone();
        self.refresh_rows();
        let first = self.rows.first().map(|&i| self.library.entries()[i].path.clone());
        self.select_path(first);
    }

    fn toggle_star(&mut self) {
        let Some(session_id) = self.selected_entry().map(|entry| entry.session_id.clone()) else {
            return;
        };
        if let Err(err) = self.favorites.toggle(&session_id) {
            self.notice = Some(format!("could not save favorites: {err}"));
        }
        let previous = self.selected_row();
        self.refresh_rows();
        if self.selected_row().is_none() {
            let row = previous.unwrap_or(0).min(self.rows.len().saturating_sub(1));
            self.select_row(row);
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        self.reap_clean_exits();
        self.sync_identity();
        let strip_height = u16::from(!self.sessions.is_empty());
        let [strip_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(strip_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let panes = self.split.layout(body_area);
        let (list_area, right_area) = (panes.list, panes.detail);
        if strip_height > 0 {
            embedded::draw_strip(
                frame,
                strip_area,
                self.sessions.all(),
                self.shown,
                self.focus == Focus::Embedded,
                self.library.activity(),
                &self.identity,
            );
        }
        self.pane_inner = embedded::pane_block(None).inner(right_area);
        if self.split.pane_resize_due() {
            self.sessions.resize_all(embedded::inner_size(right_area));
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
                let style = embedded::PaneStyle {
                    focused: self.focus == Focus::Embedded,
                    tint: Some(self.identity.color(&session.identity_key())),
                };
                embedded::draw_pane(frame, right_area, session, style)
            }
            None => self.draw_conversation(frame, right_area),
        }
        self.draw_footer(frame, footer_area);
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect) {
        let entries = self.library.entries();
        let rows: Vec<&SessionEntry> = self.rows.iter().map(|&i| &entries[i]).collect();
        let label = self
            .tabs
            .iter()
            .find(|tab| tab.key == self.active)
            .map_or("", |tab| tab.label.as_str());
        let empty_message = match (&self.active, self.scanning) {
            (TabKey::Favorites, _) => "No favorites yet.\n\nSelect a session and press s to star it.",
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
                hidden: &self.hidden,
                activity: self.library.activity(),
                now: SystemTime::now(),
            },
        );
    }

    fn draw_conversation(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Conversation;
        let loading_mark = if self.transcript.is_stale() { LOADING_MARK } else { "" };
        let title = self
            .selected_entry()
            .map(|entry| {
                let harness = entry.harness.as_str();
                let room = (area.width.saturating_sub(9) as usize)
                    .saturating_sub(loading_mark.chars().count() + harness.len() + 3);
                format!("{harness} · {}{loading_mark}", list::truncate(&entry.title, room))
            })
            .unwrap_or_else(|| "Conversation".to_owned());
        let gutter_color = self.selected_color();
        let style = embedded::PaneStyle { focused, tint: None };
        let block = embedded::pane_block(Some((&format!("TRANSCRIPT · {title}"), style)))
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        self.viewport_height = inner.height as usize;
        let conversation = match self.transcript.view() {
            View::Idle => {
                let hint = "Select a session to read it here";
                frame.render_widget(Paragraph::new(hint).style(theme::empty_state()).block(block), area);
                return;
            }
            View::Loading => {
                frame.render_widget(Paragraph::new("Loading…").style(theme::empty_state()).block(block), area);
                return;
            }
            View::Error(error) => {
                frame.render_widget(Paragraph::new(error.to_owned()).style(theme::notice()).block(block), area);
                return;
            }
            View::Content(conversation) => conversation,
        };
        let text_width = (inner.width as usize).saturating_sub(GUTTER_WIDTH);
        if let Some(turn) = self.scroll_anchor.take() {
            conversation.lines(text_width, self.view.level);
            self.scroll = conversation.first_line_of_turn(turn);
        }
        let lines = conversation.lines(text_width, self.view.level);
        self.max_scroll = lines.len().saturating_sub(self.viewport_height);
        self.scroll = if self.follow_bottom {
            self.max_scroll
        } else {
            self.scroll.min(self.max_scroll)
        };
        let gutter = Span::styled(
            GUTTER,
            gutter_color.map_or_else(Style::default, theme::identity_bar),
        );
        let visible: Vec<Line> = lines
            .iter()
            .skip(self.scroll)
            .take(self.viewport_height)
            .map(|line| {
                let mut line = line.clone();
                line.spans.insert(0, gutter.clone());
                line
            })
            .collect();
        frame.render_widget(
            Paragraph::new(visible).style(theme::zone_text(focused)).block(block),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let prompt = self.confirm.as_ref().map(|confirm| self.confirm_prompt(confirm));
        let alert = self.alert.as_ref().map(|(text, _)| text);
        let mode = match (&prompt, self.notice.as_ref().or(alert), self.focus) {
            (Some(prompt), _, _) => footer::Mode::Confirm(prompt),
            (None, Some(notice), _) => footer::Mode::Notice(notice),
            (None, None, Focus::List) if self.tab_bar_has_focus() => footer::Mode::ListTabs,
            (None, None, Focus::List) => footer::Mode::List,
            (None, None, Focus::Conversation) => footer::Mode::Transcript,
            (None, None, Focus::Embedded) => self.embedded_footer_mode(),
        };
        let summary = activity::summarize(self.library.activity().values());
        let tint = match self.focus {
            Focus::Embedded => self.shown_color(),
            _ => None,
        };
        footer::draw(
            frame,
            area,
            &mode,
            self.scanning,
            &summary,
            theme::focus_chip(tint),
            self.view.level,
            self.attach_available(),
            self.split.mouse_enabled(),
            footer::Extras {
                hidden_note: self.hidden_note(),
                harness: self.shown_harness().map(HarnessId::as_str),
            },
        );
    }

    fn shown_harness(&self) -> Option<HarnessId> {
        self.shown.and_then(|id| self.sessions.get(id)).map(|session| session.harness)
    }

    fn embedded_footer_mode(&self) -> footer::Mode<'static> {
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

#[cfg(test)]
mod focus_tests;
#[cfg(test)]
mod harness_tag_tests;
#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{library::TitleSource, terminal::{CommandSpec, Size}};
    use sessions::Launched;
    use std::{path::PathBuf, time::{Duration, UNIX_EPOCH}};

    fn entry(id: &str, cwd: &str) -> SessionEntry {
        SessionEntry {
            path: PathBuf::from(format!("/p/{id}.jsonl")),
            session_id: id.to_owned(),
            cwd: cwd.to_owned(),
            title: id.to_owned(),
            title_source: TitleSource::Prompt,
            last_prompt: None,
            last_used: UNIX_EPOCH,
            live: None,
            ..SessionEntry::stub()
        }
    }

    fn app() -> App {
        let mut app = App::new(
            Favorites::load(std::env::temp_dir().join(format!("ccview-keys-{}/f.json", std::process::id()))),
            Identity::load(std::env::temp_dir().join(format!("ccview-keys-{}/c.json", std::process::id()))),
            None,
            mpsc::channel().0,
        );
        app.apply(Update::Scanned(vec![entry("a", "/w/one"), entry("b", "/w/two")]));
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_char(app: &mut App, c: char) {
        press(app, KeyCode::Char(c));
    }

    #[test]
    fn entries_queued_before_the_app_exists_are_on_the_first_frame() {
        let (sender, receiver) = mpsc::channel();
        sender.send(Update::Scanned(vec![entry("cached-session", "/w/one")])).unwrap();
        let mut app = App::new(
            Favorites::load(std::env::temp_dir().join(format!("ccview-first-{}/f.json", std::process::id()))),
            Identity::load(std::env::temp_dir().join(format!("ccview-first-{}/c.json", std::process::id()))),
            Some(receiver),
            mpsc::channel().0,
        );
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen: String = terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("cached-session"));
    }

    #[test]
    fn tabs_switch_from_the_tab_bar_only_and_the_transcript_ignores_right() {
        let mut app = app();
        press(&mut app, KeyCode::Right);
        assert_eq!((app.active.clone(), app.focus), (TabKey::All, Focus::Conversation));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.focus, Focus::Conversation);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Focus::List);

        press(&mut app, KeyCode::Up);
        for code in [KeyCode::Right, KeyCode::Char('l'), KeyCode::Char(']')] {
            press(&mut app, code);
        }
        assert_eq!(app.active, TabKey::Dir("/w/one".to_owned()));
        for code in [KeyCode::Left, KeyCode::Char('h'), KeyCode::Char('[')] {
            press(&mut app, code);
        }
        assert_eq!(app.active, TabKey::All);
        press(&mut app, KeyCode::Down);
        for code in [KeyCode::Char('h'), KeyCode::Char('['), KeyCode::Char(']'), KeyCode::Left] {
            press(&mut app, code);
            assert_eq!((app.active.clone(), app.focus), (TabKey::All, Focus::List), "{code:?}");
        }
    }

    #[test]
    fn embedded_session_receives_every_navigation_key() {
        let mut app = app();
        let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
        let launched = Launched { title: "t".to_owned(), session_id: None, harness: HarnessId::Claude };
        let id = app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
        app.show_embedded(id);
        for code in [KeyCode::Right, KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('l'), KeyCode::Tab] {
            press(&mut app, code);
            assert_eq!((app.focus, app.active.clone(), app.should_quit), (Focus::Embedded, TabKey::All, false), "{code:?}");
        }
    }

    fn activity_with(state: crate::activity::ActivityState, background: bool, agents: usize) -> crate::activity::Activity {
        crate::activity::Activity {
            state,
            background,
            attach_id: background.then(|| "bg1".to_owned()),
            agents: (0..agents)
                .map(|_| crate::subagents::RunningAgent { agent_type: "explore".into(), description: String::new() })
                .collect(),
            last_activity: None,
        }
    }

    fn wait_for_screen(app: &App, id: u64, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.sessions.get(id).unwrap().host().snapshot().text().contains(needle) {
            assert!(Instant::now() < deadline, "never saw {needle:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn attach_runs_claude_attach_with_the_background_id_and_only_for_background_sessions() {
        use crate::activity::ActivityState;
        let dir = std::env::temp_dir().join(format!("ccview-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-claude");
        std::fs::write(&fake, "#!/bin/sh\necho \"ran: $@\"\nexec sleep 60\n").unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&fake).status().unwrap();

        let mut app = app();
        app.harness = Box::new(harness::ClaudeCode::with_binary(fake.to_str().unwrap()));
        let cwd = dir.to_str().unwrap();
        app.apply(Update::Scanned(vec![entry("bgsession", cwd)]));
        let mut activity = crate::activity::ActivityMap::new();
        activity.insert("bgsession".to_owned(), activity_with(ActivityState::Blocked, true, 0));
        activity.insert("a".to_owned(), activity_with(ActivityState::Busy, false, 0));
        app.apply(Update::Activity(activity));

        app.select_path(Some(PathBuf::from("/p/a.jsonl")));
        type_char(&mut app, 'a');
        assert!(app.sessions.is_empty());
        assert_eq!(app.notice.as_deref(), Some("not a background session"));

        app.select_path(Some(PathBuf::from("/p/bgsession.jsonl")));
        type_char(&mut app, 'a');
        let id = app.shown.expect("attach is shown embedded");
        wait_for_screen(&app, id, "ran: attach bg1");
        assert_eq!(app.focus, Focus::Embedded);
        app.sessions.remove(id);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_tab_badges_and_summary_render_at_160x35() {
        use crate::activity::ActivityState;
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = app();
        let mut activity = crate::activity::ActivityMap::new();
        activity.insert("a".to_owned(), activity_with(ActivityState::Blocked, true, 0));
        activity.insert("b".to_owned(), activity_with(ActivityState::Busy, false, 3));
        app.apply(Update::Activity(activity));
        app.switch_tab(2);
        assert_eq!(app.active, TabKey::Active);
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.library.entries()[app.rows[0]].session_id, "a");

        let mut terminal = Terminal::new(TestBackend::new(160, 35)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen: Vec<String> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(160)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let text = screen.join("\n");
        for expected in ["Active 2", "needs you", "⛭ 3", "bg", "1 busy", "1 needs you", "3 agents running"] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }
    fn script_child(dir: &std::path::Path, name: &str, body: &str) -> CommandSpec {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        CommandSpec::new("sh").args([path.to_str().unwrap()])
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccview-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app_with_wake() -> (App, Receiver<Message>) {
        let (sender, receiver) = mpsc::channel();
        let mut app = App::new(
            Favorites::load(std::env::temp_dir().join(format!("ccview-wake-{}/f.json", std::process::id()))),
            Identity::load(std::env::temp_dir().join(format!("ccview-wake-{}/c.json", std::process::id()))),
            None,
            sender,
        );
        app.apply(Update::Scanned(vec![entry("a", "/w/one"), entry("b", "/w/two")]));
        (app, receiver)
    }

    fn pump_until(app: &mut App, receiver: &Receiver<Message>, mut condition: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition(app) {
            assert!(Instant::now() < deadline, "condition not met in time");
            if let Ok(Message::Output(id)) = receiver.recv_timeout(Duration::from_millis(20)) {
                app.output_ready(id);
            }
            app.tick();
        }
    }

    fn launch_child(app: &mut App, spec: &CommandSpec) -> u64 {
        let launched = Launched { title: "child".to_owned(), session_id: None, harness: HarnessId::Claude };
        let id = app.sessions.launch(spec, launched, Size { rows: 10, cols: 40 }).unwrap();
        app.show_embedded(id);
        id
    }

    fn read_file(path: &std::path::Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_default()
    }

    fn wait_for_bytes(path: &std::path::Path, needle: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !read_file(path).windows(needle.len()).any(|w| w == needle) {
            assert!(Instant::now() < deadline, "never saw {needle:?} in {:?}", read_file(path));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn recording_child(dir: &std::path::Path, prelude: &str) -> (CommandSpec, PathBuf) {
        let out = dir.join("stdin.bin");
        let body = format!("stty raw echo; {prelude}; exec cat > {}", out.display());
        (script_child(dir, "child.sh", &body), out)
    }

    #[test]
    fn left_at_an_empty_prompt_returns_to_the_list_without_reaching_the_child() {
        let dir = temp_dir("left-empty");
        let (spec, out) = recording_child(&dir, "printf '│ > '");
        let (mut app, receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        wait_for_screen(&app, id, "│ >");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Focus::List);
        assert!(app.sessions.get(id).unwrap().is_running());
        app.show_embedded(id);
        type_char(&mut app, 'z');
        wait_for_bytes(&out, b"z");
        assert_eq!(read_file(&out), b"z");
        drop(receiver);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn left_after_typed_text_or_with_a_hidden_cursor_goes_to_the_child() {
        let dir = temp_dir("left-typed");
        let (spec, out) = recording_child(&dir, "printf '> '");
        let (mut app, _receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        wait_for_screen(&app, id, ">");
        for c in "abc".chars() {
            type_char(&mut app, c);
        }
        wait_for_screen(&app, id, "> abc");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Focus::Embedded);
        wait_for_bytes(&out, b"abc\x1b[D");
        drop(app);

        let hidden_dir = temp_dir("left-hidden");
        let (spec, out) = recording_child(&hidden_dir, "printf '\\033[?25l> '");
        let (mut app, _receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        wait_for_screen(&app, id, ">");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Focus::Embedded);
        wait_for_bytes(&out, b"\x1b[D");
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(hidden_dir).unwrap();
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    #[test]
    fn clean_exit_returns_to_the_list_drops_the_host_and_swallows_repeated_ctrl_c() {
        let dir = temp_dir("exit-zero");
        let spec = script_child(&dir, "child.sh", "stty raw -echo; dd bs=1 count=2 of=/dev/null 2>/dev/null; exit 0");
        let (mut app, receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        let pid = app.sessions.get(id).unwrap().host().process_id().unwrap();
        wait_for_screen(&app, id, "");
        std::thread::sleep(Duration::from_millis(200));
        app.handle_key(ctrl_c());
        app.handle_key(ctrl_c());
        pump_until(&mut app, &receiver, |app| app.sessions.is_empty());
        assert_eq!((app.focus, app.shown, app.should_quit), (Focus::List, None, false));
        assert!(app.alert.as_ref().unwrap().0.starts_with("session ended"));
        app.handle_key(ctrl_c());
        assert!(!app.should_quit);
        wait_until_dead(pid);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn wait_until_dead(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::registry::pid_alive(pid) {
            assert!(Instant::now() < deadline, "pid {pid} still alive");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn failing_exit_keeps_the_pane_and_a_later_ctrl_c_only_dismisses_it() {
        let dir = temp_dir("exit-three");
        let spec = script_child(&dir, "child.sh", "stty raw -echo; dd bs=1 count=2 of=/dev/null 2>/dev/null; exit 3");
        let (mut app, receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        std::thread::sleep(Duration::from_millis(200));
        app.handle_key(ctrl_c());
        app.handle_key(ctrl_c());
        pump_until(&mut app, &receiver, |app| app.sessions.get(id).is_some_and(|s| s.exit_status().is_some()));
        assert_eq!(app.sessions.get(id).unwrap().exit_status().unwrap().code, 3);
        assert_eq!((app.focus, app.shown), (Focus::Embedded, Some(id)));
        app.handle_key(ctrl_c());
        assert!(app.sessions.is_empty());
        assert!(!app.should_quit);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn child_events_reach_the_outer_terminal_and_the_strip() {
        let dir = temp_dir("events");
        let spec = script_child(
            &dir,
            "child.sh",
            "printf '\\033]0;my title\\007\\033]52;c;aGVsbG8=\\007\\033]9;build done\\007'; exec sleep 30",
        );
        let (mut app, receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        pump_until(&mut app, &receiver, |app| !app.outer_output.is_empty() && app.alert.is_some());
        let outer = String::from_utf8_lossy(&app.outer_output).into_owned();
        assert!(outer.contains("\x1b]52;c;aGVsbG8=\x1b\\"), "{outer:?}");
        assert!(outer.contains('\x07'));
        assert_eq!(app.sessions.get(id).unwrap().display_title(), "my title");
        assert!(app.alert.as_ref().unwrap().0.contains("build done"));
        assert!(!app.take_outer_output().is_empty());
        assert!(app.take_outer_output().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn focus_changes_reach_a_child_that_enabled_focus_reporting() {
        let dir = temp_dir("focus");
        let (spec, out) = recording_child(&dir, "printf '\\033[?1004h'");
        let (mut app, _receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.sessions.get(id).unwrap().host().focus_reporting() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        app.handle_event(Event::FocusGained);
        wait_for_bytes(&out, b"\x1b[I");
        app.handle_event(Event::FocusLost);
        wait_for_bytes(&out, b"\x1b[I\x1b[O");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shift_enter_is_csi_u_for_kitty_children_and_plain_otherwise() {
        let dir = temp_dir("shift-enter");
        let (spec, out) = recording_child(&dir, "printf '\\033[>5u'");
        let (mut app, _receiver) = app_with_wake();
        let id = launch_child(&mut app, &spec);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.sessions.get(id).unwrap().host().keyboard_flags().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        wait_for_bytes(&out, b"\x1b[13;2u\r");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn drawn(app: &mut App, width: u16, height: u16) -> ratatui::Terminal<ratatui::backend::TestBackend> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
    }

    fn screen_lines(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> Vec<String> {
        let width = terminal.backend().buffer().area.width as usize;
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    #[test]
    fn embedded_footer_leads_with_the_way_back_and_fits_60_columns() {
        let dir = temp_dir("footer");
        let spec = script_child(&dir, "child.sh", "exec sleep 30");
        let mut app = app();
        launch_child(&mut app, &spec);
        for (width, expected, absent) in [
            (120, vec!["← back to list (at start of prompt)", "ctrl+space then x close · [ ] switch · l list · < > resize"], vec!["drag divider"]),
            (60, vec!["← back to list", "ctrl+space x close"], vec!["switch"]),
        ] {
            let terminal = drawn(&mut app, width, 20);
            let footer = screen_lines(&terminal).pop().unwrap();
            for text in expected {
                assert!(footer.contains(text), "{width}: {text:?} not in {footer:?}");
            }
            for text in absent {
                assert!(!footer.contains(text), "{width}: {text:?} in {footer:?}");
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn list_width_now(app: &App) -> u16 {
        app.regions.list.width
    }

    #[test]
    fn angle_keys_resize_in_list_and_transcript_but_belong_to_the_child_when_embedded() {
        let mut app = app();
        drawn(&mut app, 120, 30);
        let initial = list_width_now(&app);
        type_char(&mut app, '>');
        drawn(&mut app, 120, 30);
        assert_eq!(list_width_now(&app), initial + 2);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Conversation);
        type_char(&mut app, '<');
        type_char(&mut app, '<');
        drawn(&mut app, 120, 30);
        assert_eq!(list_width_now(&app), initial - 2);
        type_char(&mut app, '=');
        drawn(&mut app, 120, 30);
        assert_eq!(list_width_now(&app), initial);

        let dir = temp_dir("angle");
        let (spec, out) = recording_child(&dir, "true");
        let id = launch_child(&mut app, &spec);
        type_char(&mut app, '>');
        wait_for_bytes(&out, b">");
        drawn(&mut app, 120, 30);
        assert_eq!(list_width_now(&app), initial);
        app.handle_key(prefix::PREFIX);
        type_char(&mut app, '>');
        drawn(&mut app, 120, 30);
        assert_eq!(list_width_now(&app), initial + 2);
        assert_eq!(read_file(&out), b">");
        app.sessions.remove(id);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn click(app: &mut App, column: u16, row: u16) {
        app.handle_event(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    #[test]
    fn clicking_a_list_row_selects_it_and_wheel_over_the_list_moves_the_selection() {
        let mut app = app();
        app.apply(Update::Scanned((0..12).map(|n| entry(&format!("s{n:02}"), "/w/many")).collect()));
        app.select_row(8);
        let terminal = drawn(&mut app, 120, 30);
        let lines = screen_lines(&terminal);
        let target = app.rows.iter().position(|&i| app.library.entries()[i].session_id == "s03").unwrap();
        let screen_row = lines.iter().position(|line| line.contains("s03")).expect("s03 visible") as u16;
        click(&mut app, 5, screen_row);
        assert_eq!(app.selected_row(), Some(target));
        let before = app.selected_row().unwrap();
        app.handle_event(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollDown,
            column: 5,
            row: screen_row,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(app.selected_row(), Some(before + 1));
    }

    #[test]
    fn clicking_a_tab_selects_it() {
        let mut app = app();
        let terminal = drawn(&mut app, 120, 30);
        let lines = screen_lines(&terminal);
        let tab_row = lines.iter().position(|line| line.contains("Favorites")).unwrap();
        let column = lines[tab_row].find("Favorites").unwrap() as u16 + 1;
        click(&mut app, column, tab_row as u16);
        assert_eq!(app.active, TabKey::Favorites);
    }

    #[test]
    fn dragging_the_divider_resizes_and_a_new_app_restores_the_saved_ratio() {
        let path = temp_dir("drag-persist").join("ui.json");
        let mut app = app().with_ui_settings(UiSettingsStore::load(path.clone()));
        drawn(&mut app, 120, 30);
        let divider = app.regions.divider.x;
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mouse = |kind, column| Event::Mouse(MouseEvent { kind, column, row: 10, modifiers: KeyModifiers::NONE });
        app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), divider));
        app.handle_event(mouse(MouseEventKind::Drag(MouseButton::Left), 60));
        drawn(&mut app, 120, 30);
        assert_eq!(list_width_now(&app), 60);
        app.handle_event(mouse(MouseEventKind::Up(MouseButton::Left), 60));
        let mut restarted = App::new(
            Favorites::load(path.with_file_name("f.json")),
            Identity::load(path.with_file_name("c.json")),
            None,
            mpsc::channel().0,
        )
            .with_ui_settings(UiSettingsStore::load(path.clone()));
        drawn(&mut restarted, 120, 30);
        assert_eq!(list_width_now(&restarted), 60);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn wheel_over_a_plain_child_scrolls_its_scrollback_and_over_the_transcript_scrolls_the_transcript() {
        use crossterm::event::{MouseEvent, MouseEventKind};
        let dir = temp_dir("wheel");
        let spec = script_child(&dir, "child.sh", "i=1; while [ $i -le 80 ]; do printf 'L%03d\\r\\n' $i; i=$((i+1)); done; exec sleep 30");
        let mut app = app();
        let id = launch_child(&mut app, &spec);
        wait_for_screen(&app, id, "L080");
        drawn(&mut app, 120, 30);
        let wheel = |app: &mut App, kind, column| {
            app.handle_event(Event::Mouse(MouseEvent { kind, column, row: 10, modifiers: KeyModifiers::NONE }));
        };
        wheel(&mut app, MouseEventKind::ScrollUp, 80);
        assert_eq!(app.sessions.get(id).unwrap().host().snapshot().scrollback_offset(), 3);
        wheel(&mut app, MouseEventKind::ScrollDown, 80);
        assert_eq!(app.sessions.get(id).unwrap().host().snapshot().scrollback_offset(), 0);
        app.sessions.remove(id);
        app.shown = None;
        app.focus = Focus::List;
        drawn(&mut app, 120, 30);
        app.max_scroll = 100;
        app.follow_bottom = false;
        app.scroll = 50;
        wheel(&mut app, MouseEventKind::ScrollUp, 80);
        assert_eq!(app.scroll, 47);
        wheel(&mut app, MouseEventKind::ScrollDown, 80);
        assert_eq!(app.scroll, 50);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
