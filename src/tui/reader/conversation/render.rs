use crate::tui::reader::{
    conversation::turn::{Turn, Unit},
    text::{display_width, wrap},
};
use crate::tui::{
    reader::{tool_block, verbosity::Level},
    style::theme,
};
use crate::{session::transcript::Role, tui::reader::markdown};
use ratatui::text::{Line, Span};

const BULLET: &str = "● ";
const PROMPT_MARKER: &str = "> ";
const INDENT: &str = "  ";
const INDENT_WIDTH: usize = 2;
const THINKING_TITLE: &str = "∴ Thinking…";
const THINKING_PREVIEW_LINES: usize = 3;

pub(super) fn render_turn(
    out: &mut Vec<Line<'static>>,
    turn: &Turn,
    level: Level,
    width: usize,
    markdown_theme: &markdown::Theme,
    cwd: &str,
    shown_day: &mut Option<String>,
) {
    let show_meta = level == Level::Everything;
    let mut dated = false;
    for unit in &turn.units {
        let mut block: Vec<Line<'static>> = Vec::new();
        match unit {
            Unit::Text(_) if turn.meta && !show_meta => {}
            Unit::Text(text) => push_text(&mut block, turn, text, width, markdown_theme),
            Unit::Thinking(_) if level == Level::Chat => {}
            Unit::Thinking(text) => push_thinking(&mut block, text, level, width),
            Unit::Tool {
                name,
                input,
                result,
            } if level != Level::Chat => tool_block::render_call(
                &mut block,
                name,
                input,
                result.as_deref(),
                level,
                width,
                cwd,
            ),
            Unit::OrphanResult(result) if level != Level::Chat => {
                tool_block::render_orphan_result(&mut block, result, level, width)
            }
            Unit::Tool { .. } | Unit::OrphanResult(_) => {}
        }
        if block.is_empty() {
            continue;
        }
        if !dated {
            push_day_divider(out, turn, shown_day);
            dated = true;
        }
        push_gap(out);
        out.extend(block);
    }
}

fn push_gap(out: &mut Vec<Line<'static>>) {
    if out.last().is_some_and(|l| !is_blank(l)) {
        out.push(Line::default());
    }
}

fn push_day_divider(out: &mut Vec<Line<'static>>, turn: &Turn, shown_day: &mut Option<String>) {
    if let Some(stamp) = &turn.stamp
        && shown_day.as_deref() != Some(&stamp.day)
    {
        push_gap(out);
        out.push(Line::styled(
            format!("── {} {} ──", stamp.day, stamp.clock),
            theme::day_divider(),
        ));
        *shown_day = Some(stamp.day.clone());
    }
}

fn push_text(
    body: &mut Vec<Line<'static>>,
    turn: &Turn,
    text: &str,
    width: usize,
    md: &markdown::Theme,
) {
    if turn.meta {
        push_indented(body, "· ", text, width, theme::secondary());
        return;
    }
    match turn.role {
        Role::Assistant => push_assistant(body, text, width, md),
        Role::User => push_prompt(body, text, width),
    }
}

fn push_assistant(body: &mut Vec<Line<'static>>, text: &str, width: usize, md: &markdown::Theme) {
    let body_width = width.saturating_sub(INDENT_WIDTH).max(1);
    for (index, mut line) in markdown::render(text, body_width as u16, md)
        .into_iter()
        .enumerate()
    {
        let lead = if index == 0 {
            Span::styled(BULLET, theme::assistant_bullet())
        } else {
            Span::raw(INDENT)
        };
        line.spans.insert(0, lead);
        body.push(line);
    }
}

fn push_prompt(body: &mut Vec<Line<'static>>, text: &str, width: usize) {
    let text_width = width.saturating_sub(INDENT_WIDTH).max(1);
    let mut first = true;
    for line in text.lines() {
        for piece in wrap(&tool_block::clean(line), text_width) {
            let marker = if first { PROMPT_MARKER } else { INDENT };
            first = false;
            let padding = width.saturating_sub(INDENT_WIDTH + display_width(&piece));
            body.push(Line::from(vec![
                Span::styled(marker, theme::user_prompt_marker()),
                Span::styled(
                    format!("{piece}{}", " ".repeat(padding)),
                    theme::user_prompt(),
                ),
            ]));
        }
    }
}

fn push_indented(
    body: &mut Vec<Line<'static>>,
    first_prefix: &str,
    text: &str,
    width: usize,
    style: ratatui::style::Style,
) {
    let text_width = width.saturating_sub(INDENT_WIDTH).max(1);
    let mut first = true;
    for line in text.lines() {
        for piece in wrap(&tool_block::clean(line), text_width) {
            let prefix = if first { first_prefix } else { INDENT };
            first = false;
            body.push(Line::styled(format!("{prefix}{piece}"), style));
        }
    }
}

fn push_thinking(body: &mut Vec<Line<'static>>, text: &str, level: Level, width: usize) {
    body.push(Line::styled(THINKING_TITLE, theme::thinking()));
    let lines: Vec<&str> = text.lines().collect();
    let shown = match level {
        Level::Everything => lines.len(),
        _ if lines.len() > THINKING_PREVIEW_LINES + 1 => THINKING_PREVIEW_LINES,
        _ => lines.len(),
    };
    for line in &lines[..shown] {
        body.extend(
            wrap(
                &tool_block::clean(line),
                width.saturating_sub(INDENT_WIDTH).max(1),
            )
            .into_iter()
            .map(|piece| Line::styled(format!("{INDENT}{piece}"), theme::thinking())),
        );
    }
    if shown < lines.len() {
        body.push(Line::styled(
            format!("{INDENT}… +{} lines", lines.len() - shown),
            theme::thinking(),
        ));
    }
}

pub(super) fn is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}
