use crate::session::activity::{Activity, badges};
use crate::tui::style::theme;
use ratatui::text::Span;

pub fn spans(activity: &Activity) -> Vec<Span<'static>> {
    badges(activity)
        .into_iter()
        .flat_map(|badge| {
            [
                Span::styled(badge.text(), theme::badge(&badge)),
                Span::raw(" "),
            ]
        })
        .collect()
}

pub fn width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}
