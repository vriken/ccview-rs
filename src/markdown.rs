use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

const MAX_CONTAINER_DEPTH: usize = 16;
const BULLETS: [&str; 3] = ["•", "◦", "▪"];
const QUOTE_BAR: &str = "│ ";
const TAB_WIDTH: usize = 4;
const MIN_TABLE_COLUMN_WIDTH: usize = 3;

#[derive(Clone, Debug)]
pub struct Theme {
    pub h1: Style,
    pub h2: Style,
    pub h3: Style,
    pub h4: Style,
    pub strong: Style,
    pub emphasis: Style,
    pub strikethrough: Style,
    pub inline_code: Style,
    pub code_block: Style,
    pub quote: Style,
    pub link: Style,
    pub list_marker: Style,
    pub rule: Style,
    pub table_border: Style,
    pub dim: Style,
}

impl Default for Theme {
    fn default() -> Self {
        let muted = Color::Indexed(244);
        Self {
            h1: Style::new()
                .fg(Color::Indexed(213))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            h2: Style::new().fg(Color::Indexed(81)).add_modifier(Modifier::BOLD),
            h3: Style::new().fg(Color::Indexed(114)).add_modifier(Modifier::BOLD),
            h4: Style::new().add_modifier(Modifier::BOLD),
            strong: Style::new().add_modifier(Modifier::BOLD),
            emphasis: Style::new().add_modifier(Modifier::ITALIC),
            strikethrough: Style::new().add_modifier(Modifier::CROSSED_OUT),
            inline_code: Style::new().fg(Color::Indexed(216)).bg(Color::Indexed(236)),
            code_block: Style::new().fg(Color::Indexed(252)).bg(Color::Indexed(235)),
            quote: Style::new().fg(Color::Indexed(246)),
            link: Style::new()
                .fg(Color::Indexed(75))
                .add_modifier(Modifier::UNDERLINED),
            list_marker: Style::new().fg(Color::Indexed(109)),
            rule: Style::new().fg(Color::Indexed(240)),
            table_border: Style::new().fg(Color::Indexed(240)),
            dim: Style::new().fg(muted),
        }
    }
}

pub fn render(markdown: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let width = width as usize;
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
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

enum Piece {
    Text(String, Style),
    Break,
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

struct CodeState {
    language: String,
    text: String,
}

struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Piece>>>,
    header_rows: usize,
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
        self.frames.pop().map(|frame| frame.lines).unwrap_or_default()
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
        let prefix_width = if prefix.len() < width { prefix.len() } else { 0 };
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
        let bar_width = if parent_width > QUOTE_BAR.len() { QUOTE_BAR.len() } else { 0 };
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
        let marker_width = if text_width < parent_width { text_width } else { 0 };
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

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| span.content.chars().map(char_width).sum::<usize>())
        .sum()
}

fn hang(lines: Vec<Line<'static>>, marker: Vec<Span<'static>>, marker_width: usize) -> Vec<Line<'static>> {
    if marker_width == 0 {
        return lines;
    }
    if lines.is_empty() {
        return vec![Line::from(marker)];
    }
    let indent = " ".repeat(marker_width);
    let mut marker = Some(marker);
    lines
        .into_iter()
        .map(|line| {
            if let Some(marker) = marker.take() {
                let mut spans = marker;
                spans.extend(line.spans);
                Line::from(spans)
            } else if line.spans.is_empty() {
                line
            } else {
                let mut spans = vec![Span::raw(indent.clone())];
                spans.extend(line.spans);
                Line::from(spans)
            }
        })
        .collect()
}

fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
    if spans_width(&line.spans) <= width {
        return line;
    }
    let mut remaining = width;
    let mut spans = Vec::new();
    for span in line.spans {
        if remaining == 0 {
            break;
        }
        let mut kept = String::new();
        for c in span.content.chars() {
            let c_width = char_width(c);
            if c_width > remaining {
                remaining = 0;
                break;
            }
            remaining -= c_width;
            kept.push(c);
        }
        if !kept.is_empty() {
            spans.push(Span::styled(kept, span.style));
        }
    }
    Line::from(spans)
}

struct Wrapper {
    width: usize,
    lines: Vec<Vec<Span<'static>>>,
    current: Vec<Span<'static>>,
    current_width: usize,
    pending_space: Option<Style>,
}

impl Wrapper {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            current: Vec::new(),
            current_width: 0,
            pending_space: None,
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        match self.current.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(text),
            _ => self.current.push(Span::styled(text.to_string(), style)),
        }
    }

    fn break_line(&mut self) {
        self.lines.push(std::mem::take(&mut self.current));
        self.current_width = 0;
        self.pending_space = None;
    }

    fn place(&mut self, word: &mut Vec<(String, Style)>, word_width: &mut usize) {
        if word.is_empty() {
            return;
        }
        let parts = std::mem::take(word);
        let total = std::mem::take(word_width);
        let space = match self.pending_space.take() {
            Some(style) if self.current_width > 0 => Some(style),
            _ => None,
        };
        let space_width = usize::from(space.is_some());
        if total == 0 || self.current_width + space_width + total <= self.width {
            if let Some(style) = space {
                self.push_text(" ", style);
            }
            for (text, style) in &parts {
                self.push_text(text, *style);
            }
            self.current_width += space_width + total;
            return;
        }
        if self.current_width > 0 {
            self.break_line();
        }
        if total <= self.width {
            for (text, style) in &parts {
                self.push_text(text, *style);
            }
            self.current_width = total;
            return;
        }
        let mut buffer = [0u8; 4];
        for (text, style) in &parts {
            for c in text.chars() {
                let c_width = char_width(c);
                if self.current_width > 0 && self.current_width + c_width > self.width {
                    self.break_line();
                }
                self.push_text(c.encode_utf8(&mut buffer), *style);
                self.current_width += c_width;
            }
        }
    }

    fn finish(mut self) -> Vec<Vec<Span<'static>>> {
        if !self.current.is_empty() {
            self.lines.push(self.current);
        }
        self.lines
    }
}

fn wrap(pieces: &[Piece], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut wrapper = Wrapper::new(width.max(1));
    let mut word: Vec<(String, Style)> = Vec::new();
    let mut word_width = 0;
    for piece in pieces {
        match piece {
            Piece::Break => {
                wrapper.place(&mut word, &mut word_width);
                wrapper.break_line();
            }
            Piece::Text(text, style) => {
                for c in text.chars() {
                    if c.is_whitespace() {
                        wrapper.place(&mut word, &mut word_width);
                        wrapper.pending_space = Some(*style);
                    } else if !c.is_control() {
                        match word.last_mut() {
                            Some((last, last_style)) if last_style == style => last.push(c),
                            _ => word.push((c.to_string(), *style)),
                        }
                        word_width += char_width(c);
                    }
                }
            }
        }
    }
    wrapper.place(&mut word, &mut word_width);
    wrapper.finish()
}

fn wrap_lines(pieces: &[Piece], width: usize) -> Vec<Line<'static>> {
    wrap(pieces, width).into_iter().map(Line::from).collect()
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

fn render_code(code: &CodeState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let padding = if width >= 6 { 1 } else { 0 };
    let inner = width - 2 * padding;
    let style = theme.code_block;
    let framed = |content: &str, content_width: usize, content_style: Style| {
        let left = " ".repeat(padding);
        let right = " ".repeat((inner + padding).saturating_sub(content_width));
        Line::from(vec![
            Span::styled(left, style),
            Span::styled(content.to_string(), content_style),
            Span::styled(right, style),
        ])
    };

    let mut lines = Vec::new();
    if !code.language.is_empty() {
        let mut label = String::new();
        let mut label_width = 0;
        for c in code.language.chars().filter(|c| !c.is_control()) {
            if label_width + char_width(c) > inner {
                break;
            }
            label.push(c);
            label_width += char_width(c);
        }
        let label_style = style.patch(theme.dim).add_modifier(Modifier::ITALIC);
        lines.push(framed(&label, label_width, label_style));
    }

    let text = code.text.strip_suffix('\n').unwrap_or(&code.text);
    for raw in text.split('\n') {
        let mut segment = String::new();
        let mut segment_width = 0;
        let push_char = |c: char, lines: &mut Vec<Line<'static>>, segment: &mut String, segment_width: &mut usize| {
            let c_width = char_width(c);
            if *segment_width > 0 && *segment_width + c_width > inner {
                lines.push(framed(segment, *segment_width, style));
                segment.clear();
                *segment_width = 0;
            }
            segment.push(c);
            *segment_width += c_width;
        };
        for c in raw.chars() {
            if c == '\t' {
                for _ in 0..TAB_WIDTH {
                    push_char(' ', &mut lines, &mut segment, &mut segment_width);
                }
            } else if !c.is_control() {
                push_char(c, &mut lines, &mut segment, &mut segment_width);
            }
        }
        lines.push(framed(&segment, segment_width, style));
    }
    lines
}

fn flat_width(pieces: &[Piece]) -> usize {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text, _) => text.chars().map(char_width).sum(),
            Piece::Break => 1,
        })
        .sum()
}

fn fit_column_widths(natural: &[usize], available: usize) -> Vec<usize> {
    let total: usize = natural.iter().sum();
    if total <= available {
        return natural.to_vec();
    }
    let used_at = |cap: usize| natural.iter().map(|&w| w.min(cap)).sum::<usize>();
    let (mut low, mut high) = (MIN_TABLE_COLUMN_WIDTH, natural.iter().copied().max().unwrap_or(1));
    while low < high {
        let mid = (low + high).div_ceil(2);
        if used_at(mid) <= available {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut widths: Vec<usize> = natural.iter().map(|&w| w.min(low)).collect();
    let mut leftover = available.saturating_sub(widths.iter().sum());
    for (width, &wanted) in widths.iter_mut().zip(natural) {
        if leftover == 0 {
            break;
        }
        if *width < wanted {
            *width += 1;
            leftover -= 1;
        }
    }
    widths
}

fn border_line(widths: &[usize], left: &str, mid: &str, right: &str, style: Style) -> Line<'static> {
    let mut text = String::from(left);
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            text.push_str(mid);
        }
        text.push_str(&"─".repeat(width + 2));
    }
    text.push_str(right);
    Line::from(Span::styled(text, style))
}

fn render_table(table: &TableState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let columns = table
        .rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(table.alignments.len());
    if columns == 0 {
        return Vec::new();
    }
    let chrome = 3 * columns + 1;
    let available = width.saturating_sub(chrome);
    if available < MIN_TABLE_COLUMN_WIDTH * columns {
        return render_table_as_rows(table, width, theme);
    }

    let mut natural = vec![1; columns];
    for row in &table.rows {
        for (index, cell) in row.iter().enumerate() {
            natural[index] = natural[index].max(flat_width(cell));
        }
    }
    let widths = fit_column_widths(&natural, available);
    let border = theme.table_border;
    let mut lines = vec![border_line(&widths, "┌", "┬", "┐", border)];
    for (row_index, row) in table.rows.iter().enumerate() {
        let wrapped: Vec<Vec<Vec<Span<'static>>>> = (0..columns)
            .map(|column| match row.get(column) {
                Some(cell) => wrap(cell, widths[column]),
                None => Vec::new(),
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for line_index in 0..height {
            let mut spans = vec![Span::styled("│", border)];
            for column in 0..columns {
                let cell_line = wrapped[column].get(line_index).cloned().unwrap_or_default();
                let slack = widths[column] - spans_width(&cell_line).min(widths[column]);
                let (left, right) = match table.alignments.get(column) {
                    Some(Alignment::Right) => (slack, 0),
                    Some(Alignment::Center) => (slack / 2, slack - slack / 2),
                    _ => (0, slack),
                };
                spans.push(Span::raw(" ".repeat(1 + left)));
                spans.extend(cell_line);
                spans.push(Span::raw(" ".repeat(1 + right)));
                spans.push(Span::styled("│", border));
            }
            lines.push(Line::from(spans));
        }
        if row_index + 1 == table.header_rows {
            lines.push(border_line(&widths, "├", "┼", "┤", border));
        }
    }
    lines.push(border_line(&widths, "└", "┴", "┘", border));
    lines
}

fn render_table_as_rows(table: &TableState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (row_index, row) in table.rows.iter().enumerate() {
        let mut pieces = Vec::new();
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                pieces.push(Piece::Text(" │ ".into(), theme.table_border));
            }
            for piece in cell {
                pieces.push(match piece {
                    Piece::Text(text, style) => Piece::Text(text.clone(), *style),
                    Piece::Break => Piece::Break,
                });
            }
        }
        lines.extend(wrap_lines(&pieces, width));
        if row_index + 1 == table.header_rows {
            lines.push(Line::from(Span::styled("─".repeat(width), theme.rule)));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_width(line: &Line<'_>) -> usize {
        spans_width(&line.spans)
    }

    #[test]
    fn no_line_exceeds_width_on_hostile_input() {
        let hostile = format!(
            "# {}\n\n> > > {}\n\n- a\n  - b\n    - {}\n\n| a | b |\n|---|---|\n| {} | x |\n\n```rust\n{}\n",
            "w".repeat(500),
            "word ".repeat(100),
            "漢字".repeat(100),
            "z".repeat(300),
            "long ".repeat(200),
        );
        let theme = Theme::default();
        for width in [1u16, 2, 5, 20, 80] {
            for line in render(&hostile, width, &theme) {
                assert!(line_width(&line) <= width as usize, "width {width}");
            }
        }
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let theme = Theme::default();
        assert!(render("", 80, &theme).is_empty());
        assert!(render("text", 0, &theme).is_empty());
        render("```", 1, &theme);
        render("|", 3, &theme);
    }
}
