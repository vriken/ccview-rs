use super::{prefix::PREFIX_LABEL, theme, verbosity::Level};
use crate::activity::Summary;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

const LIST_HINTS: &[(&str, &str)] = &[
    ("←/→", "tab"),
    ("↑/↓", "move"),
    ("v", "view"),
    ("r", "resume"),
    ("f", "fork"),
    ("n", "new"),
    ("a", "attach"),
    ("q", "quit"),
    ("c", "continue"),
    ("o/O", "terminal"),
    ("s", "star"),
    ("enter", "open"),
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
    ("q", "quit"),
];

const PREFIX_HINTS: &[(&str, &str)] = &[
    ("l", "list"),
    ("x", "close"),
    ("[ ]", "switch"),
    ("PgUp/PgDn", "scroll"),
    (PREFIX_LABEL, "send literal"),
];

const EMBEDDED_COMMANDS: &[(&str, &str)] = &[
    ("l", "list"),
    ("x", "close"),
    ("[ ]", "switch"),
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

pub fn draw(frame: &mut Frame, area: Rect, mode: &Mode, scanning: bool, summary: &Summary, view: Level) {
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
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for (key, label) in hints {
        let needed = key.chars().count() + label.chars().count() + 4;
        if used + needed > width {
            break;
        }
        spans.push(Span::styled(key.clone(), theme::footer_key()));
        spans.push(Span::styled(format!(" {label}   "), theme::footer_text()));
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
