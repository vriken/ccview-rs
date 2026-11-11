use super::renderer::CodeState;
use super::theme::Theme;
use super::wrap::char_width;
use crate::tui::reader::links::{self, LinkTable};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

const TAB_WIDTH: usize = 4;

type StyledChar = (char, Option<usize>);

pub(super) fn render_code(
    code: &CodeState,
    width: usize,
    theme: &Theme,
    links: &mut LinkTable,
) -> Vec<Line<'static>> {
    let padding = if width >= 6 { 1 } else { 0 };
    let inner = width - 2 * padding;
    let style = theme.code_block;
    let link_style = style.patch(theme.link);
    let framed = |content: Vec<Span<'static>>, content_width: usize| {
        let left = " ".repeat(padding);
        let right = " ".repeat((inner + padding).saturating_sub(content_width));
        let mut spans = vec![Span::styled(left, style)];
        spans.extend(content);
        spans.push(Span::styled(right, style));
        Line::from(spans)
    };
    let styled = |segment: &[StyledChar]| -> Vec<Span<'static>> {
        segment
            .chunk_by(|a, b| a.1 == b.1)
            .map(|run| {
                let text: String = run.iter().map(|(c, _)| *c).collect();
                match run[0].1 {
                    Some(id) => Span::styled(text, LinkTable::tag(link_style, id)),
                    None => Span::styled(text, style),
                }
            })
            .collect()
    };

    let mut lines = Vec::new();
    if !code.language.is_empty() {
        let mut label = String::new();
        let mut label_width = 0;
        for c in code.language.chars().filter(|c| !c.is_control()) {
            if label_width + char_width(c) > inner {
                break;
            }
            label.push(c);
            label_width += char_width(c);
        }
        let label_style = style.patch(theme.dim).add_modifier(Modifier::ITALIC);
        lines.push(framed(vec![Span::styled(label, label_style)], label_width));
    }

    let text = code.text.strip_suffix('\n').unwrap_or(&code.text);
    for raw in text.split('\n') {
        let expanded: String = raw
            .chars()
            .flat_map(|c| match c {
                '\t' => vec![' '; TAB_WIDTH],
                c if c.is_control() => Vec::new(),
                c => vec![c],
            })
            .collect();
        let mut link_at: Vec<Option<usize>> = vec![None; expanded.chars().count()];
        for found in links::find_urls(&expanded) {
            let id = links.add(&expanded[found.bytes]);
            link_at[found.chars].fill(Some(id));
        }
        let mut segment: Vec<StyledChar> = Vec::new();
        let mut segment_width = 0;
        for (c, link) in expanded.chars().zip(link_at) {
            let c_width = char_width(c);
            if segment_width > 0 && segment_width + c_width > inner {
                lines.push(framed(styled(&segment), segment_width));
                segment.clear();
                segment_width = 0;
            }
            segment.push((c, link));
            segment_width += c_width;
        }
        lines.push(framed(styled(&segment), segment_width));
    }
    lines
}
