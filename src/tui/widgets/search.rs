use crate::{catalog::search::snippet::Snippet, tui::style::theme};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

const PROMPT: &str = "/ ";
const CURSOR: &str = "▏";
const ELLIPSIS: &str = "…";
const STATUS_GAP: usize = 1;
const MIN_QUERY_ROOM: usize = 8;
const LEAD_COLUMNS: usize = 8;

pub(crate) struct ResultLine {
    pub(crate) hits: Option<u32>,
    pub(crate) snippet: Option<Snippet>,
    pub(crate) title_spans: Vec<Range<usize>>,
}

pub(crate) struct SearchView<'a> {
    pub(crate) text: &'a str,
    pub(crate) cursor: usize,
    pub(crate) input_focused: bool,
    pub(crate) status: String,
    pub(crate) lines: Vec<&'a ResultLine>,
}

fn width_of(text: &str) -> usize {
    text.chars().map(|letter| letter.width().unwrap_or(0)).sum()
}

pub(crate) fn query_line(view: &SearchView, width: usize) -> Line<'static> {
    let prompt_width = width_of(PROMPT);
    let status_width = width_of(&view.status);
    let with_status = width.saturating_sub(prompt_width + STATUS_GAP + status_width);
    let (room, show_status) = if with_status >= MIN_QUERY_ROOM {
        (with_status, true)
    } else {
        (width.saturating_sub(prompt_width), false)
    };
    let letters: Vec<char> = view.text.chars().collect();
    let cursor = view.cursor.min(letters.len());
    let cursor_width = usize::from(view.input_focused) * width_of(CURSOR);
    let mut before = String::new();
    let mut left_room = room.saturating_sub(cursor_width);
    for &letter in letters[..cursor].iter().rev() {
        let letter_width = letter.width().unwrap_or(0);
        if letter_width > left_room {
            break;
        }
        left_room -= letter_width;
        before.insert(0, letter);
    }
    let mut after = String::new();
    for &letter in &letters[cursor..] {
        let letter_width = letter.width().unwrap_or(0);
        if letter_width > left_room {
            break;
        }
        left_room -= letter_width;
        after.push(letter);
    }
    let mut spans = vec![
        Span::styled(PROMPT, theme::search_prompt()),
        Span::raw(before),
    ];
    if view.input_focused {
        spans.push(Span::styled(CURSOR, theme::search_prompt()));
    }
    spans.push(Span::raw(after));
    if show_status {
        spans.push(Span::styled(
            format!("{}{}", " ".repeat(STATUS_GAP), view.status),
            theme::secondary(),
        ));
    }
    Line::from(spans)
}

pub(crate) fn highlighted(
    text: &str,
    spans: &[Range<usize>],
    base: Style,
    hit: Style,
) -> Vec<Span<'static>> {
    let mut parts = Vec::new();
    let mut at = 0;
    for span in spans {
        let end = span.end.min(text.len());
        if span.start < at || span.start >= end || !text.is_char_boundary(span.start) {
            continue;
        }
        if !text.is_char_boundary(end) {
            continue;
        }
        if span.start > at {
            parts.push(Span::styled(text[at..span.start].to_owned(), base));
        }
        parts.push(Span::styled(text[span.start..end].to_owned(), hit));
        at = end;
    }
    if at < text.len() {
        parts.push(Span::styled(text[at..].to_owned(), base));
    }
    parts
}

pub(crate) fn clip_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= width {
        return spans;
    }
    let mut room = width.saturating_sub(width_of(ELLIPSIS));
    let mut clipped = Vec::new();
    let mut last_style = Style::default();
    for span in spans {
        let mut kept = String::new();
        for letter in span.content.chars() {
            let letter_width = letter.width().unwrap_or(0);
            if letter_width > room {
                room = 0;
                break;
            }
            room -= letter_width;
            kept.push(letter);
        }
        let complete = kept.len() == span.content.len();
        if !kept.is_empty() {
            last_style = span.style;
            clipped.push(Span::styled(kept, span.style));
        }
        if !complete {
            break;
        }
    }
    clipped.push(Span::styled(ELLIPSIS, last_style));
    clipped
}

pub(crate) fn window_on_match(snippet: &Snippet, room: usize) -> (String, Vec<Range<usize>>) {
    let text = &snippet.text;
    let Some(first) = snippet.spans.first() else {
        return (text.clone(), Vec::new());
    };
    if width_of(&text[..first.end]) <= room {
        return (text.clone(), snippet.spans.clone());
    }
    let mut cut = first.start;
    let mut lead = 0;
    for (index, letter) in text[..first.start].char_indices().rev() {
        let letter_width = letter.width().unwrap_or(0);
        if lead + letter_width > LEAD_COLUMNS {
            break;
        }
        lead += letter_width;
        cut = index;
    }
    let spans = snippet
        .spans
        .iter()
        .filter(|span| span.start >= cut)
        .map(|span| span.start - cut + ELLIPSIS.len()..span.end - cut + ELLIPSIS.len())
        .collect();
    (format!("{ELLIPSIS}{}", &text[cut..]), spans)
}

pub(crate) fn hits_label(hits: u32) -> String {
    match hits {
        1 => "1 hit".to_owned(),
        many => format!("{many} hits"),
    }
}
