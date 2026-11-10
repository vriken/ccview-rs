mod conversation;
mod sidebar;

use crate::{discover, parse};
use conversation::Conversation;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};
use sidebar::Project;
use std::{io, time::SystemTime};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Content,
}

#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Session(usize, usize),
}

const PAGE_SCROLL_OVERLAP: usize = 2;

pub struct App {
    projects: Vec<Project>,
    rows: Vec<Row>,
    selected_row: usize,
    focus: Focus,
    conversation: Option<Conversation>,
    status: String,
    scroll: usize,
    viewport_height: usize,
    should_quit: bool,
}

pub fn run() -> io::Result<()> {
    let projects_dir = discover::claude_projects_dir()
        .ok_or_else(|| io::Error::other("HOME is not set"))?;
    let mut app = App::new(sidebar::load_projects(&projects_dir));
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| app.draw(frame))?;
        if let Event::Key(key) = event::read()? {
            app.handle_key(key);
        }
    }
    Ok(())
}

impl App {
    pub fn new(projects: Vec<Project>) -> Self {
        let rows = build_rows(&projects);
        let selected_row = rows
            .iter()
            .position(|row| matches!(row, Row::Session(..)))
            .unwrap_or(0);
        Self {
            projects,
            rows,
            selected_row,
            focus: Focus::Sidebar,
            conversation: None,
            status: String::new(),
            scroll: 0,
            viewport_height: 0,
            should_quit: false,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Content,
                    Focus::Content => Focus::Sidebar,
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Sidebar,
            KeyCode::Char('l') | KeyCode::Right => self.focus = Focus::Content,
            KeyCode::Enter => self.open_selected(),
            KeyCode::PageDown => self.scroll_down(self.page_size()),
            KeyCode::PageUp => self.scroll_up(self.page_size()),
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = usize::MAX,
            _ => {}
        }
    }

    fn page_size(&self) -> usize {
        self.viewport_height.saturating_sub(PAGE_SCROLL_OVERLAP).max(1)
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Content => self.scroll_down(1),
            Focus::Sidebar => {
                let below = self.rows.get(self.selected_row + 1..).unwrap_or(&[]);
                if let Some(offset) = below
                    .iter()
                    .position(|row| matches!(row, Row::Session(..)))
                {
                    self.selected_row += 1 + offset;
                }
            }
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Content => self.scroll_up(1),
            Focus::Sidebar => {
                if let Some(row) = self.rows[..self.selected_row]
                    .iter()
                    .rposition(|row| matches!(row, Row::Session(..)))
                {
                    self.selected_row = row;
                }
            }
        }
    }

    fn open_selected(&mut self) {
        let Some(Row::Session(project, session)) = self.rows.get(self.selected_row).copied()
        else {
            return;
        };
        let path = &self.projects[project].sessions[session].path;
        match parse::parse_file(path) {
            Ok(parsed) => {
                self.status = format!(
                    "{} messages, {} malformed lines",
                    parsed.messages.len(),
                    parsed.malformed_lines
                );
                self.conversation = Some(Conversation::from_messages(&parsed.messages));
            }
            Err(err) => {
                self.status = format!("{}: {err}", path.display());
                self.conversation = None;
            }
        }
        self.scroll = 0;
        self.focus = Focus::Content;
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let [sidebar_area, content_area] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(frame.area());
        self.draw_sidebar(frame, sidebar_area);
        self.draw_content(frame, content_area);
    }

    fn pane_block(&self, title: String, focus: Focus) -> Block<'static> {
        let border = if self.focus == focus {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title(title)
    }

    fn draw_sidebar(&self, frame: &mut Frame, area: Rect) {
        let now = SystemTime::now();
        let inner_width = area.width.saturating_sub(2) as usize;
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|row| match *row {
                Row::Header(project) => ListItem::new(Line::styled(
                    truncate(&self.projects[project].name, inner_width),
                    Style::default().add_modifier(Modifier::BOLD).fg(Color::Magenta),
                )),
                Row::Session(project, session) => {
                    let entry = &self.projects[project].sessions[session];
                    let time = sidebar::format_time(entry.modified, now);
                    let preview_width = inner_width.saturating_sub(time.chars().count() + 4);
                    ListItem::new(Line::from(vec![
                        Span::raw("  "),
                        Span::raw(truncate(&entry.preview, preview_width)),
                        Span::raw(" "),
                        Span::styled(time, Style::default().fg(Color::DarkGray)),
                    ]))
                }
            })
            .collect();
        let list = List::new(items)
            .block(self.pane_block("Projects".to_owned(), Focus::Sidebar))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default().with_selected(Some(self.selected_row));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_content(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.status.is_empty() {
            "Conversation".to_owned()
        } else {
            format!("Conversation ({})", self.status)
        };
        let block = self.pane_block(title, Focus::Content);
        let inner = block.inner(area);
        self.viewport_height = inner.height as usize;
        let Some(conversation) = self.conversation.as_mut() else {
            let hint = Paragraph::new("Select a session and press enter").block(block);
            frame.render_widget(hint, area);
            return;
        };
        let lines = conversation.lines(inner.width as usize);
        let max_scroll = lines.len().saturating_sub(self.viewport_height);
        self.scroll = self.scroll.min(max_scroll);
        let visible: Vec<Line> = lines
            .iter()
            .skip(self.scroll)
            .take(self.viewport_height)
            .cloned()
            .collect();
        frame.render_widget(Paragraph::new(visible).block(block), area);
    }
}

fn build_rows(projects: &[Project]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (project, entry) in projects.iter().enumerate() {
        rows.push(Row::Header(project));
        rows.extend((0..entry.sessions.len()).map(|session| Row::Session(project, session)));
    }
    rows
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}
