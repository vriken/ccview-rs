use crate::harness::browser;
use crate::tui::reader::text::display_width;
use crate::tui::style::theme;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::{ops::Range, sync::Arc};

const SCHEMES: [&str; 2] = ["http://", "https://"];
const TRAILING_PUNCTUATION: [char; 6] = ['.', ',', ';', ':', '\'', '"'];
const BRACKET_PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
const NOT_IN_URL: [char; 6] = ['<', '>', '"', '`', '…', '\\'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkSpan {
    pub(crate) columns: Range<usize>,
    pub(crate) url: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRange {
    pub(crate) chars: Range<usize>,
    pub(crate) url: Arc<str>,
}

pub(crate) struct UrlMatch {
    pub(crate) bytes: Range<usize>,
    pub(crate) chars: Range<usize>,
}

#[derive(Default)]
pub(crate) struct LinkTable {
    urls: Vec<Arc<str>>,
}

impl LinkTable {
    pub(crate) fn add(&mut self, url: &str) -> usize {
        self.urls.push(url.into());
        self.urls.len() - 1
    }

    pub(crate) fn tag(style: Style, id: usize) -> Style {
        let [_, high, mid, low] = (id as u32).to_be_bytes();
        style.underline_color(Color::Rgb(high, mid, low))
    }

    fn resolve(&self, color: Color) -> Option<Arc<str>> {
        let Color::Rgb(high, mid, low) = color else {
            return None;
        };
        let id = u32::from_be_bytes([0, high, mid, low]) as usize;
        self.urls.get(id).cloned()
    }
}

pub(crate) fn collect(lines: &mut [Line<'static>], table: &LinkTable) -> Vec<Vec<LinkSpan>> {
    lines
        .iter_mut()
        .map(|line| {
            let mut links: Vec<LinkSpan> = Vec::new();
            let mut column = 0;
            for span in &mut line.spans {
                let width = display_width(&span.content);
                let url = span
                    .style
                    .underline_color
                    .and_then(|color| table.resolve(color));
                if let Some(url) = url {
                    span.style.underline_color = None;
                    match links.last_mut() {
                        Some(last) if last.columns.end == column && last.url == url => {
                            last.columns.end += width;
                        }
                        _ if width > 0 => links.push(LinkSpan {
                            columns: column..column + width,
                            url,
                        }),
                        _ => {}
                    }
                }
                column += width;
            }
            links
        })
        .collect()
}

pub(crate) fn find_urls(text: &str) -> Vec<UrlMatch> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte_at = |index: usize| chars.get(index).map_or(text.len(), |(byte, _)| *byte);
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let end = scheme_length(&chars, at)
            .filter(|_| at == 0 || !chars[at - 1].1.is_alphanumeric())
            .and_then(|scheme| url_end(&chars, at, scheme))
            .filter(|&end| browser::is_openable(&text[byte_at(at)..byte_at(end)]));
        match end {
            Some(end) => {
                found.push(UrlMatch {
                    bytes: byte_at(at)..byte_at(end),
                    chars: at..end,
                });
                at = end;
            }
            None => at += 1,
        }
    }
    found
}

fn scheme_length(chars: &[(usize, char)], at: usize) -> Option<usize> {
    SCHEMES
        .iter()
        .find(|scheme| {
            let candidate: String = chars
                .iter()
                .skip(at)
                .take(scheme.len())
                .map(|(_, c)| *c)
                .collect();
            candidate.eq_ignore_ascii_case(scheme)
        })
        .map(|scheme| scheme.len())
}

fn url_end(chars: &[(usize, char)], at: usize, scheme: usize) -> Option<usize> {
    let first = at + scheme;
    let mut end = first;
    while end < chars.len() && is_url_char(chars[end].1) {
        end += 1;
    }
    while end > first {
        let last = chars[end - 1].1;
        let unbalanced_closer = BRACKET_PAIRS.iter().any(|&(open, close)| {
            last == close && count(&chars[at..end], close) > count(&chars[at..end], open)
        });
        if TRAILING_PUNCTUATION.contains(&last) || unbalanced_closer {
            end -= 1;
        } else {
            break;
        }
    }
    (end > first).then_some(end)
}

fn count(chars: &[(usize, char)], wanted: char) -> usize {
    chars.iter().filter(|(_, c)| *c == wanted).count()
}

fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !NOT_IN_URL.contains(&c)
}

pub(crate) fn url_at(text: &str, char_index: usize) -> Option<&str> {
    find_urls(text)
        .into_iter()
        .find(|found| found.chars.contains(&char_index))
        .map(|found| &text[found.bytes])
}

pub(crate) fn link_ranges(text: &str) -> Vec<LinkRange> {
    find_urls(text)
        .into_iter()
        .map(|found| LinkRange {
            url: text[found.bytes].into(),
            chars: found.chars,
        })
        .collect()
}

pub(crate) fn clip(ranges: &[LinkRange], from: usize, to: usize, offset: usize) -> Vec<LinkRange> {
    ranges
        .iter()
        .filter_map(|range| {
            let start = range.chars.start.max(from);
            let end = range.chars.end.min(to);
            (start < end).then(|| LinkRange {
                chars: start - from + offset..end - from + offset,
                url: range.url.clone(),
            })
        })
        .collect()
}

pub(crate) fn spans(
    text: &str,
    links: &[LinkRange],
    base: Style,
    table: &mut LinkTable,
) -> Vec<Span<'static>> {
    let link_style = base.patch(theme::link());
    let chars: Vec<char> = text.chars().collect();
    let slice = |range: Range<usize>| chars[range].iter().collect::<String>();
    let mut spans = Vec::new();
    let mut at = 0;
    for link in links {
        if link.chars.start > at {
            spans.push(Span::styled(slice(at..link.chars.start), base));
        }
        let id = table.add(&link.url);
        spans.push(Span::styled(
            slice(link.chars.clone()),
            LinkTable::tag(link_style, id),
        ));
        at = link.chars.end;
    }
    if at < chars.len() {
        spans.push(Span::styled(slice(at..chars.len()), base));
    }
    spans
}

pub(crate) fn line(
    text: &str,
    links: &[LinkRange],
    style: Style,
    table: &mut LinkTable,
) -> Line<'static> {
    if links.is_empty() {
        return Line::styled(text.to_owned(), style);
    }
    Line::from(spans(text, links, Style::default(), table)).style(style)
}
