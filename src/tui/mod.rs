mod badges;
mod conversation;
mod embedded;
mod follow;
mod footer;
mod launch;
mod list;
mod prefix;
mod runtime;
mod sessions;
mod tabs;
mod theme;

use crate::{
    activity,
    discover, external,
    favorites::Favorites,
    harness::{self, Harness, LaunchKind},
    library::{self, Library, SessionEntry, Update},
};
use conversation::Conversation;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
};
use follow::{Batch, Follower};
use launch::{PendingLaunch, Target};
use list::ListView;
use prefix::Prefix;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Padding, Paragraph},
};
pub use runtime::{Message, event_loop};
use sessions::SessionManager;
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant, SystemTime},
};
use tabs::{Tab, TabKey};

const OPEN_DEBOUNCE: Duration = Duration::from_millis(150);
const PAGE_SCROLL_OVERLAP: usize = 2;
const LIST_WIDTH_PERCENT: u16 = 40;
const MIN_LIST_WIDTH: u16 = 30;

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

struct OpenConversation {
    follower: Follower,
    conversation: Conversation,
    loaded: bool,
    error: Option<String>,
}

pub struct App {
    library: Library,
    favorites: Favorites,
    updates: Option<Receiver<Update>>,
    scanning: bool,
    tabs: Vec<Tab>,
    active: TabKey,
    rows: Vec<usize>,
    selected_path: Option<PathBuf>,
    focus: Focus,
    open: Option<OpenConversation>,
    pending_open: Option<(PathBuf, Instant)>,
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
}

pub fn run() -> io::Result<()> {
    let projects_dir =
        discover::claude_projects_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
    external::remove_stale_scripts();
    let (sender, messages) = mpsc::channel();
    let mut app = App::new(
        Favorites::load_default(),
        Some(library::spawn(projects_dir)),
        sender.clone(),
    );
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableBracketedPaste)?;
    runtime::spawn_input_reader(sender);
    let result = runtime::event_loop(
        &mut app,
        &messages,
        |app| terminal.draw(|frame| app.draw(frame)).map(|_| ()),
        runtime::set_mouse_capture,
    );
    let _ = runtime::set_mouse_capture(false);
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    drop(app);
    result
}

impl App {
    pub fn new(favorites: Favorites, updates: Option<Receiver<Update>>, wake: Sender<Message>) -> Self {
        let mut app = Self {
            library: Library::default(),
            favorites,
            updates,
            scanning: true,
            tabs: Vec::new(),
            active: TabKey::All,
            rows: Vec::new(),
            selected_path: None,
            focus: Focus::List,
            open: None,
            pending_open: None,
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
        };
        app.rebuild_view();
        app
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn output_ready(&mut self, id: u64) -> bool {
        let newly_exited = self.sessions.acknowledge_output(id);
        newly_exited || self.shown == Some(id)
    }

    pub fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Paste(text) => {
                self.handle_paste(&text);
                false
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse);
                false
            }
            Event::Resize(..) => true,
            _ => false,
        }
    }

    pub fn apply(&mut self, update: Update) {
        self.absorb(update);
        self.rebuild_view();
    }

    fn absorb(&mut self, update: Update) {
        match update {
            Update::Scanned(entries) => self.library.merge(entries),
            Update::Live(live) => {
                self.sessions.bind_live(&live);
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
        let mut changed = self.drain_updates();
        changed |= self.drain_conversation();
        if let Some((path, since)) = &self.pending_open {
            if since.elapsed() >= OPEN_DEBOUNCE {
                let path = path.clone();
                self.pending_open = None;
                self.open_conversation(path);
                changed = true;
            }
        }
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
        let Some(open) = &mut self.open else {
            return false;
        };
        let batches: Vec<Batch> = open.follower.batches.try_iter().collect();
        let changed = !batches.is_empty();
        for batch in batches {
            match batch {
                Batch::Initial { messages, hidden_earlier } => {
                    open.conversation.push_messages(&messages);
                    open.conversation.note_hidden_earlier(hidden_earlier);
                    open.loaded = true;
                }
                Batch::Appended(messages) => open.conversation.push_messages(&messages),
                Batch::Failed(message) => open.error = Some(message),
            }
        }
        changed
    }

    fn rebuild_view(&mut self) {
        self.tabs = tabs::build_tabs(self.library.entries(), self.library.activity());
        if !self.tabs.iter().any(|tab| tab.key == self.active) {
            self.active = TabKey::All;
        }
        self.refresh_rows();
        let still_present = self
            .selected_path
            .as_ref()
            .is_some_and(|path| self.rows.iter().any(|&i| &self.library.entries()[i].path == path));
        if !still_present {
            let first = self.rows.first().map(|&i| self.library.entries()[i].path.clone());
            self.select_path(first);
        }
    }

    fn refresh_rows(&mut self) {
        self.rows = tabs::session_indices(
            &self.active,
            self.library.entries(),
            &self.favorites,
            self.library.activity(),
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
        let already_open = self
            .open
            .as_ref()
            .is_some_and(|open| Some(&open.follower.path) == path.as_ref());
        self.pending_open = match path {
            Some(path) if !already_open => Some((path, Instant::now())),
            Some(_) => None,
            None => {
                self.open = None;
                None
            }
        };
    }

    fn open_conversation(&mut self, path: PathBuf) {
        self.open = Some(OpenConversation {
            follower: Follower::start(path),
            conversation: Conversation::default(),
            loaded: false,
            error: None,
        });
        self.scroll = 0;
        self.follow_bottom = true;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if let Some(confirm) = self.confirm.take() {
            return self.answer_confirm(confirm, key);
        }
        if self.focus == Focus::Embedded {
            return self.handle_embedded_key(key);
        }
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.request_quit();
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

    fn handle_list_only_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h' | '[') => self.switch_tab(-1),
            KeyCode::Right | KeyCode::Char('l' | ']') => self.switch_tab(1),
            KeyCode::Enter | KeyCode::Tab => self.focus = self.reading_focus(),
            _ => {}
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
            Focus::List => {
                let next = self.selected_row().map_or(0, |row| row + 1);
                if next < self.rows.len() {
                    self.select_row(next);
                }
            }
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Conversation | Focus::Embedded => self.scroll_up(1),
            Focus::List => {
                if let Some(row) = self.selected_row().filter(|&row| row > 0) {
                    self.select_row(row - 1);
                }
            }
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
        let strip_height = u16::from(!self.sessions.is_empty());
        let [tab_area, strip_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(strip_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let list_width = (frame.area().width * LIST_WIDTH_PERCENT / 100).max(MIN_LIST_WIDTH);
        let [list_area, right_area] =
            Layout::horizontal([Constraint::Length(list_width), Constraint::Min(0)])
                .areas(body_area);
        frame.render_widget(
            Paragraph::new(tabs::render_bar(&self.tabs, &self.active, tab_area.width as usize)),
            tab_area,
        );
        if strip_height > 0 {
            embedded::draw_strip(
                frame,
                strip_area,
                self.sessions.all(),
                self.shown,
                self.focus == Focus::Embedded,
                self.library.activity(),
            );
        }
        self.pane_inner = embedded::pane_block(None).inner(right_area);
        self.sessions.resize_all(embedded::inner_size(right_area));
        self.draw_list(frame, list_area);
        match self.shown.and_then(|id| self.sessions.get(id)) {
            Some(session) => {
                embedded::draw_pane(frame, right_area, session, self.focus == Focus::Embedded)
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
                selected: self.selected_row(),
                entries: rows,
                show_directory: !matches!(self.active, TabKey::Dir(_)),
                focused: self.focus == Focus::List,
                empty_message,
                favorites: &self.favorites,
                activity: self.library.activity(),
                now: SystemTime::now(),
            },
        );
    }

    fn draw_conversation(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Conversation;
        let title = self
            .selected_entry()
            .map(|entry| list::truncate(&entry.title, area.width.saturating_sub(6) as usize))
            .unwrap_or_else(|| "Conversation".to_owned());
        let block = embedded::pane_block(Some((&title, focused))).padding(Padding::horizontal(1));
        let inner = block.inner(area);
        self.viewport_height = inner.height as usize;
        let Some(open) = self.open.as_mut().filter(|_| self.selected_path.is_some()) else {
            let hint = if self.selected_path.is_some() {
                "Loading…"
            } else {
                "Select a session to read it here"
            };
            frame.render_widget(Paragraph::new(hint).style(theme::empty_state()).block(block), area);
            return;
        };
        if let Some(error) = &open.error {
            frame.render_widget(Paragraph::new(error.clone()).style(theme::notice()).block(block), area);
            return;
        }
        if !open.loaded {
            frame.render_widget(Paragraph::new("Loading…").style(theme::empty_state()).block(block), area);
            return;
        }
        let lines = open.conversation.lines(inner.width as usize);
        self.max_scroll = lines.len().saturating_sub(self.viewport_height);
        self.scroll = if self.follow_bottom {
            self.max_scroll
        } else {
            self.scroll.min(self.max_scroll)
        };
        let visible: Vec<Line> = lines
            .iter()
            .skip(self.scroll)
            .take(self.viewport_height)
            .cloned()
            .collect();
        frame.render_widget(Paragraph::new(visible).block(block), area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let prompt = self.confirm.as_ref().map(|confirm| self.confirm_prompt(confirm));
        let mode = match (&prompt, &self.notice, self.focus) {
            (Some(prompt), _, _) => footer::Mode::Confirm(prompt),
            (None, Some(notice), _) => footer::Mode::Notice(notice),
            (None, None, Focus::List) => footer::Mode::List,
            (None, None, Focus::Conversation) => footer::Mode::Transcript,
            (None, None, Focus::Embedded) => self.embedded_footer_mode(),
        };
        let summary = activity::summarize(self.library.activity().values());
        footer::draw(frame, area, &mode, self.scanning, &summary);
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
mod tests {
    use super::*;
    use crate::{library::TitleSource, terminal::{CommandSpec, Size}};
    use sessions::Launched;
    use std::{path::PathBuf, time::UNIX_EPOCH};

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
        }
    }

    fn app() -> App {
        let mut app = App::new(
            Favorites::load(std::env::temp_dir().join(format!("ccview-keys-{}/f.json", std::process::id()))),
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
    fn tabs_switch_only_while_the_list_is_focused() {
        let mut app = app();
        press(&mut app, KeyCode::Right);
        assert_eq!(app.active, TabKey::Favorites);
        type_char(&mut app, 'h');
        assert_eq!(app.active, TabKey::All);
        type_char(&mut app, ']');
        assert_eq!(app.active, TabKey::Favorites);
        type_char(&mut app, '[');
        assert_eq!(app.active, TabKey::All);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Conversation);
        for code in [KeyCode::Right, KeyCode::Char('l'), KeyCode::Char(']'), KeyCode::Char('['), KeyCode::Down, KeyCode::PageDown] {
            press(&mut app, code);
            assert_eq!((app.active.clone(), app.focus), (TabKey::All, Focus::Conversation), "{code:?}");
        }
        let selected = app.selected_path.clone();
        press(&mut app, KeyCode::Left);
        assert_eq!((app.focus, app.active.clone(), app.selected_path.clone()), (Focus::List, TabKey::All, selected.clone()));

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.focus, app.selected_path.clone()), (Focus::List, selected.clone()));
        press(&mut app, KeyCode::Enter);
        type_char(&mut app, 'h');
        assert_eq!(app.focus, Focus::List);
    }

    #[test]
    fn embedded_session_receives_every_navigation_key() {
        let mut app = app();
        let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
        let launched = Launched { title: "t".to_owned(), session_id: None };
        let id = app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
        app.show_embedded(id);
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('l'), KeyCode::Tab] {
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
    fn active_tab_badges_and_summary_render_at_120x35() {
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

        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen: Vec<String> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let text = screen.join("\n");
        for expected in ["Active 2", "needs you", "⛭ 3", "bg", "1 busy", "1 needs you", "3 agents running"] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }
}
