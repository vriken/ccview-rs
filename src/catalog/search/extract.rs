use crate::{
    session::transcript::{Block, Message, Role},
    sources::claude::scan::is_genuine_prompt,
};
use serde_json::Value;

pub(crate) type LineParser = fn(&str) -> Result<Vec<Message>, serde_json::Error>;

const THINKING_CAP: usize = 2048;
const TOOL_RESULT_CAP: usize = 2048;
const TOOL_ARGUMENT_CAP: usize = 512;
const BLOB_PROBE: usize = 200;
const TOOL_ARGUMENT_KEYS: [&str; 8] = [
    "file_path",
    "path",
    "command",
    "pattern",
    "description",
    "url",
    "query",
    "prompt",
];

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Columns {
    pub(crate) user: String,
    pub(crate) assistant: String,
    pub(crate) tool: String,
}

impl Columns {
    pub(crate) fn is_empty(&self) -> bool {
        self.user.is_empty() && self.assistant.is_empty() && self.tool.is_empty()
    }
}

pub(crate) fn columns_of(messages: &[Message]) -> Columns {
    let mut columns = Columns::default();
    for message in messages {
        for block in &message.blocks {
            match (message.role, block) {
                (Role::User, Block::Text(text)) => {
                    if !message.is_meta && is_genuine_prompt(text) {
                        push(&mut columns.user, text, usize::MAX);
                    }
                }
                (Role::Assistant, Block::Text(text)) => {
                    push(&mut columns.assistant, text, usize::MAX);
                }
                (_, Block::Thinking(text)) => push(&mut columns.assistant, text, THINKING_CAP),
                (_, Block::ToolResult(text)) => push(&mut columns.tool, text, TOOL_RESULT_CAP),
                (_, Block::ToolUse { input, .. }) => push_arguments(&mut columns.tool, input),
            }
        }
    }
    columns
}

fn push_arguments(column: &mut String, input: &Value) {
    for key in TOOL_ARGUMENT_KEYS {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            push(column, text, TOOL_ARGUMENT_CAP);
        }
    }
}

fn push(column: &mut String, text: &str, cap: usize) {
    if is_blob(text) {
        return;
    }
    let text = truncated(text, cap);
    if text.trim().is_empty() {
        return;
    }
    if !column.is_empty() {
        column.push('\n');
    }
    column.push_str(text);
}

fn is_blob(text: &str) -> bool {
    text.len() > BLOB_PROBE
        && !truncated(text, BLOB_PROBE)
            .bytes()
            .any(|byte| byte == b' ' || byte == b'\n')
}

fn truncated(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
