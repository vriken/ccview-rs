use crate::{
    session::{
        entry::TitleSource,
        transcript::{Block, Message, Role, Session},
    },
    terminal::CommandSpec,
};
use crate::{
    session::{entry::TranscriptHandle, harness_id::HarnessId},
    sources::{SourceEntry, home_dir, iso8601_from_millis, prompt_title, resolve_binary, short_id},
};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

pub const BINARY_ENV: &str = "CCVIEW_OPENCODE_BIN";
const DEFAULT_BINARY: &str = "opencode";
const BUSY_TIMEOUT: Duration = Duration::from_millis(1500);
const PLACEHOLDER_TITLE_PREFIXES: [&str; 2] = ["New session - ", "Child session - "];

pub fn database_path() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")))?;
    Some(data_home.join("opencode/opencode.db"))
}

pub fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    Ok(connection)
}

pub fn list_sessions_in(path: &Path) -> rusqlite::Result<Vec<SourceEntry>> {
    let connection = open_read_only(path)?;
    let mut statement = connection.prepare(
        "SELECT id, directory, title, time_updated FROM session
         WHERE parent_id IS NULL AND time_archived IS NULL
         ORDER BY time_updated DESC",
    )?;
    let rows: Vec<(String, String, String, i64)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut entries = Vec::with_capacity(rows.len());
    for (session_id, cwd, title, time_updated) in rows {
        let (title, title_source) = if is_real_title(&title) {
            (title, TitleSource::Agent)
        } else {
            match first_user_prompt(&connection, &session_id)? {
                Some(prompt) => (prompt, TitleSource::Prompt),
                None => (short_id(&session_id), TitleSource::SessionId),
            }
        };
        entries.push(SourceEntry {
            harness: HarnessId::OpenCode,
            handle: TranscriptHandle::Database {
                path: path.to_owned(),
                session_id: session_id.clone(),
            },
            session_id,
            cwd,
            title,
            title_source,
            last_used: UNIX_EPOCH + Duration::from_millis(time_updated.max(0) as u64),
        });
    }
    Ok(entries)
}

pub fn load_transcript(path: &Path, session_id: &str) -> rusqlite::Result<Session> {
    let connection = open_read_only(path)?;
    load_transcript_with(&connection, session_id)
}

pub struct DatabaseFollower {
    path: PathBuf,
    session_id: String,
    fingerprint: Option<Fingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    messages: i64,
    parts: i64,
    latest_update: i64,
}

impl DatabaseFollower {
    pub fn new(path: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            session_id: session_id.into(),
            fingerprint: None,
        }
    }

    pub fn poll(&mut self) -> rusqlite::Result<Option<Session>> {
        let connection = open_read_only(&self.path)?;
        let current = fingerprint(&connection, &self.session_id)?;
        if self.fingerprint == Some(current) {
            return Ok(None);
        }
        let session = load_transcript_with(&connection, &self.session_id)?;
        self.fingerprint = Some(current);
        Ok(Some(session))
    }
}

pub fn new_session(cwd: &Path) -> CommandSpec {
    command(&[], cwd)
}

pub fn resume_session(session_id: &str, cwd: &Path) -> CommandSpec {
    command(&["--session", session_id], cwd)
}

pub fn fork_session(session_id: &str, cwd: &Path) -> CommandSpec {
    command(&["--session", session_id, "--fork"], cwd)
}

pub fn continue_latest(cwd: &Path) -> CommandSpec {
    command(&["--continue"], cwd)
}

fn command(args: &[&str], cwd: &Path) -> CommandSpec {
    CommandSpec::new(resolve_binary(BINARY_ENV, DEFAULT_BINARY))
        .args(args.iter().copied())
        .cwd(cwd)
}

fn fingerprint(connection: &Connection, session_id: &str) -> rusqlite::Result<Fingerprint> {
    let (messages, message_update): (i64, i64) = connection.query_row(
        "SELECT COUNT(*), COALESCE(MAX(time_updated), 0) FROM message WHERE session_id = ?1",
        params![session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (parts, part_update): (i64, i64) = connection.query_row(
        "SELECT COUNT(*), COALESCE(MAX(time_updated), 0) FROM part WHERE session_id = ?1",
        params![session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Fingerprint {
        messages,
        parts,
        latest_update: message_update.max(part_update),
    })
}

fn is_real_title(title: &str) -> bool {
    !title.trim().is_empty()
        && !PLACEHOLDER_TITLE_PREFIXES
            .iter()
            .any(|prefix| title.starts_with(prefix))
}

fn first_user_prompt(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<Option<String>> {
    let mut statement = connection.prepare(
        "SELECT json_extract(part.data, '$.text') FROM part
         JOIN message ON message.id = part.message_id
         WHERE part.session_id = ?1
           AND json_extract(message.data, '$.role') = 'user'
           AND json_extract(part.data, '$.type') = 'text'
         ORDER BY message.time_created, message.id, part.id",
    )?;
    let texts = statement.query_map(params![session_id], |row| row.get::<_, Option<String>>(0))?;
    for text in texts {
        if let Some(prompt) = text?.as_deref().and_then(prompt_title) {
            return Ok(Some(prompt));
        }
    }
    Ok(None)
}

fn load_transcript_with(connection: &Connection, session_id: &str) -> rusqlite::Result<Session> {
    let mut parts_by_message: HashMap<String, Vec<Value>> = HashMap::new();
    let mut part_statement = connection.prepare(
        "SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY message_id, id",
    )?;
    for row in part_statement.query_map(params![session_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (message_id, data) = row?;
        if let Ok(part) = serde_json::from_str(&data) {
            parts_by_message.entry(message_id).or_default().push(part);
        }
    }

    let mut session = Session::default();
    let mut message_statement = connection
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")?;
    for row in message_statement.query_map(params![session_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (message_id, data) = row?;
        let Ok(message): Result<Value, _> = serde_json::from_str(&data) else {
            session.malformed_lines += 1;
            continue;
        };
        let parts = parts_by_message.remove(&message_id).unwrap_or_default();
        session.messages.extend(convert_message(&message, &parts));
    }
    Ok(session)
}

fn convert_message(message: &Value, parts: &[Value]) -> Vec<Message> {
    let role = match message.get("role").and_then(Value::as_str) {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return Vec::new(),
    };
    let timestamp = message
        .pointer("/time/created")
        .and_then(Value::as_i64)
        .map(iso8601_from_millis);
    let mut blocks = Vec::new();
    let mut results = Vec::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => push_nonempty(&mut blocks, part, "text", Block::Text),
            Some("reasoning") => push_nonempty(&mut blocks, part, "text", Block::Thinking),
            Some("file") => blocks.push(Block::Text(file_placeholder(part))),
            Some("tool") => {
                let Some(name) = part.get("tool").and_then(Value::as_str) else {
                    continue;
                };
                let state = part.get("state");
                blocks.push(Block::ToolUse {
                    name: name.to_owned(),
                    input: state
                        .and_then(|state| state.get("input"))
                        .cloned()
                        .unwrap_or(Value::Null),
                });
                if let Some(result) = state.and_then(tool_result_text) {
                    results.push(Block::ToolResult(result));
                }
            }
            _ => {}
        }
    }
    let mut converted = Vec::new();
    if !blocks.is_empty() {
        converted.push(Message {
            role,
            timestamp: timestamp.clone(),
            is_meta: false,
            blocks,
        });
    }
    if !results.is_empty() {
        converted.push(Message {
            role: Role::User,
            timestamp,
            is_meta: false,
            blocks: results,
        });
    }
    converted
}

fn push_nonempty(blocks: &mut Vec<Block>, part: &Value, key: &str, make: fn(String) -> Block) {
    if let Some(text) = part
        .get(key)
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        blocks.push(make(text.to_owned()));
    }
}

fn file_placeholder(part: &Value) -> String {
    let name = part
        .get("filename")
        .and_then(Value::as_str)
        .or_else(|| part.get("mime").and_then(Value::as_str))
        .unwrap_or("file");
    format!("[file: {name}]")
}

fn tool_result_text(state: &Value) -> Option<String> {
    match state.get("status").and_then(Value::as_str)? {
        "completed" => Some(
            state
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        "error" => Some(
            state
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ccview-oc-{name}-{}.db", std::process::id()));
        let _ = fs::remove_file(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, parent_id TEXT, title TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
             INSERT INTO session VALUES ('ses_a','/work/app',NULL,'Named chat',20,NULL);
             INSERT INTO session VALUES ('ses_b','/work/app',NULL,'New session - 2026',30,NULL);
             INSERT INTO session VALUES ('ses_c','/work/app','ses_a','child',40,NULL);
             INSERT INTO session VALUES ('ses_d','/work/app',NULL,'archived',50,5);
             INSERT INTO message VALUES ('m1','ses_b',1,1,'{\"role\":\"user\",\"time\":{\"created\":1000}}');
             INSERT INTO message VALUES ('m2','ses_b',2,2,'{\"role\":\"assistant\",\"time\":{\"created\":2000}}');
             INSERT INTO message VALUES ('m3','ses_b',3,3,'{\"role\":\"surprise\"}');
             INSERT INTO part VALUES ('p1','m1','ses_b',1,1,'{\"type\":\"text\",\"text\":\"hello there\"}');
             INSERT INTO part VALUES ('p2','m2','ses_b',2,2,'{\"type\":\"reasoning\",\"text\":\"\"}');
             INSERT INTO part VALUES ('p3','m2','ses_b',3,3,'{\"type\":\"tool\",\"tool\":\"bash\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"ls\"},\"output\":\"a.rs\"}}');
             INSERT INTO part VALUES ('p4','m2','ses_b',4,4,'{\"type\":\"text\",\"text\":\"done\"}');
             INSERT INTO part VALUES ('p5','m2','ses_b',5,5,'{\"type\":\"step-finish\"}');",
        )
        .unwrap();
        path
    }

    #[test]
    fn lists_top_level_unarchived_sessions_with_titles() {
        let path = fixture("list");
        let entries = list_sessions_in(&path).unwrap();
        let summary: Vec<_> = entries
            .iter()
            .map(|e| (e.session_id.as_str(), e.title.as_str(), e.title_source))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("ses_b", "hello there", TitleSource::Prompt),
                ("ses_a", "Named chat", TitleSource::Agent),
            ]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn converts_messages_and_pairs_tool_results() {
        let path = fixture("convert");
        let session = load_transcript(&path, "ses_b").unwrap();
        let roles: Vec<_> = session.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::User, Role::Assistant, Role::User]);
        assert_eq!(
            session.messages[0].timestamp.as_deref(),
            Some("1970-01-01T00:00:01.000Z")
        );
        assert!(matches!(
            &session.messages[1].blocks[..],
            [Block::ToolUse { name, .. }, Block::Text(text)] if name == "bash" && text == "done"
        ));
        assert!(
            matches!(&session.messages[2].blocks[0], Block::ToolResult(text) if text == "a.rs")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn follower_reports_only_when_rows_change() {
        let path = fixture("follow");
        let mut follower = DatabaseFollower::new(&path, "ses_b");
        assert_eq!(follower.poll().unwrap().unwrap().messages.len(), 3);
        assert!(follower.poll().unwrap().is_none());

        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "INSERT INTO message VALUES ('m4','ses_b',4,4,'{\"role\":\"user\"}');
                 INSERT INTO part VALUES ('p6','m4','ses_b',6,6,'{\"type\":\"text\",\"text\":\"more\"}');",
            )
            .unwrap();
        assert_eq!(follower.poll().unwrap().unwrap().messages.len(), 4);
        fs::remove_file(path).unwrap();
    }
}
