use crate::session::activity::Summary;
use crate::tui::{
    bindings::{self, HELP_KEY, Zone},
    embedded::prefix::PREFIX_LABEL,
    reader::verbosity::Level,
    style::theme,
};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

const ATTACH_KEY: &str = "a";
const PINNED_HINTS: usize = 2;
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

    fn pinned_hints(&self) -> usize {
        match self {
            Mode::List | Mode::ListTabs | Mode::Transcript => PINNED_HINTS,
            _ => 0,
        }
    }
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    mode: &Mode,
    scanning: bool,
    summary: &Summary,
    chip: Style,
    view: Level,
    attach_available: bool,
    mouse: bool,
    extras: Extras,
) {
    let hints: Vec<(String, String)> = match mode {
        Mode::Confirm(prompt) => return draw_message(frame, area, prompt, true),
        Mode::Notice(notice) => return draw_message(frame, area, notice, false),
        Mode::List => zone_hints(Zone::ListRows, false),
        Mode::ListTabs => zone_hints(Zone::TabBar, false),
        Mode::Transcript => zone_hints(Zone::Transcript, false),
        Mode::Embedded => embedded_hints(area.width, mouse),
        Mode::EmbeddedPrefix => zone_hints(Zone::Prefix, true),
        Mode::EmbeddedExited => EXITED_HINTS
            .iter()
            .map(|(key, label)| ((*key).to_owned(), (*label).to_owned()))
            .collect(),
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
    let hints: Vec<(String, String)> = hints
        .into_iter()
        .filter(|(key, _)| attach_available || key != ATTACH_KEY)
        .collect();
    let hint_width =
        |(key, label): &(String, String)| key.chars().count() + label.chars().count() + 4;
    let first_pinned = hints.len().saturating_sub(mode.pinned_hints());
    let pinned_width: usize = hints[first_pinned..].iter().map(hint_width).sum();
    let budget = width.saturating_sub(pinned_width);
    let mut full = false;
    for (index, hint) in hints.iter().enumerate() {
        let is_pinned = index >= first_pinned;
        let needed = hint_width(hint);
        if !is_pinned && (full || used + needed > budget) {
            full = true;
            continue;
        }
        spans.push(Span::styled(hint.0.clone(), theme::footer_key()));
        spans.push(Span::styled(
            format!(" {}   ", hint.1),
            theme::footer_text(),
        ));
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

fn summary_spans(
    summary: &Summary,
    view: Option<Level>,
    hidden_note: Option<String>,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(note) = hidden_note {
        spans.push(Span::styled(note, theme::summary(false)));
    }
    if let Some(view) = view {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme::summary(false)));
        }
        spans.push(Span::styled(
            format!("view: {}", view.label()),
            theme::summary(false),
        ));
    }
    for segment in summary.segments() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme::summary(false)));
        }
        spans.push(Span::styled(
            segment.text,
            theme::summary(segment.attention),
        ));
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

fn zone_hints(zone: Zone, all: bool) -> Vec<(String, String)> {
    bindings::zone_keys(zone)
        .filter(|hint| all || hint.essential)
        .map(|hint| (hint.keys.to_owned(), hint.label.to_owned()))
        .collect()
}

fn embedded_hints(width: u16, mouse: bool) -> Vec<(String, String)> {
    let back = bindings::essential(Zone::Embedded)
        .next()
        .map_or(("←", "back to list"), |hint| (hint.keys, hint.label));
    if width < WIDE_EMBEDDED_WIDTH {
        return vec![
            (back.0.to_owned(), NARROW_BACK_LABEL.to_owned()),
            (format!("{PREFIX_LABEL} {HELP_KEY}"), "help".to_owned()),
        ];
    }
    let mut commands: Vec<String> = bindings::essential(Zone::Prefix)
        .map(|hint| format!("{} {}", hint.keys, hint.label))
        .collect();
    if !mouse {
        commands.push("PgUp scroll".to_owned());
    }
    vec![
        (
            back.0.to_owned(),
            format!("{} (at start of prompt)", back.1),
        ),
        (format!("{PREFIX_LABEL} then"), commands.join(" · ")),
    ]
}
