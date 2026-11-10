use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

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

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type", default)]
    kind: String,
    timestamp: Option<String>,
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    content: Value,
}

pub fn parse_file(path: &Path) -> std::io::Result<Session> {
    let mut session = Session::default();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(&line) {
            Ok(message) => session.messages.extend(message),
            Err(_) => session.malformed_lines += 1,
        }
    }
    Ok(session)
}

pub fn parse_line(line: &str) -> Result<Option<Message>, serde_json::Error> {
    serde_json::from_str::<RawEntry>(line).map(into_message)
}

fn into_message(entry: RawEntry) -> Option<Message> {
    let role = match entry.kind.as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    Some(Message {
        role,
        timestamp: entry.timestamp,
        is_meta: entry.is_meta,
        blocks: parse_blocks(entry.message?.content),
    })
}

fn parse_blocks(content: Value) -> Vec<Block> {
    match content {
        Value::String(text) => vec![Block::Text(text)],
        Value::Array(items) => items.iter().filter_map(parse_block).collect(),
        _ => Vec::new(),
    }
}

fn parse_block(item: &Value) -> Option<Block> {
    let str_field = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    match item.get("type")?.as_str()? {
        "text" => str_field("text").map(Block::Text),
        "thinking" => str_field("thinking").map(Block::Thinking),
        "tool_use" => Some(Block::ToolUse {
            name: str_field("name")?,
            input: item.get("input").cloned().unwrap_or(Value::Null),
        }),
        "tool_result" => Some(Block::ToolResult(flatten_text(item.get("content")?))),
        _ => None,
    }
}

fn flatten_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
