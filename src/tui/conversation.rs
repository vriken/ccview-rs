use crate::parse::{Block, Message, Role};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};
use serde_json::Value;

const TOOL_RESULT_LINES: usize = 3;
const SUMMARY_WIDTH: usize = 80;
const SUMMARY_KEYS: [&str; 7] = [
    "command",
    "file_path",
    "path",
    "pattern",
    "url",
    "description",
    "query",
];

struct Entry {
    style: Style,
    text: String,
}

#[derive(Default)]
pub struct Conversation {
    entries: Vec<Entry>,
    wrapped: Vec<Line<'static>>,
    wrapped_width: usize,
}

impl Conversation {
    pub fn from_messages(messages: &[Message]) -> Self {
        let mut entries = Vec::new();
        for message in messages.iter().filter(|m| !m.is_meta) {
            push_message(&mut entries, message);
        }
        Self {
            entries,
            ..Self::default()
        }
    }

    pub fn lines(&mut self, width: usize) -> &[Line<'static>] {
        let width = width.max(1);
        if self.wrapped_width != width {
            self.wrapped = self
                .entries
                .iter()
                .flat_map(|entry| {
                    wrap(&entry.text, width)
                        .into_iter()
                        .map(|piece| Line::styled(piece, entry.style))
                })
                .collect();
            self.wrapped_width = width;
        }
        &self.wrapped
    }
}

fn role_color(role: Role) -> Color {
    match role {
        Role::User => Color::Cyan,
        Role::Assistant => Color::Green,
    }
}

fn push_message(entries: &mut Vec<Entry>, message: &Message) {
    let color = role_color(message.role);
    let text_style = Style::default().fg(color);
    let dim = Style::default().fg(Color::DarkGray);
    let tool_style = Style::default().fg(Color::Yellow);
    let mut body = Vec::new();
    let mut has_text = false;
    for block in &message.blocks {
        match block {
            Block::Text(text) => {
                has_text = true;
                for line in text.lines() {
                    body.push(Entry {
                        style: text_style,
                        text: format!("{} {line}", gutter(message.role)),
                    });
                }
            }
            Block::Thinking(_) => {}
            Block::ToolUse { name, input } => body.push(Entry {
                style: tool_style,
                text: format!("  {name}({})", summarize_input(input)),
            }),
            Block::ToolResult(content) => push_tool_result(&mut body, content, dim),
        }
    }
    if body.is_empty() {
        return;
    }
    if has_text || message.role == Role::Assistant {
        let label = match message.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
        };
        let stamp = message.timestamp.as_deref().unwrap_or("");
        entries.push(Entry {
            style: text_style.add_modifier(Modifier::BOLD),
            text: format!("{label}  {stamp}"),
        });
    }
    entries.extend(body);
    entries.push(Entry {
        style: Style::default(),
        text: String::new(),
    });
}

fn gutter(role: Role) -> &'static str {
    match role {
        Role::User => "▌",
        Role::Assistant => " ",
    }
}

fn push_tool_result(body: &mut Vec<Entry>, content: &str, style: Style) {
    let total = content.lines().count();
    for line in content.lines().take(TOOL_RESULT_LINES) {
        body.push(Entry {
            style,
            text: format!("  ↳ {line}"),
        });
    }
    if total > TOOL_RESULT_LINES {
        body.push(Entry {
            style,
            text: format!("  … {} more lines", total - TOOL_RESULT_LINES),
        });
    }
}

fn summarize_input(input: &Value) -> String {
    let chosen = SUMMARY_KEYS
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .or_else(|| {
            input
                .as_object()
                .and_then(|o| o.values().find_map(Value::as_str))
        })
        .unwrap_or("");
    let single_line = chosen.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() > SUMMARY_WIDTH {
        let cut: String = single_line.chars().take(SUMMARY_WIDTH).collect();
        format!("{cut}…")
    } else {
        single_line
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let indent: String = text.chars().take_while(|c| *c == ' ').collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let mut word = word.to_owned();
        let mut word_len = word.chars().count();
        if current_len > 0 && current_len + 1 + word_len > width {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        while word_len > width {
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            word_len -= width;
            if current_len > 0 {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            lines.push(head);
        }
        if current_len > 0 {
            current.push(' ');
            current_len += 1;
        } else if lines.is_empty() {
            current.push_str(&indent);
            current_len = indent.chars().count();
        }
        current.push_str(&word);
        current_len += word_len;
    }
    lines.push(current);
    lines
}
