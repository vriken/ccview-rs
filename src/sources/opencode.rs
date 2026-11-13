use crate::{
    session::{
        cursor::Cursor,
        entry::TitleSource,
        transcript::{Block, Message, Role, Session},
    },
    sys::command::CommandSpec,
};
use crate::{
    session::{entry::TranscriptHandle, harness_id::HarnessId},
    sources::{
        AgeFilter, SourceEntry, home_dir, iso8601_from_millis, prompt_title, resolve_binary,
        short_id,
    },
};
use rusqlite::{Connection, OpenFlags, params, params_from_iter, types::Value as SqlValue};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

pub(crate) const BINARY_ENV: &str = "CCVIEW_OPENCODE_BIN";
const DEFAULT_BINARY: &str = "opencode";
const BUSY_TIMEOUT: Duration = Duration::from_millis(1500);
const PLACEHOLDER_TITLE_PREFIXES: [&str; 2] = ["New session - ", "Child session - "];

pub(crate) fn database_path() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".local/share")))?;
    Some(data_home.join("opencode/opencode.db"))
}

pub(crate) fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    Ok(connection)
}

#[cfg(test)]
pub(crate) fn list_sessions_in(path: &Path) -> rusqlite::Result<Vec<SourceEntry>> {
    list_sessions_within(path, &AgeFilter::default())
}

pub(crate) fn list_sessions_within(
    path: &Path,
    age: &AgeFilter,
) -> rusqlite::Result<Vec<SourceEntry>> {
    let connection = open_read_only(path)?;
    let (age_clause, age_params) = age_clause(age);
    let mut statement = connection.prepare(&format!(
        "SELECT id, directory, title,
                COALESCE((SELECT MAX(time_created) FROM message
                          WHERE message.session_id = session.id
                            AND json_extract(message.data, '$.role') = 'user'),
                         time_updated)
         FROM session
         WHERE parent_id IS NULL AND time_archived IS NULL{age_clause}
         ORDER BY 4 DESC"
    ))?;
    let rows: Vec<(String, String, String, i64)> = statement
        .query_map(params_from_iter(age_params), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut entries = Vec::with_capacity(rows.len());
    for (session_id, cwd, title, last_user_message) in rows {
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
            last_used: UNIX_EPOCH + Duration::from_millis(last_user_message.max(0) as u64),
        });
    }
    Ok(entries)
}

fn age_clause(age: &AgeFilter) -> (String, Vec<SqlValue>) {
    let Some(cutoff) = age.cutoff() else {
        return (String::new(), Vec::new());
    };
    let cutoff_millis = cutoff
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let mut values = vec![SqlValue::Integer(cutoff_millis)];
    values.extend(age.keep_ids().iter().cloned().map(SqlValue::Text));
    let kept = vec!["?"; values.len() - 1].join(",");
    let clause = if kept.is_empty() {
        " AND time_updated >= ?".to_owned()
    } else {
        format!(" AND (time_updated >= ? OR id IN ({kept}))")
    };
    (clause, values)
}

#[cfg(test)]
pub(crate) fn load_transcript(path: &Path, session_id: &str) -> rusqlite::Result<Session> {
    let connection = open_read_only(path)?;
    load_transcript_with(&connection, session_id)
}

pub(crate) struct DatabaseFollower {
    path: PathBuf,
    session_id: String,
    fingerprint: Option<Fingerprint>,
    window: Option<Window>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    messages: i64,
    parts: i64,
    latest_update: i64,
}

type RowKey = (i64, String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Window {
    lower: Option<RowKey>,
    has_earlier: bool,
}

impl Window {
    fn earlier(&self) -> Option<Cursor> {
        let (created, id) = self.lower.clone().filter(|_| self.has_earlier)?;
        Some(Cursor::Row { created, id })
    }
}

pub(crate) struct Loaded {
    pub(crate) messages: Vec<Message>,
    pub(crate) earlier: Option<Cursor>,
}

impl DatabaseFollower {
    pub(crate) fn new(path: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            session_id: session_id.into(),
            fingerprint: None,
            window: None,
        }
    }

    pub(crate) fn poll(&mut self, latest: usize) -> rusqlite::Result<Option<Loaded>> {
        let connection = open_read_only(&self.path)?;
        let current = fingerprint(&connection, &self.session_id)?;
        if self.fingerprint == Some(current) {
            return Ok(None);
        }
        let window = match self.window.clone() {
            Some(window) => window,
            None => page_window(&connection, &self.session_id, None, latest)?,
        };
        let messages =
            load_range(&connection, &self.session_id, window.lower.as_ref(), None)?.messages;
        let earlier = window.earlier();
        self.window = Some(window);
        self.fingerprint = Some(current);
        Ok(Some(Loaded { messages, earlier }))
    }

    pub(crate) fn earlier(&mut self, before: &Cursor, count: usize) -> rusqlite::Result<Loaded> {
        let Cursor::Row { created, id } = before else {
            return Ok(Loaded {
                messages: Vec::new(),
                earlier: Some(before.clone()),
            });
        };
        let upper = (*created, id.clone());
        let connection = open_read_only(&self.path)?;
        let window = page_window(&connection, &self.session_id, Some(&upper), count)?;
        let messages = load_range(
            &connection,
            &self.session_id,
            window.lower.as_ref(),
            Some(&upper),
        )?
        .messages;
        let earlier = window.earlier();
        if self.window.as_ref().and_then(|w| w.lower.as_ref()) == Some(&upper) {
            self.window = Some(window);
        }
        Ok(Loaded { messages, earlier })
    }
}

pub(crate) fn new_session(cwd: &Path) -> CommandSpec {
    command(&[], cwd)
}

pub(crate) fn resume_session(session_id: &str, cwd: &Path) -> CommandSpec {
    command(&["--session", session_id], cwd)
}

pub(crate) fn fork_session(session_id: &str, cwd: &Path) -> CommandSpec {
    command(&["--session", session_id, "--fork"], cwd)
}

pub(crate) fn continue_latest(cwd: &Path) -> CommandSpec {
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

fn page_window(
    connection: &Connection,
    session_id: &str,
    upper: Option<&RowKey>,
    count: usize,
) -> rusqlite::Result<Window> {
    let mut statement = connection.prepare(
        "SELECT time_created, id FROM message
         WHERE session_id = ?1 AND (?2 IS NULL OR (time_created, id) < (?2, ?3))
         ORDER BY time_created DESC, id DESC LIMIT 2 OFFSET ?4",
    )?;
    let rows: Vec<RowKey> = statement
        .query_map(
            params![
                session_id,
                upper.map(|key| key.0),
                upper.map(|key| key.1.as_str()),
                count.saturating_sub(1) as i64
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<_, _>>()?;
    Ok(Window {
        has_earlier: rows.len() == 2,
        lower: rows.into_iter().next(),
    })
}

#[cfg(test)]
fn load_transcript_with(connection: &Connection, session_id: &str) -> rusqlite::Result<Session> {
    load_range(connection, session_id, None, None)
}

fn load_range(
    connection: &Connection,
    session_id: &str,
    lower: Option<&RowKey>,
    upper: Option<&RowKey>,
) -> rusqlite::Result<Session> {
    let chunks = load_chunks(connection, session_id, lower, upper)?;
    let mut session = Session {
        malformed_lines: chunks.malformed,
        ..Session::default()
    };
    for chunk in chunks.chunks {
        session.messages.extend(chunk.messages);
    }
    Ok(session)
}

pub(crate) struct MessageChunk {
    pub(crate) created: i64,
    pub(crate) id: String,
    pub(crate) messages: Vec<Message>,
}

pub(crate) struct MessageChunks {
    pub(crate) chunks: Vec<MessageChunk>,
    pub(crate) malformed: usize,
}

pub(crate) fn load_all_chunks(
    connection: &Connection,
    session_id: &str,
) -> rusqlite::Result<MessageChunks> {
    load_chunks(connection, session_id, None, None)
}

fn load_chunks(
    connection: &Connection,
    session_id: &str,
    lower: Option<&RowKey>,
    upper: Option<&RowKey>,
) -> rusqlite::Result<MessageChunks> {
    let bounds = params![
        session_id,
        lower.map(|key| key.0),
        lower.map(|key| key.1.as_str()),
        upper.map(|key| key.0),
        upper.map(|key| key.1.as_str()),
    ];
    let mut parts_by_message: HashMap<String, Vec<Value>> = HashMap::new();
    let mut part_statement = connection.prepare(
        "SELECT part.message_id, part.data FROM part
         JOIN message ON message.id = part.message_id
         WHERE part.session_id = ?1
           AND (?2 IS NULL OR (message.time_created, message.id) >= (?2, ?3))
           AND (?4 IS NULL OR (message.time_created, message.id) < (?4, ?5))
         ORDER BY part.message_id, part.id",
    )?;
    for row in part_statement.query_map(bounds, |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (message_id, data) = row?;
        if let Ok(part) = serde_json::from_str(&data) {
            parts_by_message.entry(message_id).or_default().push(part);
        }
    }

    let mut loaded = MessageChunks {
        chunks: Vec::new(),
        malformed: 0,
    };
    let mut message_statement = connection.prepare(
        "SELECT id, time_created, data FROM message
         WHERE session_id = ?1
           AND (?2 IS NULL OR (time_created, id) >= (?2, ?3))
           AND (?4 IS NULL OR (time_created, id) < (?4, ?5))
         ORDER BY time_created, id",
    )?;
    for row in message_statement.query_map(bounds, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (id, created, data) = row?;
        let Ok(message): Result<Value, _> = serde_json::from_str(&data) else {
            loaded.malformed += 1;
            continue;
        };
        let parts = parts_by_message.remove(&id).unwrap_or_default();
        loaded.chunks.push(MessageChunk {
            created,
            id,
            messages: convert_message(&message, &parts),
        });
    }
    Ok(loaded)
}

pub(crate) fn load_message(
    connection: &Connection,
    session_id: &str,
    message_id: &str,
) -> rusqlite::Result<Option<Vec<Message>>> {
    let data: Option<String> = connection
        .query_row(
            "SELECT data FROM message WHERE id = ?1 AND session_id = ?2",
            params![message_id, session_id],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(message) = data.and_then(|data| serde_json::from_str::<Value>(&data).ok()) else {
        return Ok(None);
    };
    let mut statement =
        connection.prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY id")?;
    let parts: Vec<Value> = statement
        .query_map(params![message_id], |row| row.get::<_, String>(0))?
        .filter_map(|data| serde_json::from_str(&data.ok()?).ok())
        .collect();
    Ok(Some(convert_message(&message, &parts)))
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
            offset: None,
            blocks,
        });
    }
    if !results.is_empty() {
        converted.push(Message {
            role: Role::User,
            timestamp,
            is_meta: false,
            offset: None,
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
                ("ses_a", "Named chat", TitleSource::Agent),
                ("ses_b", "hello there", TitleSource::Prompt),
            ]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn last_used_is_the_latest_user_message_else_the_session_update_time() {
        let path = fixture("recency");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "INSERT INTO message VALUES ('m5','ses_b',500,500,'{\"role\":\"user\"}');
                 INSERT INTO message VALUES ('m6','ses_b',900,900,'{\"role\":\"assistant\"}');",
            )
            .unwrap();
        let used: HashMap<String, u64> = list_sessions_in(&path)
            .unwrap()
            .into_iter()
            .map(|e| {
                let millis = e.last_used.duration_since(UNIX_EPOCH).unwrap().as_millis();
                (e.session_id, millis as u64)
            })
            .collect();
        assert_eq!(used["ses_b"], 500);
        assert_eq!(used["ses_a"], 20);
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
        assert_eq!(follower.poll(200).unwrap().unwrap().messages.len(), 3);
        assert!(follower.poll(200).unwrap().is_none());

        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "INSERT INTO message VALUES ('m4','ses_b',4,4,'{\"role\":\"user\"}');
                 INSERT INTO part VALUES ('p6','m4','ses_b',6,6,'{\"type\":\"text\",\"text\":\"more\"}');",
            )
            .unwrap();
        assert_eq!(follower.poll(200).unwrap().unwrap().messages.len(), 4);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn paging_back_from_the_latest_messages_reproduces_the_whole_session() {
        let path = fixture("paging");
        let connection = Connection::open(&path).unwrap();
        for index in 0..25 {
            connection
                .execute(
                    "INSERT INTO message VALUES (?1, 'ses_p', ?2, ?2, '{\"role\":\"user\"}')",
                    params![format!("pm{index:02}"), index / 2],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO part VALUES (?1, ?2, 'ses_p', ?3, ?3, ?4)",
                    params![
                        format!("pp{index:02}"),
                        format!("pm{index:02}"),
                        index,
                        format!("{{\"type\":\"text\",\"text\":\"msg {index}\"}}")
                    ],
                )
                .unwrap();
        }
        let text_of = |messages: &[Message]| -> Vec<String> {
            messages
                .iter()
                .flat_map(|message| &message.blocks)
                .filter_map(|block| match block {
                    Block::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let expected = text_of(&load_transcript(&path, "ses_p").unwrap().messages);
        assert_eq!(expected.len(), 25);

        let mut follower = DatabaseFollower::new(&path, "ses_p");
        let mut loaded = follower.poll(10).unwrap().unwrap();
        let mut pages = vec![text_of(&loaded.messages)];
        assert_eq!(pages[0].len(), 10);
        while let Some(cursor) = loaded.earlier.clone() {
            loaded = follower.earlier(&cursor, 10).unwrap();
            pages.push(text_of(&loaded.messages));
        }
        assert_eq!(pages.iter().map(Vec::len).collect::<Vec<_>>(), [10, 10, 5]);
        let paged: Vec<String> = pages.into_iter().rev().flatten().collect();
        assert_eq!(paged, expected);
        fs::remove_file(path).unwrap();
    }
}
