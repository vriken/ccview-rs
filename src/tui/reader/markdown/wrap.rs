use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub(super) enum Piece {
    Text(String, Style),
    Break,
}

pub(super) fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

pub(super) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| span.content.chars().map(char_width).sum::<usize>())
        .sum()
}

pub(super) fn hang(
    lines: Vec<Line<'static>>,
    marker: Vec<Span<'static>>,
    marker_width: usize,
) -> Vec<Line<'static>> {
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

pub(super) fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
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

pub(super) fn wrap(pieces: &[Piece], width: usize) -> Vec<Vec<Span<'static>>> {
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

pub(super) fn wrap_lines(pieces: &[Piece], width: usize) -> Vec<Line<'static>> {
    wrap(pieces, width).into_iter().map(Line::from).collect()
}

pub(super) fn flat_width(pieces: &[Piece]) -> usize {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text, _) => text.chars().map(char_width).sum(),
            Piece::Break => 1,
        })
        .sum()
}
