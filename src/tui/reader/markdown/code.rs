use super::renderer::CodeState;
use super::theme::Theme;
use super::wrap::char_width;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const TAB_WIDTH: usize = 4;

pub(super) fn render_code(code: &CodeState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let padding = if width >= 6 { 1 } else { 0 };
    let inner = width - 2 * padding;
    let style = theme.code_block;
    let framed = |content: &str, content_width: usize, content_style: Style| {
        let left = " ".repeat(padding);
        let right = " ".repeat((inner + padding).saturating_sub(content_width));
        Line::from(vec![
            Span::styled(left, style),
            Span::styled(content.to_string(), content_style),
            Span::styled(right, style),
        ])
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
        lines.push(framed(&label, label_width, label_style));
    }

    let text = code.text.strip_suffix('\n').unwrap_or(&code.text);
    for raw in text.split('\n') {
        let mut segment = String::new();
        let mut segment_width = 0;
        let push_char = |c: char,
                         lines: &mut Vec<Line<'static>>,
                         segment: &mut String,
                         segment_width: &mut usize| {
            let c_width = char_width(c);
            if *segment_width > 0 && *segment_width + c_width > inner {
                lines.push(framed(segment, *segment_width, style));
                segment.clear();
                *segment_width = 0;
            }
            segment.push(c);
            *segment_width += c_width;
        };
        for c in raw.chars() {
            if c == '\t' {
                for _ in 0..TAB_WIDTH {
                    push_char(' ', &mut lines, &mut segment, &mut segment_width);
                }
            } else if !c.is_control() {
                push_char(c, &mut lines, &mut segment, &mut segment_width);
            }
        }
        lines.push(framed(&segment, segment_width, style));
    }
    lines
}
