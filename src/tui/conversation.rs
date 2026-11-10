use crate::parse::{Block, Message, Role};
use super::theme;
use ratatui::{style::Style, text::Line};
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
    wrapped_entries: usize,
    wrapped_width: usize,
}

impl Conversation {
    pub fn note_hidden_earlier(&mut self, count: usize) {
        if count > 0 {
            self.entries.insert(
                0,
                Entry {
                    style: theme::secondary(),
                    text: format!("… {count} earlier messages not shown"),
                },
            );
            self.wrapped.clear();
            self.wrapped_entries = 0;
        }
    }

    pub fn push_messages(&mut self, messages: &[Message]) {
        for message in messages.iter().filter(|m| !m.is_meta) {
            push_message(&mut self.entries, message);
        }
    }

    pub fn lines(&mut self, width: usize) -> &[Line<'static>] {
        let width = width.max(1);
        if self.wrapped_width != width {
            self.wrapped.clear();
            self.wrapped_entries = 0;
            self.wrapped_width = width;
        }
        for entry in &self.entries[self.wrapped_entries..] {
            self.wrapped.extend(
                wrap(&entry.text, width)
                    .into_iter()
                    .map(|piece| Line::styled(piece, entry.style)),
            );
        }
        self.wrapped_entries = self.entries.len();
        &self.wrapped
    }
}

fn role_text_style(role: Role) -> Style {
    match role {
        Role::User => theme::user_text(),
        Role::Assistant => theme::assistant_text(),
    }
}

fn role_label_style(role: Role) -> Style {
    match role {
        Role::User => theme::user_label(),
        Role::Assistant => theme::assistant_label(),
    }
}

fn message_text_lines(role: Role, text: &str) -> Vec<Entry> {
    text.lines()
        .map(|line| Entry {
            style: role_text_style(role),
            text: format!("{} {line}", gutter(role)),
        })
        .collect()
}

fn push_message(entries: &mut Vec<Entry>, message: &Message) {
    let mut body = Vec::new();
    let mut has_text = false;
    for block in &message.blocks {
        match block {
            Block::Text(text) => {
                has_text = true;
                body.extend(message_text_lines(message.role, text));
            }
            Block::Thinking(_) => {}
            Block::ToolUse { name, input } => body.push(Entry {
                style: theme::tool_call(),
                text: format!("  {name}({})", summarize_input(input)),
            }),
            Block::ToolResult(content) => push_tool_result(&mut body, content),
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
            style: role_label_style(message.role),
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

fn push_tool_result(body: &mut Vec<Entry>, content: &str) {
    let style = theme::tool_result();
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
