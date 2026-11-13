use crate::catalog::search::{query::Term, snippet::matched_spans};
use crate::tui::style::theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

pub(crate) struct Highlight {
    words: Vec<Term>,
}

impl Highlight {
    pub(crate) fn of(terms: &[Term]) -> Self {
        let mut words = Vec::new();
        for term in terms {
            let last = term.folded.len().saturating_sub(1);
            for (at, folded) in term.folded.iter().enumerate() {
                words.push(Term {
                    raw: Vec::new(),
                    folded: vec![folded.clone()],
                    prefix: term.prefix && at == last,
                });
            }
        }
        Self { words }
    }
}

pub(super) fn apply(lines: &mut [Line<'static>], highlight: &Highlight) {
    for line in lines {
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let matches = matched_spans(&text, &highlight.words);
        if !matches.is_empty() {
            line.spans = restyled(&line.spans, &matches, theme::search_match());
        }
    }
}

fn restyled(
    spans: &[Span<'static>],
    matches: &[std::ops::Range<usize>],
    mark: Style,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut start = 0;
    for span in spans {
        let end = start + span.content.len();
        let mut at = 0;
        for matched in matches.iter().filter(|m| m.start < end && m.end > start) {
            let from = matched.start.saturating_sub(start);
            let to = (matched.end - start).min(span.content.len());
            if from > at {
                out.push(Span::styled(span.content[at..from].to_owned(), span.style));
            }
            out.push(Span::styled(
                span.content[from..to].to_owned(),
                span.style.patch(mark),
            ));
            at = to;
        }
        if at < span.content.len() {
            out.push(Span::styled(span.content[at..].to_owned(), span.style));
        }
        start = end;
    }
    out
}
