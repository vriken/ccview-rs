use super::{prefix::PREFIX_LABEL, theme, verbosity::Level};
use crate::activity::Summary;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

const LIST_HINTS: &[(&str, &str)] = &[
    ("←/→", "tab"),
    ("↑/↓", "move"),
    ("enter", "open"),
    ("v", "view"),
    ("r", "resume"),
    ("n", "new"),
    ("s", "star"),
    ("C", "colour"),
    ("f", "fork"),
    ("a", "attach"),
    ("c", "continue"),
    ("o/O", "terminal"),
    ("q", "quit"),
];

const TRANSCRIPT_HINTS: &[(&str, &str)] = &[
    ("←/esc", "list"),
    ("↑/↓", "scroll"),
    ("v", "view"),
    ("PgUp/PgDn", "page"),
    ("g/G", "top/end"),
    ("r", "resume"),
    ("f", "fork"),
    ("o", "terminal"),
    ("C", "colour"),
    ("q", "quit"),
];

const PREFIX_HINTS: &[(&str, &str)] = &[
    ("l", "list"),
    ("x", "close"),
    ("[ ]", "switch"),
    ("C", "colour"),
    ("PgUp/PgDn", "scroll"),
    (PREFIX_LABEL, "send literal"),
];

const EMBEDDED_COMMANDS: &[(&str, &str)] = &[
    ("l", "list"),
    ("x", "close"),
    ("[ ]", "switch"),
    ("C", "colour"),
    ("PgUp", "scroll"),
];

const EXITED_HINTS: &[(&str, &str)] = &[("any key", "return to transcript")];

pub enum Mode<'a> {
    List,
    Transcript,
    Embedded,
    EmbeddedPrefix,
    EmbeddedExited,
    Confirm(&'a str),
    Notice(&'a str),
}

impl Mode<'_> {
    fn label(&self) -> Option<&'static str> {
        match self {
            Mode::List => Some("LIST"),
            Mode::Transcript => Some("TRANSCRIPT"),
            Mode::Embedded | Mode::EmbeddedPrefix | Mode::EmbeddedExited => Some("SESSION"),
            Mode::Confirm(_) | Mode::Notice(_) => None,
        }
    }

    fn pins_last_hint(&self) -> bool {
        matches!(self, Mode::List | Mode::Transcript)
    }
}

pub fn draw(frame: &mut Frame, area: Rect, mode: &Mode, scanning: bool, summary: &Summary, chip: Style, view: Level) {
    let hints: Vec<(String, String)> = match mode {
        Mode::Confirm(prompt) => return draw_message(frame, area, prompt, true),
        Mode::Notice(notice) => return draw_message(frame, area, notice, false),
        Mode::List => owned(LIST_HINTS, ""),
        Mode::Transcript => owned(TRANSCRIPT_HINTS, ""),
        Mode::Embedded => owned(EMBEDDED_COMMANDS, PREFIX_LABEL),
        Mode::EmbeddedPrefix => owned(PREFIX_HINTS, ""),
        Mode::EmbeddedExited => owned(EXITED_HINTS, ""),
    };
    let shows_view = matches!(mode, Mode::List | Mode::Transcript);
    let summary_spans = summary_spans(summary, shows_view.then_some(view));
    let summary_width: usize = summary_spans.iter().map(Span::width).sum();
    let width = (area.width as usize).saturating_sub(summary_width);
    let mut spans = Vec::new();
    let mut used = 1;
    if let Some(label) = mode.label() {
        spans.push(Span::styled(format!(" {label} "), chip));
        used += label.len() + 2;
    }
    spans.push(Span::raw(" "));
    let hint_width = |(key, label): &(String, String)| key.chars().count() + label.chars().count() + 4;
    let pinned = if mode.pins_last_hint() { hints.last().map(hint_width) } else { None };
    let last = hints.len().saturating_sub(1);
    let budget = width.saturating_sub(pinned.unwrap_or(0));
    let mut full = false;
    for (index, hint) in hints.iter().enumerate() {
        let is_pinned = pinned.is_some() && index == last;
        let needed = hint_width(hint);
        if !is_pinned && (full || used + needed > budget) {
            full = true;
            continue;
        }
        spans.push(Span::styled(hint.0.clone(), theme::footer_key()));
        spans.push(Span::styled(format!(" {}   ", hint.1), theme::footer_text()));
        used += needed;
    }
    if scanning && used + 12 <= width {
        spans.push(Span::styled("scanning…", theme::footer_text()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    if !summary_spans.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(summary_spans)).alignment(Alignment::Right),
            area,
        );
    }
}

fn summary_spans(summary: &Summary, view: Option<Level>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(view) = view {
        spans.push(Span::styled(format!("view: {}", view.label()), theme::summary(false)));
    }
    for segment in summary.segments() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme::summary(false)));
        }
        spans.push(Span::styled(segment.text, theme::summary(segment.attention)));
    }
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans
}

fn draw_message(frame: &mut Frame, area: Rect, text: &str, with_answer_keys: bool) {
    let mut spans = vec![Span::styled(format!(" {text}"), theme::notice())];
    if with_answer_keys {
        spans.push(Span::styled("  y", theme::footer_key()));
        spans.push(Span::styled(" yes  ", theme::footer_text()));
        spans.push(Span::styled("n", theme::footer_key()));
        spans.push(Span::styled(" no", theme::footer_text()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn owned(hints: &[(&str, &str)], key_prefix: &str) -> Vec<(String, String)> {
    hints
        .iter()
        .map(|(key, label)| {
            let key = if key_prefix.is_empty() {
                (*key).to_owned()
            } else {
                format!("{key_prefix} {key}")
            };
            (key, (*label).to_owned())
        })
        .collect()
}
