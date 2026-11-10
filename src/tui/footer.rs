use super::{prefix::PREFIX_LABEL, theme};
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
    ("< >", "resize"),
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
    ("PgUp/PgDn", "page"),
    ("< >", "resize"),
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
    ("< > =", "resize/reset"),
    (PREFIX_LABEL, "send literal"),
];

const BACK_HINT: (&str, &str) = ("←", "back to list (at start of prompt)");
const WIDE_EMBEDDED_WIDTH: u16 = 100;

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

pub fn draw(frame: &mut Frame, area: Rect, mode: &Mode, scanning: bool, summary: &Summary, mouse: bool) {
    let hints: Vec<(String, String)> = match mode {
        Mode::Confirm(prompt) => return draw_message(frame, area, prompt, true),
        Mode::Notice(notice) => return draw_message(frame, area, notice, false),
        Mode::List => with_drag_hint(owned(LIST_HINTS, ""), mouse),
        Mode::Transcript => with_drag_hint(owned(TRANSCRIPT_HINTS, ""), mouse),
        Mode::Embedded => embedded_hints(area.width, mouse),
        Mode::EmbeddedPrefix => owned(PREFIX_HINTS, ""),
        Mode::EmbeddedExited => owned(EXITED_HINTS, ""),
    };
    let summary_spans = summary_spans(summary);
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

fn summary_spans(summary: &Summary) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, segment) in summary.segments().into_iter().enumerate() {
        if index > 0 {
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

fn with_drag_hint(mut hints: Vec<(String, String)>, mouse: bool) -> Vec<(String, String)> {
    if mouse && let Some(position) = hints.iter().position(|(_, label)| label == "resize") {
        hints.insert(position + 1, ("drag".to_owned(), "divider".to_owned()));
    }
    hints
}

fn embedded_hints(width: u16, mouse: bool) -> Vec<(String, String)> {
    let mut hints = vec![(BACK_HINT.0.to_owned(), BACK_HINT.1.to_owned())];
    if width < WIDE_EMBEDDED_WIDTH {
        hints.push((format!("{PREFIX_LABEL} x"), "close".to_owned()));
        return hints;
    }
    let mut commands = "x close · [ ] switch · l list · < > resize".to_owned();
    if !mouse {
        commands.push_str(" · PgUp scroll");
    }
    hints.push((format!("{PREFIX_LABEL} then"), commands));
    if mouse {
        hints.push(("drag".to_owned(), "divider".to_owned()));
    }
    hints
}
