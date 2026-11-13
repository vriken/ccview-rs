use crate::tui::reader::{
    conversation::{
        highlight::{self, Highlight},
        turn::{Turn, Unit},
    },
    links::{self, LinkSpan, LinkTable},
    text::{display_width, wrap},
};
use crate::tui::{
    reader::{
        tool_block::{self, Call},
        verbosity::Level,
    },
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

pub(super) struct Output<'a> {
    pub(crate) lines: &'a mut Vec<Line<'static>>,
    pub(crate) links: &'a mut Vec<Vec<LinkSpan>>,
    pub(crate) highlight: Option<&'a Highlight>,
}

pub(super) fn render_turn(
    output: Output,
    turn: &Turn,
    level: Level,
    width: usize,
    markdown_theme: &markdown::Theme,
    cwd: &str,
    shown_day: &mut Option<String>,
) {
    let Output {
        lines: out,
        links: out_links,
        highlight,
    } = output;
    let show_meta = level == Level::Everything;
    let mut dated = false;
    for unit in &turn.units {
        let mut block: Vec<Line<'static>> = Vec::new();
        let mut table = LinkTable::default();
        match unit {
            Unit::Text(_) if turn.meta && !show_meta => {}
            Unit::Text(text) => {
                push_text(&mut block, &mut table, turn, text, width, markdown_theme)
            }
            Unit::Thinking(_) if level == Level::Chat => {}
            Unit::Thinking(text) => push_thinking(&mut block, &mut table, text, level, width),
            Unit::Tool {
                name,
                input,
                result,
            } if level != Level::Chat => tool_block::render_call(
                &mut block,
                &Call {
                    name,
                    input,
                    result: result.as_deref(),
                },
                level,
                width,
                cwd,
                &mut table,
            ),
            Unit::OrphanResult(result) if level != Level::Chat => {
                tool_block::render_orphan_result(&mut block, result, level, width, &mut table)
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
        out_links.resize_with(out.len(), Vec::new);
        out_links.extend(links::collect(&mut block, &table));
        if let Some(highlight) = highlight {
            highlight::apply(&mut block, highlight);
        }
        out.extend(block);
    }
    out_links.resize_with(out.len(), Vec::new);
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
    table: &mut LinkTable,
    turn: &Turn,
    text: &str,
    width: usize,
    md: &markdown::Theme,
) {
    if turn.meta {
        push_indented(body, table, "· ", text, width, theme::secondary());
        return;
    }
    match turn.role {
        Role::Assistant => push_assistant(body, table, text, width, md),
        Role::User => push_prompt(body, table, text, width),
    }
}

fn push_assistant(
    body: &mut Vec<Line<'static>>,
    table: &mut LinkTable,
    text: &str,
    width: usize,
    md: &markdown::Theme,
) {
    let body_width = width.saturating_sub(INDENT_WIDTH).max(1);
    for (index, mut line) in markdown::render(text, body_width as u16, md, table)
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

fn push_prompt(body: &mut Vec<Line<'static>>, table: &mut LinkTable, text: &str, width: usize) {
    let text_width = width.saturating_sub(INDENT_WIDTH).max(1);
    let mut first = true;
    for line in text.lines() {
        for piece in wrap(&tool_block::clean(line), text_width) {
            let marker = if first { PROMPT_MARKER } else { INDENT };
            first = false;
            let padding = width.saturating_sub(INDENT_WIDTH + display_width(&piece.text));
            let mut spans = vec![Span::styled(marker, theme::user_prompt_marker())];
            spans.extend(links::spans(
                &piece.text,
                &piece.links,
                theme::user_prompt(),
                table,
            ));
            spans.push(Span::styled(" ".repeat(padding), theme::user_prompt()));
            body.push(Line::from(spans));
        }
    }
}

fn push_indented(
    body: &mut Vec<Line<'static>>,
    table: &mut LinkTable,
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
            let linked = shifted(&piece.links, prefix);
            body.push(links::line(
                &format!("{prefix}{}", piece.text),
                &linked,
                style,
                table,
            ));
        }
    }
}

fn push_thinking(
    body: &mut Vec<Line<'static>>,
    table: &mut LinkTable,
    text: &str,
    level: Level,
    width: usize,
) {
    body.push(Line::styled(THINKING_TITLE, theme::thinking()));
    let lines: Vec<&str> = text.lines().collect();
    let shown = match level {
        Level::Everything => lines.len(),
        _ if lines.len() > THINKING_PREVIEW_LINES + 1 => THINKING_PREVIEW_LINES,
        _ => lines.len(),
    };
    for line in &lines[..shown] {
        for piece in wrap(
            &tool_block::clean(line),
            width.saturating_sub(INDENT_WIDTH).max(1),
        ) {
            let linked = shifted(&piece.links, INDENT);
            body.push(links::line(
                &format!("{INDENT}{}", piece.text),
                &linked,
                theme::thinking(),
                table,
            ));
        }
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

fn shifted(ranges: &[links::LinkRange], prefix: &str) -> Vec<links::LinkRange> {
    links::clip(ranges, 0, usize::MAX, prefix.chars().count())
}
