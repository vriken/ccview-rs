use super::code::render_code;
use super::table::render_table;
use super::theme::Theme;
use super::wrap::{Piece, clip_line, hang, wrap, wrap_lines};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

const MAX_CONTAINER_DEPTH: usize = 16;

const BULLETS: [&str; 3] = ["•", "◦", "▪"];

const QUOTE_BAR: &str = "│ ";

pub fn render(markdown: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let width = width as usize;
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::new(theme, width);
    for event in Parser::new_ext(markdown, options) {
        renderer.event(event);
    }
    renderer
        .finish()
        .into_iter()
        .map(|line| clip_line(line, width))
        .collect()
}

#[derive(PartialEq)]
enum FrameKind {
    Root,
    Item,
    Quote,
}

struct Frame {
    kind: FrameKind,
    width: usize,
    lines: Vec<Line<'static>>,
    separate: bool,
    marker: Vec<Span<'static>>,
    marker_width: usize,
}

struct ListState {
    next_number: Option<u64>,
    items: Vec<Vec<Line<'static>>>,
    loose: bool,
}

struct LinkState {
    url: String,
    first_piece: usize,
}

pub(super) struct CodeState {
    pub(super) language: String,
    pub(super) text: String,
}

pub(super) struct TableState {
    pub(super) alignments: Vec<Alignment>,
    pub(super) rows: Vec<Vec<Vec<Piece>>>,
    pub(super) header_rows: usize,
    current_row: Vec<Vec<Piece>>,
}

struct Renderer<'a> {
    theme: &'a Theme,
    frames: Vec<Frame>,
    lists: Vec<Option<ListState>>,
    item_opened: Vec<bool>,
    quote_opened: Vec<bool>,
    inline: Vec<Piece>,
    styles: Vec<Style>,
    links: Vec<LinkState>,
    code: Option<CodeState>,
    html: Option<String>,
    table: Option<TableState>,
    heading: Option<HeadingLevel>,
}

impl<'a> Renderer<'a> {
    fn new(theme: &'a Theme, width: usize) -> Self {
        Self {
            theme,
            frames: vec![Frame {
                kind: FrameKind::Root,
                width,
                lines: Vec::new(),
                separate: true,
                marker: Vec::new(),
                marker_width: 0,
            }],
            lists: Vec::new(),
            item_opened: Vec::new(),
            quote_opened: Vec::new(),
            inline: Vec::new(),
            styles: vec![Style::default()],
            links: Vec::new(),
            code: None,
            html: None,
            table: None,
            heading: None,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline();
        while self.frames.len() > 1 {
            self.frames.pop();
        }
        self.frames
            .pop()
            .map(|frame| frame.lines)
            .unwrap_or_default()
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, style: Style) {
        let combined = self.style().patch(style);
        self.styles.push(combined);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn width(&self) -> usize {
        self.frames.last().map_or(1, |frame| frame.width)
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("root frame always present")
    }

    fn push_block(&mut self, lines: Vec<Line<'static>>) {
        if lines.is_empty() {
            return;
        }
        let frame = self.frame_mut();
        if frame.separate && !frame.lines.is_empty() {
            frame.lines.push(Line::default());
        }
        frame.lines.extend(lines);
    }

    fn flush_inline(&mut self) {
        if self.table.is_some() || self.inline.is_empty() {
            return;
        }
        let pieces = std::mem::take(&mut self.inline);
        let lines = wrap_lines(&pieces, self.width());
        self.push_block(lines);
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(text.into_string()),
            Event::Code(text) => {
                let style = self.style().patch(self.theme.inline_code);
                self.inline.push(Piece::Text(text.into_string(), style));
            }
            Event::Html(text) | Event::InlineHtml(text) => match self.html.as_mut() {
                Some(buffer) => buffer.push_str(&text),
                None => {
                    let style = self.style().patch(self.theme.dim);
                    self.inline.push(Piece::Text(text.into_string(), style));
                }
            },
            Event::SoftBreak => {
                let style = self.style();
                self.inline.push(Piece::Text(" ".into(), style));
            }
            Event::HardBreak => self.inline.push(Piece::Break),
            Event::Rule => {
                self.flush_inline();
                let line = Line::from(Span::styled("─".repeat(self.width()), self.theme.rule));
                self.push_block(vec![line]);
            }
            Event::TaskListMarker(checked) => self.task_marker(checked),
            _ => {}
        }
    }

    fn text(&mut self, text: String) {
        if let Some(code) = self.code.as_mut() {
            code.text.push_str(&text);
        } else if let Some(html) = self.html.as_mut() {
            html.push_str(&text);
        } else {
            let style = self.style();
            self.inline.push(Piece::Text(text, style));
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.frame_mut().kind == FrameKind::Item {
                    self.frame_mut().separate = true;
                }
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.heading = Some(level);
                self.push_style(heading_style(self.theme, level));
            }
            Tag::BlockQuote(_) => self.start_quote(),
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeState {
                    language,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => {
                self.flush_inline();
                self.html = Some(String::new());
            }
            Tag::List(first) => {
                self.flush_inline();
                if self.frames.len() > MAX_CONTAINER_DEPTH {
                    self.lists.push(None);
                } else {
                    self.lists.push(Some(ListState {
                        next_number: first,
                        items: Vec::new(),
                        loose: false,
                    }));
                }
            }
            Tag::Item => self.start_item(),
            Tag::Table(alignments) => {
                self.flush_inline();
                self.table = Some(TableState {
                    alignments,
                    rows: Vec::new(),
                    header_rows: 0,
                    current_row: Vec::new(),
                });
            }
            Tag::TableHead => self.push_style(self.theme.strong),
            Tag::Emphasis => self.push_style(self.theme.emphasis),
            Tag::Strong => self.push_style(self.theme.strong),
            Tag::Strikethrough => self.push_style(self.theme.strikethrough),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.push_style(self.theme.link);
                self.links.push(LinkState {
                    url: dest_url.into_string(),
                    first_piece: self.inline.len(),
                });
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_inline(),
            TagEnd::Heading(level) => self.end_heading(level),
            TagEnd::BlockQuote(_) => self.end_quote(),
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    let lines = render_code(&code, self.width(), self.theme);
                    self.push_block(lines);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    let style = self.style().patch(self.theme.dim);
                    let lines = render_html(&html, self.width(), style);
                    self.push_block(lines);
                }
            }
            TagEnd::List(_) => self.end_list(),
            TagEnd::Item => self.end_item(),
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    let lines = render_table(&table, self.width(), self.theme);
                    self.push_block(lines);
                }
            }
            TagEnd::TableHead => {
                self.pop_style();
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.current_row);
                    table.rows.push(row);
                    table.header_rows = 1;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.current_row);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(table) = self.table.as_mut() {
                    table.current_row.push(cell);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link | TagEnd::Image => self.end_link(),
            _ => {}
        }
    }

    fn end_link(&mut self) {
        self.pop_style();
        let Some(link) = self.links.pop() else {
            return;
        };
        if link.url.is_empty() {
            return;
        }
        let shown: String = self.inline[link.first_piece.min(self.inline.len())..]
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text(text, _) => Some(text.as_str()),
                Piece::Break => None,
            })
            .collect();
        if shown == link.url {
            return;
        }
        let style = self.style().patch(self.theme.dim);
        self.inline
            .push(Piece::Text(format!(" ({})", link.url), style));
    }

    fn end_heading(&mut self, level: HeadingLevel) {
        self.pop_style();
        self.heading = None;
        let pieces = std::mem::take(&mut self.inline);
        let hashes = heading_depth(level);
        let prefix = format!("{} ", "#".repeat(hashes));
        let width = self.width();
        let prefix_width = if prefix.len() < width {
            prefix.len()
        } else {
            0
        };
        let lines = wrap_lines(&pieces, width - prefix_width);
        let marker = vec![Span::styled(prefix, heading_style(self.theme, level))];
        let lines = hang(lines, marker, prefix_width);
        self.push_block(lines);
    }

    fn start_quote(&mut self) {
        self.flush_inline();
        self.push_style(self.theme.quote);
        if self.frames.len() > MAX_CONTAINER_DEPTH {
            self.quote_opened.push(false);
            return;
        }
        self.quote_opened.push(true);
        let parent_width = self.width();
        let bar_width = if parent_width > QUOTE_BAR.len() {
            QUOTE_BAR.len()
        } else {
            0
        };
        self.frames.push(Frame {
            kind: FrameKind::Quote,
            width: parent_width - bar_width,
            lines: Vec::new(),
            separate: true,
            marker: Vec::new(),
            marker_width: bar_width,
        });
    }

    fn end_quote(&mut self) {
        self.flush_inline();
        self.pop_style();
        if !self.quote_opened.pop().unwrap_or(false) {
            return;
        }
        let Some(frame) = self.frames.pop() else {
            return;
        };
        if frame.marker_width == 0 {
            self.push_block(frame.lines);
            return;
        }
        let bar = self.theme.quote;
        let lines = frame
            .lines
            .into_iter()
            .map(|line| {
                if line.spans.is_empty() {
                    Line::from(Span::styled("│", bar))
                } else {
                    let mut spans = vec![Span::styled(QUOTE_BAR, bar)];
                    spans.extend(line.spans);
                    Line::from(spans)
                }
            })
            .collect();
        self.push_block(lines);
    }

    fn start_item(&mut self) {
        self.flush_inline();
        let depth = self.lists.len().saturating_sub(1);
        let marker_text = match self.lists.last_mut() {
            Some(Some(list)) => Some(match list.next_number.as_mut() {
                Some(number) => {
                    let text = format!("{number}. ");
                    *number = number.saturating_add(1);
                    text
                }
                None => format!("{} ", BULLETS[depth % BULLETS.len()]),
            }),
            _ => None,
        };
        let Some(marker_text) = marker_text else {
            self.item_opened.push(false);
            return;
        };
        self.item_opened.push(true);
        let parent_width = self.width();
        self.frames.push(Frame {
            kind: FrameKind::Item,
            width: parent_width,
            lines: Vec::new(),
            separate: false,
            marker: Vec::new(),
            marker_width: 0,
        });
        self.set_item_marker(marker_text);
    }

    fn set_item_marker(&mut self, text: String) {
        let parent_width = self.frames[self.frames.len() - 2].width;
        let text_width = text.chars().count();
        let marker_width = if text_width < parent_width {
            text_width
        } else {
            0
        };
        let style = self.theme.list_marker;
        let frame = self.frame_mut();
        frame.width = parent_width - marker_width;
        frame.marker_width = marker_width;
        frame.marker = if marker_width == 0 {
            Vec::new()
        } else {
            vec![Span::styled(text, style)]
        };
    }

    fn task_marker(&mut self, checked: bool) {
        if self.frame_mut().kind == FrameKind::Item {
            let text = if checked { "[x] " } else { "[ ] " };
            self.set_item_marker(text.to_string());
        } else {
            let style = self.style();
            let text = if checked { "[x] " } else { "[ ] " };
            self.inline.push(Piece::Text(text.into(), style));
        }
    }

    fn end_item(&mut self) {
        self.flush_inline();
        if !self.item_opened.pop().unwrap_or(false) {
            return;
        }
        let Some(frame) = self.frames.pop() else {
            return;
        };
        let loose = frame.separate;
        let lines = hang(frame.lines, frame.marker, frame.marker_width);
        if let Some(Some(list)) = self.lists.last_mut() {
            list.loose |= loose;
            list.items.push(lines);
        }
    }

    fn end_list(&mut self) {
        let Some(Some(list)) = self.lists.pop() else {
            return;
        };
        let mut lines = Vec::new();
        for (index, item) in list.items.into_iter().enumerate() {
            if list.loose && index > 0 {
                lines.push(Line::default());
            }
            lines.extend(item);
        }
        self.push_block(lines);
    }
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        _ => 4,
    }
}

fn heading_style(theme: &Theme, level: HeadingLevel) -> Style {
    match heading_depth(level) {
        1 => theme.h1,
        2 => theme.h2,
        3 => theme.h3,
        _ => theme.h4,
    }
}

fn render_html(html: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for raw in html.trim_end_matches('\n').split('\n') {
        let wrapped = wrap(&[Piece::Text(raw.to_string(), style)], width);
        if wrapped.is_empty() {
            lines.push(Line::default());
        } else {
            lines.extend(wrapped.into_iter().map(Line::from));
        }
    }
    lines
}
