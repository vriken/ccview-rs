use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Debug)]
pub(crate) enum Block {
    Text(String),
    Thinking(String),
    ToolUse { name: String, input: Value },
    ToolResult(String),
}

#[derive(Debug)]
pub(crate) struct Message {
    pub(crate) role: Role,
    pub(crate) timestamp: Option<String>,
    pub(crate) is_meta: bool,
    pub(crate) blocks: Vec<Block>,
}

#[derive(Debug, Default)]
pub(crate) struct Session {
    pub(crate) messages: Vec<Message>,
    pub(crate) malformed_lines: usize,
}
