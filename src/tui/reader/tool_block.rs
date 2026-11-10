use crate::tui::reader::text::display_width;
use crate::tui::{
    reader::{
        diff::{self, Change, Shown},
        verbosity::Level,
    },
    style::theme,
};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use unicode_width::UnicodeWidthChar;

const BULLET: &str = "● ";
const BULLET_WIDTH: usize = 2;
const FIRST_ROW_PREFIX: &str = "  ⎿  ";
const NEXT_ROW_PREFIX: &str = "     ";
const DETAIL_PREFIX: &str = "    ";
const PREFIX_WIDTH: usize = 5;
const SUMMARY_WIDTH: usize = 80;
const TAB_WIDTH: usize = 4;
const DIFF_CONTEXT_LINES: usize = 1;
const SUMMARY_KEYS: [&str; 8] = [
    "command",
    "file_path",
    "notebook_path",
    "path",
    "pattern",
    "url",
    "description",
    "query",
];
const SELF_DESCRIBING_TOOLS: [&str; 10] = [
    "Bash",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Grep",
    "Glob",
    "WebFetch",
    "WebSearch",
];
const DELEGATING_TOOLS: [&str; 2] = ["Agent", "Task"];

struct Caps {
    result_lines: usize,
    diff_rows: usize,
    detail_lines: usize,
}

fn caps(level: Level) -> Caps {
    match level {
        Level::Everything => Caps {
            result_lines: 40,
            diff_rows: 80,
            detail_lines: 40,
        },
        _ => Caps {
            result_lines: 6,
            diff_rows: 14,
            detail_lines: 0,
        },
    }
}

pub fn render_call(
    out: &mut Vec<Line<'static>>,
    name: &str,
    input: &Value,
    result: Option<&str>,
    level: Level,
    width: usize,
) {
    let caps = caps(level);
    let header = format!("{name}({})", summarize(name, input));
    let (name_part, args_part) =
        split_header(&truncate(&header, width.saturating_sub(BULLET_WIDTH)), name);
    out.push(Line::from(vec![
        Span::styled(BULLET, theme::tool_bullet()),
        Span::styled(name_part, theme::tool_name()),
        Span::styled(args_part, theme::tool_args()),
    ]));
    if level == Level::Everything {
        for line in detail_lines(name, input)
            .into_iter()
            .take(caps.detail_lines)
        {
            for piece in fit(&line, width.saturating_sub(DETAIL_PREFIX.len()), true) {
                out.push(Line::styled(
                    format!("{DETAIL_PREFIX}{piece}"),
                    theme::tool_args(),
                ));
            }
        }
    }
    let row_width = width.saturating_sub(PREFIX_WIDTH).max(1);
    let mut rows = Vec::new();
    let edit = edit_changes(input);
    if let Some(changes) = &edit {
        diff_rows(&mut rows, changes, &caps, row_width);
    }
    match result {
        None => rows.push(Line::styled("…", theme::tool_connector())),
        Some(text) => {
            let shown = match (&edit, level) {
                (Some(_), Level::Everything) => true,
                (Some(_), _) => looks_like_error(text),
                (None, _) => true,
            };
            if shown {
                result_rows(&mut rows, name, text, level, &caps, row_width);
            }
        }
    }
    attach_connector(out, rows);
}

pub fn render_orphan_result(
    out: &mut Vec<Line<'static>>,
    result: &str,
    level: Level,
    width: usize,
) {
    let mut rows = Vec::new();
    result_rows(
        &mut rows,
        "",
        result,
        level,
        &caps(level),
        width.saturating_sub(PREFIX_WIDTH).max(1),
    );
    attach_connector(out, rows);
}

fn attach_connector(out: &mut Vec<Line<'static>>, rows: Vec<Line<'static>>) {
    for (index, mut row) in rows.into_iter().enumerate() {
        let (prefix, style) = if index == 0 {
            (FIRST_ROW_PREFIX, theme::tool_connector())
        } else {
            (NEXT_ROW_PREFIX, Style::default())
        };
        row.spans.insert(0, Span::styled(prefix, style));
        out.push(row);
    }
}

fn result_rows(
    rows: &mut Vec<Line<'static>>,
    tool: &str,
    text: &str,
    level: Level,
    caps: &Caps,
    width: usize,
) {
    let style = theme::tool_result();
    let cleaned = clean(text);
    let total = cleaned.lines().count();
    if total == 0 {
        rows.push(Line::styled("(no output)", style));
        return;
    }
    if tool == "Read" && level != Level::Everything {
        let noun = if total == 1 { "line" } else { "lines" };
        rows.push(Line::styled(format!("{total} {noun}"), style));
        return;
    }
    for line in cleaned.lines().take(caps.result_lines) {
        for piece in fit(line, width, level == Level::Everything) {
            rows.push(Line::styled(piece, style));
        }
    }
    if total > caps.result_lines {
        rows.push(Line::styled(
            format!("… {} more lines", total - caps.result_lines),
            style,
        ));
    }
}

fn diff_rows(rows: &mut Vec<Line<'static>>, changes: &[Change], caps: &Caps, width: usize) {
    let (added, removed) = diff::count(changes);
    rows.push(Line::styled(
        change_summary(added, removed),
        theme::diff_header(),
    ));
    let shown = diff::with_context(changes, DIFF_CONTEXT_LINES);
    for item in shown.iter().take(caps.diff_rows) {
        rows.push(match item {
            Shown::Gap => Line::styled("⋯", theme::diff_context()),
            Shown::Line(Change::Same(text)) => {
                Line::styled(truncate(&clean(text), width), theme::diff_context())
            }
            Shown::Line(Change::Added(text)) => tinted('+', text, width, theme::diff_added()),
            Shown::Line(Change::Removed(text)) => tinted('-', text, width, theme::diff_removed()),
        });
    }
    if shown.len() > caps.diff_rows {
        rows.push(Line::styled(
            format!("… {} more lines", shown.len() - caps.diff_rows),
            theme::diff_header(),
        ));
    }
}

fn tinted(sign: char, text: &str, width: usize, style: Style) -> Line<'static> {
    let body = truncate(&format!("{sign} {}", clean(text)), width);
    let padding = width.saturating_sub(display_width(&body));
    Line::styled(format!("{body}{}", " ".repeat(padding)), style)
}

fn change_summary(added: usize, removed: usize) -> String {
    let plural = |count: usize| if count == 1 { "line" } else { "lines" };
    match (added, removed) {
        (added, 0) => format!("Added {added} {}", plural(added)),
        (0, removed) => format!("Removed {removed} {}", plural(removed)),
        (added, removed) => format!(
            "Added {added} {}, removed {removed} {}",
            plural(added),
            plural(removed)
        ),
    }
}

fn edit_changes(input: &Value) -> Option<Vec<Change<'_>>> {
    if input.get("old_string").is_some() {
        return replacement(input);
    }
    if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        let merged: Vec<Change> = edits.iter().filter_map(replacement).flatten().collect();
        return (!merged.is_empty()).then_some(merged);
    }
    input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))?;
    let written = input
        .get("content")
        .or_else(|| input.get("new_source"))?
        .as_str()?;
    Some(diff::additions(written))
}

fn replacement(edit: &Value) -> Option<Vec<Change<'_>>> {
    Some(diff::line_diff(
        edit.get("old_string")?.as_str()?,
        edit.get("new_string")?.as_str()?,
    ))
}

fn looks_like_error(result: &str) -> bool {
    let start = result.trim_start();
    start.starts_with("Error") || start.contains("<tool_use_error>")
}

fn detail_lines(name: &str, input: &Value) -> Vec<String> {
    let field = |key: &str| input.get(key).and_then(Value::as_str).map(clean);
    if name == "Bash" {
        return field("command")
            .map(|command| command.lines().skip(1).map(str::to_owned).collect())
            .unwrap_or_default();
    }
    if DELEGATING_TOOLS.contains(&name) {
        return field("prompt")
            .map(|p| p.lines().map(str::to_owned).collect())
            .unwrap_or_default();
    }
    if SELF_DESCRIBING_TOOLS.contains(&name) {
        return Vec::new();
    }
    serde_json::to_string_pretty(input)
        .map(|json| json.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

pub fn summarize(name: &str, input: &Value) -> String {
    let field = |key: &str| input.get(key).and_then(Value::as_str).map(clean);
    let chosen = match name {
        "Bash" => field("command").map(|command| match command.split_once('\n') {
            Some((first, _)) => format!("{first} …"),
            None => command,
        }),
        "Grep" => field("pattern").map(|pattern| match field("path") {
            Some(path) => format!("\"{pattern}\" in {path}"),
            None => format!("\"{pattern}\""),
        }),
        "Agent" | "Task" => field("description").map(|description| match field("subagent_type") {
            Some(kind) => format!("{kind}: {description}"),
            None => description,
        }),
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|todos| format!("{} todos", todos.len())),
        _ => None,
    }
    .or_else(|| SUMMARY_KEYS.iter().find_map(|key| field(key)))
    .or_else(|| {
        input
            .as_object()
            .and_then(|o| o.values().find_map(Value::as_str))
            .map(clean)
    })
    .unwrap_or_default();
    let single_line = chosen.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&single_line, SUMMARY_WIDTH)
}

fn split_header(header: &str, name: &str) -> (String, String) {
    match header.strip_prefix(name) {
        Some(rest) => (name.to_owned(), rest.to_owned()),
        None => (header.to_owned(), String::new()),
    }
}

pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                if chars.next_if_eq(&'[').is_some() {
                    while chars.next_if(|n| !n.is_ascii_alphabetic()).is_some() {}
                    chars.next();
                } else {
                    chars.next();
                }
            }
            '\t' => out.push_str(&" ".repeat(TAB_WIDTH)),
            '\n' => out.push('\n'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

fn fit(line: &str, width: usize, wrap: bool) -> Vec<String> {
    if !wrap {
        return vec![truncate(line, width)];
    }
    let mut pieces = vec![String::new()];
    let mut used = 0;
    for c in line.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width.max(1) {
            pieces.push(String::new());
            used = 0;
        }
        pieces.last_mut().unwrap().push(c);
        used += w;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn render(name: &str, input: Value, result: Option<&str>, level: Level) -> Vec<String> {
        let mut out = Vec::new();
        render_call(&mut out, name, &input, result, level, 60);
        text(&out)
    }

    #[test]
    fn failed_edit_still_shows_its_error_at_tools_level() {
        let input = json!({"file_path": "f", "old_string": "a", "new_string": "b"});
        let rows = render(
            "Edit",
            input,
            Some("Error: String to replace not found"),
            Level::Tools,
        );
        assert!(
            rows.iter()
                .any(|l| l.contains("String to replace not found"))
        );
    }

    #[test]
    fn ansi_escapes_and_wide_text_do_not_break_truncation() {
        assert_eq!(clean("\u{1b}[31mred\u{1b}[0m\tx"), "red    x");
        assert_eq!(truncate("日本語日本語", 7), "日本語…");
    }
}
