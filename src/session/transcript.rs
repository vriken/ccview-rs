use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse { name: String, input: Value },
    ToolResult(String),
}

#[derive(Debug)]
pub struct Message {
    pub role: Role,
    pub timestamp: Option<String>,
    pub is_meta: bool,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Default)]
pub struct Session {
    pub messages: Vec<Message>,
    pub malformed_lines: usize,
}
