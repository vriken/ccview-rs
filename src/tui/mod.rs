mod conversation;
mod follow;
mod list;
mod tabs;
mod theme;

use crate::{
    discover,
    favorites::Favorites,
    library::{self, Library, SessionEntry, Update},
};
use conversation::Conversation;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use follow::{Batch, Follower};
use list::ListView;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Padding, Paragraph},
};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::Receiver,
    time::{Duration, Instant, SystemTime},
};
use tabs::{Tab, TabKey};

const OPEN_DEBOUNCE: Duration = Duration::from_millis(150);
const EVENT_POLL: Duration = Duration::from_millis(50);
const PAGE_SCROLL_OVERLAP: usize = 2;
const LIST_WIDTH_PERCENT: u16 = 40;
const MIN_LIST_WIDTH: u16 = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Conversation,
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
}

pub fn run() -> io::Result<()> {
    let projects_dir =
        discover::claude_projects_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
    let mut app = App::new(Favorites::load_default(), Some(library::spawn(projects_dir)));
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut redraw = true;
    while !app.should_quit {
        if redraw {
            terminal.draw(|frame| app.draw(frame))?;
        }
        redraw = false;
        while event::poll(if redraw { Duration::ZERO } else { EVENT_POLL })? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
            redraw = true;
        }
        redraw |= app.tick();
    }
    Ok(())
}

impl App {
    pub fn new(favorites: Favorites, updates: Option<Receiver<Update>>) -> Self {
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
        };
        app.rebuild_view();
        app
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn apply(&mut self, update: Update) {
        self.absorb(update);
        self.rebuild_view();
    }

    fn absorb(&mut self, update: Update) {
        match update {
            Update::Scanned(entries) => self.library.merge(entries),
            Update::Live(live) => self.library.set_live(live),
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
        self.tabs = tabs::build_tabs(self.library.entries());
        if !self.tabs.iter().any(|tab| tab.key == self.active) {
            self.active = TabKey::All;
        }
        self.rows =
            tabs::session_indices(&self.active, self.library.entries(), &self.favorites);
        let still_present = self
            .selected_path
            .as_ref()
            .is_some_and(|path| self.rows.iter().any(|&i| &self.library.entries()[i].path == path));
        if !still_present {
            let first = self.rows.first().map(|&i| self.library.entries()[i].path.clone());
            self.select_path(first);
        }
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

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Left | KeyCode::Char('h' | '[') => self.switch_tab(-1),
            KeyCode::Right | KeyCode::Char('l' | ']') => self.switch_tab(1),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Char('s' | ' ') => self.toggle_star(),
            KeyCode::Enter => self.focus = Focus::Conversation,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::List => Focus::Conversation,
                    Focus::Conversation => Focus::List,
                }
            }
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::PageDown => self.scroll_down(self.page_size()),
            KeyCode::PageUp => self.scroll_up(self.page_size()),
            KeyCode::Char('g') => {
                self.scroll = 0;
                self.follow_bottom = false;
            }
            KeyCode::Char('G') => self.follow_bottom = true,
            _ => {}
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
            Focus::Conversation => self.scroll_down(1),
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
            Focus::Conversation => self.scroll_up(1),
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
        self.rows =
            tabs::session_indices(&self.active, self.library.entries(), &self.favorites);
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
        self.rows =
            tabs::session_indices(&self.active, self.library.entries(), &self.favorites);
        if self.selected_row().is_none() {
            let row = previous.unwrap_or(0).min(self.rows.len().saturating_sub(1));
            self.select_row(row);
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let [tab_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let list_width = (frame.area().width * LIST_WIDTH_PERCENT / 100).max(MIN_LIST_WIDTH);
        let [list_area, conversation_area] =
            Layout::horizontal([Constraint::Length(list_width), Constraint::Min(0)])
                .areas(body_area);
        frame.render_widget(
            Paragraph::new(tabs::render_bar(&self.tabs, &self.active, tab_area.width as usize)),
            tab_area,
        );
        self.draw_list(frame, list_area);
        self.draw_conversation(frame, conversation_area);
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
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::border(focused))
            .padding(Padding::horizontal(1))
            .title(Span::styled(format!(" {title} "), theme::pane_title(focused)));
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
        let width = area.width as usize;
        if let Some(notice) = &self.notice {
            frame.render_widget(
                Paragraph::new(Span::styled(format!(" {notice}"), theme::notice())),
                area,
            );
            return;
        }
        let hints: &[(&str, &str)] = match self.focus {
            Focus::List => &[
                ("←/→", "tab"),
                ("↑/↓", "move"),
                ("s", "star"),
                ("enter", "read"),
                ("q", "quit"),
            ],
            Focus::Conversation => &[
                ("esc", "list"),
                ("←/→", "tab"),
                ("↑/↓", "scroll"),
                ("PgUp/PgDn", "page"),
                ("g/G", "top/end"),
                ("s", "star"),
                ("q", "quit"),
            ],
        };
        let mut spans = vec![Span::raw(" ")];
        let mut used = 1;
        for (key, label) in hints {
            let needed = key.chars().count() + label.chars().count() + 4;
            if used + needed > width {
                break;
            }
            spans.push(Span::styled((*key).to_owned(), theme::footer_key()));
            spans.push(Span::styled(format!(" {label}   "), theme::footer_text()));
            used += needed;
        }
        if self.scanning && used + 12 <= width {
            spans.push(Span::styled("scanning…", theme::footer_text()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}
