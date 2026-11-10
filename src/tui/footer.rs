use super::{prefix::PREFIX_LABEL, theme, verbosity::Level};
use crate::activity::Summary;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

const TAB_BAR_HINTS: &[(&str, &str)] = &[
    ("←/→", "tab"),
    ("↓", "rows"),
    ("q", "quit"),
];

const LIST_HINTS: &[(&str, &str)] = &[
    ("↑", "tabs"),
    ("→", "open"),
    ("↑/↓", "move"),
    ("x", "hide"),
    ("v", "view"),
    ("< >", "resize"),
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

const ATTACH_KEY: &str = "a";

const TRANSCRIPT_HINTS: &[(&str, &str)] = &[
    ("←/esc", "list"),
    ("↑/↓", "scroll"),
    ("v", "view"),
    ("PgUp/PgDn", "page"),
    ("< >", "resize"),
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
    ("< > =", "resize/reset"),
    (PREFIX_LABEL, "send literal"),
];

const BACK_HINT: (&str, &str) = ("←", "back to list (at start of prompt)");
const WIDE_EMBEDDED_WIDTH: u16 = 100;
const NARROW_BACK_LABEL: &str = "back to list";

const EXITED_HINTS: &[(&str, &str)] = &[("any key", "return to transcript")];

pub struct Extras<'a> {
    pub hidden_note: Option<String>,
    pub harness: Option<&'a str>,
}

pub enum Mode<'a> {
    List,
    ListTabs,
    Transcript,
    Embedded,
    EmbeddedPrefix,
    EmbeddedExited,
    Confirm(&'a str),
    Notice(&'a str),
}

impl Mode<'_> {
    fn label(&self, harness: Option<&str>) -> Option<String> {
        match self {
            Mode::List | Mode::ListTabs => Some("LIST".to_owned()),
            Mode::Transcript => Some("TRANSCRIPT".to_owned()),
            Mode::Embedded | Mode::EmbeddedPrefix | Mode::EmbeddedExited => Some(match harness {
                Some(harness) => format!("SESSION · {harness}"),
                None => "SESSION".to_owned(),
            }),
            Mode::Confirm(_) | Mode::Notice(_) => None,
        }
    }

    fn pins_last_hint(&self) -> bool {
        matches!(self, Mode::List | Mode::ListTabs | Mode::Transcript)
    }
}

pub fn draw(frame: &mut Frame, area: Rect, mode: &Mode, scanning: bool, summary: &Summary, chip: Style, view: Level, attach_available: bool, mouse: bool, extras: Extras) {
    let hints: Vec<(String, String)> = match mode {
        Mode::Confirm(prompt) => return draw_message(frame, area, prompt, true),
        Mode::Notice(notice) => return draw_message(frame, area, notice, false),
        Mode::List => owned(LIST_HINTS, ""),
        Mode::ListTabs => owned(TAB_BAR_HINTS, ""),
        Mode::Transcript => owned(TRANSCRIPT_HINTS, ""),
        Mode::Embedded => embedded_hints(area.width, mouse),
        Mode::EmbeddedPrefix => owned(PREFIX_HINTS, ""),
        Mode::EmbeddedExited => owned(EXITED_HINTS, ""),
    };
    let shows_view = matches!(mode, Mode::List | Mode::ListTabs | Mode::Transcript);
    let hidden_note = extras.hidden_note.filter(|_| shows_view);
    let summary_spans = summary_spans(summary, shows_view.then_some(view), hidden_note);
    let summary_width: usize = summary_spans.iter().map(Span::width).sum();
    let width = (area.width as usize).saturating_sub(summary_width);
    let mut spans = Vec::new();
    let mut used = 1;
    if let Some(label) = mode.label(extras.harness) {
        used += label.chars().count() + 2;
        spans.push(Span::styled(format!(" {label} "), chip));
    }
    spans.push(Span::raw(" "));
    let hints: Vec<(String, String)> = hints.into_iter().filter(|(key, _)| attach_available || key != ATTACH_KEY).collect();
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

fn summary_spans(summary: &Summary, view: Option<Level>, hidden_note: Option<String>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(note) = hidden_note {
        spans.push(Span::styled(note, theme::summary(false)));
    }
    if let Some(view) = view {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme::summary(false)));
        }
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

fn embedded_hints(width: u16, mouse: bool) -> Vec<(String, String)> {
    if width < WIDE_EMBEDDED_WIDTH {
        let mut hints = vec![(BACK_HINT.0.to_owned(), NARROW_BACK_LABEL.to_owned())];
        hints.push((format!("{PREFIX_LABEL} x"), "close".to_owned()));
        return hints;
    }
    let mut hints = vec![(BACK_HINT.0.to_owned(), BACK_HINT.1.to_owned())];
    let mut commands = "x close · [ ] switch · l list · < > resize".to_owned();
    if !mouse {
        commands.push_str(" · PgUp scroll");
    }
    hints.push((format!("{PREFIX_LABEL} then"), commands));
    hints
}
