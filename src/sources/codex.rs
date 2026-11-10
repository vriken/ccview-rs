use crate::{
    session::{
        entry::TitleSource,
        transcript::{Block, Message, Role, Session},
    },
    sources::SessionFile,
    terminal::CommandSpec,
};
use crate::{
    session::{entry::TranscriptHandle, harness_id::HarnessId},
    sources::{
        AgeFilter, SourceEntry, follow::JsonlFollower, home_dir, prompt_title, resolve_binary,
        short_id,
    },
};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const BINARY_ENV: &str = "CCVIEW_CODEX_BIN";
const DEFAULT_BINARY: &str = "codex";
const PROMPT_SEARCH_LINE_LIMIT: usize = 400;
const IGNORED_PROMPT_PREFIXES: [&str; 2] = ["<", "# AGENTS.md"];

pub fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".codex")))
}

pub fn list_sessions_in(codex_home: &Path) -> Vec<SourceEntry> {
    let thread_names = thread_names(codex_home);
    let mut entries: Vec<SourceEntry> = list_rollouts(codex_home)
        .into_iter()
        .filter_map(|path| entry_for_rollout(path, &thread_names))
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used));
    entries
}

pub fn load_transcript(path: &Path) -> io::Result<Session> {
    let mut session = Session::default();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(messages) => session.messages.extend(messages),
            Err(_) => session.malformed_lines += 1,
        }
    }
    Ok(session)
}

pub fn follower(path: impl Into<PathBuf>) -> JsonlFollower {
    JsonlFollower::new(path, parse_rollout_line)
}

pub fn parse_rollout_line(line: &str) -> Result<Vec<Message>, serde_json::Error> {
    let record: Value = serde_json::from_str(line)?;
    if record.get("type").and_then(Value::as_str) != Some("response_item") {
        return Ok(Vec::new());
    }
    let Some(payload) = record.get("payload") else {
        return Ok(Vec::new());
    };
    let timestamp = record
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(response_item_message(payload, timestamp)
        .into_iter()
        .collect())
}

pub fn new_session(cwd: &Path) -> CommandSpec {
    command(&resolve_binary(BINARY_ENV, DEFAULT_BINARY), &[], cwd)
}

pub fn resume_session(session_id: &str, cwd: &Path) -> CommandSpec {
    command(
        &resolve_binary(BINARY_ENV, DEFAULT_BINARY),
        &["resume", session_id],
        cwd,
    )
}

pub fn fork_session(session_id: &str, cwd: &Path) -> CommandSpec {
    command(
        &resolve_binary(BINARY_ENV, DEFAULT_BINARY),
        &["fork", session_id],
        cwd,
    )
}

pub fn continue_latest(cwd: &Path) -> CommandSpec {
    command(
        &resolve_binary(BINARY_ENV, DEFAULT_BINARY),
        &["resume", "--last"],
        cwd,
    )
}

fn command(binary: &str, args: &[&str], cwd: &Path) -> CommandSpec {
    CommandSpec::new(binary).args(args.iter().copied()).cwd(cwd)
}

fn collect_rollouts(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(children) = fs::read_dir(dir) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        if path.is_dir() {
            collect_rollouts(&path, found);
        } else if is_rollout(&path) {
            found.push(path);
        }
    }
}

fn is_rollout(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn read_thread_names(index: &Path) -> HashMap<String, String> {
    let Ok(file) = File::open(index) else {
        return HashMap::new();
    };
    let mut names = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(id), Some(name)) = (
            record.get("id").and_then(Value::as_str),
            record.get("thread_name").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !name.trim().is_empty() {
            names.insert(id.to_owned(), name.to_owned());
        }
    }
    names
}

pub struct RolloutHead {
    pub session_id: String,
    pub cwd: String,
    pub first_prompt: Option<String>,
}

pub fn list_rollouts(codex_home: &Path) -> Vec<PathBuf> {
    let mut rollouts = Vec::new();
    collect_rollouts(&codex_home.join("sessions"), &mut rollouts);
    rollouts
}

pub const THREAD_INDEX_FILE: &str = "session_index.jsonl";

pub fn thread_names(codex_home: &Path) -> HashMap<String, String> {
    read_thread_names(&codex_home.join(THREAD_INDEX_FILE))
}

pub fn read_rollout_head(path: &Path) -> Option<RolloutHead> {
    let mut lines = BufReader::new(File::open(path).ok()?).lines();
    let meta: Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = meta.get("payload")?;
    Some(RolloutHead {
        session_id: payload.get("id")?.as_str()?.to_owned(),
        cwd: payload
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        first_prompt: first_prompt(lines),
    })
}

pub fn entry_from_head(
    path: PathBuf,
    last_used: SystemTime,
    head: RolloutHead,
    thread_names: &HashMap<String, String>,
) -> SourceEntry {
    let (title, title_source) = match (thread_names.get(&head.session_id), head.first_prompt) {
        (Some(name), _) => (name.clone(), TitleSource::Agent),
        (None, Some(prompt)) => (prompt, TitleSource::Prompt),
        (None, None) => (short_id(&head.session_id), TitleSource::SessionId),
    };
    SourceEntry {
        harness: HarnessId::Codex,
        session_id: head.session_id,
        cwd: head.cwd,
        title,
        title_source,
        last_used,
        handle: TranscriptHandle::Rollout(path),
    }
}

fn entry_for_rollout(path: PathBuf, thread_names: &HashMap<String, String>) -> Option<SourceEntry> {
    let head = read_rollout_head(&path)?;
    let last_used = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some(entry_from_head(path, last_used, head, thread_names))
}

fn first_prompt(lines: impl Iterator<Item = io::Result<String>>) -> Option<String> {
    for line in lines.take(PROMPT_SEARCH_LINE_LIMIT).map_while(Result::ok) {
        let Ok(messages) = parse_rollout_line(&line) else {
            continue;
        };
        for message in messages {
            if message.role != Role::User || message.is_meta {
                continue;
            }
            let prompt = message.blocks.iter().find_map(|block| match block {
                Block::Text(text) => prompt_title(text),
                _ => None,
            });
            if prompt.is_some() {
                return prompt;
            }
        }
    }
    None
}

fn response_item_message(payload: &Value, timestamp: Option<String>) -> Option<Message> {
    let (role, is_meta, blocks) = match payload.get("type")?.as_str()? {
        "message" => {
            let role = match payload.get("role")?.as_str()? {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return None,
            };
            let blocks = content_blocks(payload.get("content")?);
            let is_meta = role == Role::User && starts_with_ignored_prefix(&blocks);
            (role, is_meta, blocks)
        }
        "reasoning" => {
            let summary = reasoning_summary(payload.get("summary")?);
            if summary.is_empty() {
                return None;
            }
            (Role::Assistant, false, vec![Block::Thinking(summary)])
        }
        "function_call" => {
            let arguments = payload.get("arguments")?;
            let input = match arguments.as_str() {
                Some(raw) => serde_json::from_str(raw).unwrap_or_else(|_| arguments.clone()),
                None => arguments.clone(),
            };
            let name = payload.get("name")?.as_str()?.to_owned();
            (Role::Assistant, false, vec![Block::ToolUse { name, input }])
        }
        "custom_tool_call" => {
            let name = payload.get("name")?.as_str()?.to_owned();
            let input = payload.get("input").cloned().unwrap_or(Value::Null);
            (Role::Assistant, false, vec![Block::ToolUse { name, input }])
        }
        "function_call_output" | "custom_tool_call_output" => (
            Role::User,
            false,
            vec![Block::ToolResult(flatten_output(payload.get("output")?))],
        ),
        _ => return None,
    };
    if blocks.is_empty() {
        return None;
    }
    Some(Message {
        role,
        timestamp,
        is_meta,
        blocks,
    })
}

fn starts_with_ignored_prefix(blocks: &[Block]) -> bool {
    blocks.iter().any(|block| match block {
        Block::Text(text) => IGNORED_PROMPT_PREFIXES
            .iter()
            .any(|prefix| text.trim_start().starts_with(prefix)),
        _ => false,
    })
}

fn content_blocks(content: &Value) -> Vec<Block> {
    let Some(items) = content.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item.get("type")?.as_str()? {
            "input_text" | "output_text" => {
                Some(Block::Text(item.get("text")?.as_str()?.to_owned()))
            }
            "input_image" => Some(Block::Text("[image]".to_owned())),
            _ => None,
        })
        .collect()
}

fn reasoning_summary(summary: &Value) -> String {
    let Some(items) = summary.as_array() else {
        return String::new();
    };
    items
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn flatten_output(output: &Value) -> String {
    match output {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("input_image") => Some("[image]"),
                _ => item.get("text").and_then(Value::as_str),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn list_codex_files(codex_home: &Path) -> Vec<SessionFile> {
    list_codex_files_within(codex_home, &AgeFilter::default())
}

pub fn list_codex_files_within(codex_home: &Path, age: &AgeFilter) -> Vec<SessionFile> {
    list_rollouts(codex_home)
        .into_iter()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            let stem = path.file_stem().and_then(|stem| stem.to_str())?;
            if !age.admits_file_stem(modified, stem) {
                return None;
            }
            Some(SessionFile {
                modified,
                len: metadata.len(),
                path,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &[&str] = &[
        r#"{"timestamp":"2026-01-01T10:00:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/work/app"}}"#,
        r#"{"type":"future_record","payload":{"x":1}}"#,
        r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
        r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
        r#"{"timestamp":"2026-01-01T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the build\nplease"}]}}"#,
        r#"{"type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"abc"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"ls\"}","call_id":"c1"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":[{"type":"input_text","text":"a.rs"}]}}"#,
        r#"{"type":"response_item","payload":{"type":"unknown_item_kind"}}"#,
        "not json",
        r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
    ];

    fn write_fixture(dir: &Path, lines: &[&str]) -> PathBuf {
        let day = dir.join("sessions/2026/01/01");
        fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-2026-01-01T10-00-00-sess-1.jsonl");
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccview-codex-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn converts_rollout_and_skips_unknown_records() {
        let home = temp_home("convert");
        let path = write_fixture(&home, FIXTURE);
        let session = load_transcript(&path).unwrap();
        assert_eq!(session.malformed_lines, 1);
        let roles: Vec<_> = session
            .messages
            .iter()
            .map(|m| (m.role, m.is_meta))
            .collect();
        assert_eq!(
            roles,
            vec![
                (Role::User, true),
                (Role::User, false),
                (Role::Assistant, false),
                (Role::User, false),
                (Role::Assistant, false),
            ]
        );
        assert!(matches!(
            &session.messages[2].blocks[0],
            Block::ToolUse { name, input } if name == "shell" && input["cmd"] == "ls"
        ));
        assert!(
            matches!(&session.messages[3].blocks[0], Block::ToolResult(text) if text == "a.rs")
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn lists_entry_with_prompt_title_then_index_name_wins() {
        let home = temp_home("list");
        write_fixture(&home, FIXTURE);
        let entry = &list_sessions_in(&home)[0];
        assert_eq!(entry.session_id, "sess-1");
        assert_eq!(entry.cwd, "/work/app");
        assert_eq!(entry.title, "fix the build");
        assert_eq!(entry.title_source, TitleSource::Prompt);

        fs::write(
            home.join("session_index.jsonl"),
            "{\"id\":\"sess-1\",\"thread_name\":\"first\"}\n{\"id\":\"sess-1\",\"thread_name\":\"Final name\"}\n",
        )
        .unwrap();
        let entry = &list_sessions_in(&home)[0];
        assert_eq!(entry.title, "Final name");
        assert_eq!(entry.title_source, TitleSource::Agent);
        fs::remove_dir_all(home).unwrap();
    }
}
