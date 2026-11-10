use crate::tui::reader::{
    conversation::turn::{Turn, Unit},
    text::wrap,
};
use crate::tui::{
    reader::{tool_block, verbosity::Level},
    style::theme,
};
use crate::{session::transcript::Role, tui::reader::markdown};
use ratatui::text::{Line, Span};

pub(super) fn render_turn(
    out: &mut Vec<Line<'static>>,
    turn: &Turn,
    level: Level,
    width: usize,
    markdown_theme: &markdown::Theme,
    shown_day: &mut Option<String>,
) {
    let mut body: Vec<Line<'static>> = Vec::new();
    let show_meta = level == Level::Everything;
    let mut headed = false;
    for unit in &turn.units {
        match unit {
            Unit::Text(_) if turn.meta && !show_meta => {}
            Unit::Text(text) => {
                headed = true;
                push_text(&mut body, turn, text, width, markdown_theme);
            }
            Unit::Thinking(text) if level == Level::Everything => {
                headed = true;
                push_thinking(&mut body, text, width);
            }
            Unit::Thinking(_) => {}
            Unit::Tool {
                name,
                input,
                result,
            } if level != Level::Chat => {
                tool_block::render_call(&mut body, name, input, result.as_deref(), level, width)
            }
            Unit::OrphanResult(result) if level != Level::Chat => {
                tool_block::render_orphan_result(&mut body, result, level, width)
            }
            Unit::Tool { .. } | Unit::OrphanResult(_) => {}
        }
    }
    if body.is_empty() {
        return;
    }
    if headed {
        push_header(out, turn, shown_day);
        out.extend(body);
        out.push(Line::default());
    } else {
        out.extend(body);
    }
}

pub(super) fn push_header(
    out: &mut Vec<Line<'static>>,
    turn: &Turn,
    shown_day: &mut Option<String>,
) {
    if let Some(stamp) = &turn.stamp
        && shown_day.as_deref() != Some(&stamp.day)
    {
        if out.last().is_some_and(|l| !is_blank(l)) {
            out.push(Line::default());
        }
        out.push(Line::styled(
            format!("── {} ──", stamp.day),
            theme::day_divider(),
        ));
        *shown_day = Some(stamp.day.clone());
    }
    if out.last().is_some_and(|l| !is_blank(l)) {
        out.push(Line::default());
    }
    let (label, style) = match (turn.role, turn.meta) {
        (_, true) => ("SYSTEM", theme::system_label()),
        (Role::User, false) => ("USER", theme::user_label()),
        (Role::Assistant, false) => ("ASSISTANT", theme::assistant_label()),
    };
    let clock = turn.stamp.as_ref().map_or("", |s| s.clock.as_str());
    out.push(Line::styled(format!("{label}  {clock}"), style));
}

pub(super) fn push_text(
    body: &mut Vec<Line<'static>>,
    turn: &Turn,
    text: &str,
    width: usize,
    md: &markdown::Theme,
) {
    if turn.meta {
        for line in text.lines() {
            body.extend(
                wrap(&format!("  {}", tool_block::clean(line)), width)
                    .into_iter()
                    .map(|p| Line::styled(p, theme::secondary())),
            );
        }
        return;
    }
    match turn.role {
        Role::Assistant => {
            let gutter = "  ";
            let body_width = width.saturating_sub(gutter.len()).max(1);
            for mut line in markdown::render(text, body_width as u16, md) {
                line.spans
                    .insert(0, Span::styled(gutter, theme::assistant_text()));
                body.push(line);
            }
        }
        Role::User => {
            for line in text.lines() {
                body.extend(
                    wrap(&format!("▌ {line}"), width)
                        .into_iter()
                        .map(|piece| Line::styled(piece, theme::user_text())),
                );
            }
        }
    }
}

pub(super) fn push_thinking(body: &mut Vec<Line<'static>>, text: &str, width: usize) {
    body.push(Line::styled("  ∴ thinking", theme::thinking()));
    for line in text.lines() {
        body.extend(
            wrap(&format!("    {}", tool_block::clean(line)), width)
                .into_iter()
                .map(|piece| Line::styled(piece, theme::thinking())),
        );
    }
}

pub(super) fn is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}
