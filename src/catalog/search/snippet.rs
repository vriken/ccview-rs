use super::query::{Term, fold};
use std::ops::Range;

const BEFORE_CHARS: usize = 40;
const AFTER_CHARS: usize = 100;
const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snippet {
    pub(crate) text: String,
    pub(crate) spans: Vec<Range<usize>>,
}

struct Word {
    range: Range<usize>,
    folded: String,
}

pub(crate) fn snippet(text: &str, terms: &[Term]) -> Snippet {
    let spans = matched_spans(text, terms);
    let anchor = spans.first().map_or(0, |span| span.start);
    let from = anchor
        - text[..anchor]
            .chars()
            .rev()
            .take(BEFORE_CHARS)
            .map(char::len_utf8)
            .sum::<usize>();
    let to = anchor
        + text[anchor..]
            .chars()
            .take(AFTER_CHARS)
            .map(char::len_utf8)
            .sum::<usize>();
    let mut line = String::new();
    if from > 0 {
        line.push_str(ELLIPSIS);
    }
    let lead = line.len();
    let mut positions = Vec::new();
    for (offset, letter) in text[from..to].char_indices() {
        positions.push((from + offset, line.len()));
        if letter.is_whitespace() || letter.is_control() {
            if line.len() > lead && !line.ends_with(' ') {
                line.push(' ');
            }
        } else {
            line.push(letter);
        }
    }
    positions.push((to, line.len()));
    line.truncate(line.trim_end().len());
    let spans = spans_in_line(&spans, &positions, line.len(), from..to);
    if to < text.len() {
        line.push_str(ELLIPSIS);
    }
    Snippet { text: line, spans }
}

fn spans_in_line(
    spans: &[Range<usize>],
    positions: &[(usize, usize)],
    line_len: usize,
    window: Range<usize>,
) -> Vec<Range<usize>> {
    let locate = |original: usize| {
        let index = positions.partition_point(|(position, _)| *position < original);
        positions[index.min(positions.len() - 1)].1.min(line_len)
    };
    spans
        .iter()
        .filter(|span| span.start < window.end && span.end > window.start)
        .map(|span| locate(span.start.max(window.start))..locate(span.end.min(window.end)))
        .filter(|span| span.start < span.end)
        .collect()
}

pub(crate) fn matched_spans(text: &str, terms: &[Term]) -> Vec<Range<usize>> {
    let words = words(text);
    let mut spans: Vec<Range<usize>> = Vec::new();
    for term in terms {
        let count = term.folded.len();
        if count == 0 || words.len() < count {
            continue;
        }
        for start in 0..=words.len() - count {
            let run = &words[start..start + count];
            let matches = run
                .iter()
                .zip(&term.folded)
                .enumerate()
                .all(|(at, (word, wanted))| {
                    if term.prefix && at + 1 == count {
                        word.folded.starts_with(wanted.as_str())
                    } else {
                        word.folded == *wanted
                    }
                });
            if matches {
                spans.push(run[0].range.start..run[count - 1].range.end);
            }
        }
    }
    spans.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    merged
}

fn words(text: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    for (index, letter) in text.char_indices() {
        match (letter.is_alphanumeric(), start) {
            (true, None) => start = Some(index),
            (false, Some(first)) => {
                words.push(word(text, first..index));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(first) = start {
        words.push(word(text, first..text.len()));
    }
    words
}

fn word(text: &str, range: Range<usize>) -> Word {
    Word {
        folded: fold(&text[range.clone()]),
        range,
    }
}
