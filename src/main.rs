mod discover;
mod parse;

use parse::{Block, Message, Role};
use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let path = match resolve_path() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let session = match parse::parse_file(&path) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    for message in &session.messages {
        print_message(message);
    }
    eprintln!(
        "{}: {} messages, {} malformed lines",
        path.display(),
        session.messages.len(),
        session.malformed_lines
    );
    ExitCode::SUCCESS
}

fn resolve_path() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--file"), Some(path)) => Ok(PathBuf::from(path)),
        (None, _) => {
            let dir = discover::claude_projects_dir().ok_or("HOME is not set")?;
            discover::find_sessions(&dir)
                .into_iter()
                .next()
                .ok_or_else(|| format!("no sessions under {}", dir.display()))
        }
        _ => Err("usage: ccview-rs [--file <session.jsonl>]".to_owned()),
    }
}

fn print_message(message: &Message) {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    for block in &message.blocks {
        match block {
            Block::Text(text) => println!("[{role}] {text}"),
            Block::Thinking(_) => {}
            Block::ToolUse { name, .. } => println!("[{role}] tool_use {name}"),
            Block::ToolResult(content) => {
                let first_line = content.lines().next().unwrap_or("");
                println!("[{role}] tool_result {first_line}");
            }
        }
    }
}
