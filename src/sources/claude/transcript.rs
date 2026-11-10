use crate::session::transcript::{Block, Message, Role, Session};
use crate::sources::follow::JsonlFollower;
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

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

pub fn follow(path: impl Into<PathBuf>) -> JsonlFollower {
    JsonlFollower::new(path, parse_claude_line)
}

fn parse_claude_line(line: &str) -> Result<Vec<Message>, serde_json::Error> {
    parse_line(line).map(|message| message.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, File},
        io::Write,
        sync::atomic::AtomicBool,
    };

    const LINE: &str = r#"{"type":"user","message":{"content":"hi"}}"#;

    fn poll(tailer: &mut JsonlFollower) -> Vec<Message> {
        tailer
            .poll(usize::MAX, &AtomicBool::new(false))
            .unwrap()
            .messages
    }

    #[test]
    fn buffers_partial_line_and_rereads_after_truncation() {
        let path = std::env::temp_dir().join(format!("ccview-tail-{}.jsonl", std::process::id()));
        let mut tailer = follow(&path);
        assert!(poll(&mut tailer).is_empty());

        let mut file = File::create(&path).unwrap();
        write!(file, "{LINE}\n{}", &LINE[..10]).unwrap();
        assert_eq!(poll(&mut tailer).len(), 1);
        assert!(poll(&mut tailer).is_empty());

        writeln!(file, "{}", &LINE[10..]).unwrap();
        assert_eq!(poll(&mut tailer).len(), 1);

        fs::write(&path, format!("{LINE}\n")).unwrap();
        assert_eq!(poll(&mut tailer).len(), 1);
        fs::remove_file(&path).unwrap();
    }
}
