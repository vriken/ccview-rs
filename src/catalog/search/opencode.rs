use super::{
    db::{FileRow, SearchDb, rowid},
    extract::{Columns, columns_of},
};
use crate::{
    session::cursor::Cursor,
    sources::opencode::{load_all_chunks, load_message, open_read_only},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub(crate) const SOURCE: &str = "opencode";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCodeSession {
    pub(crate) database: PathBuf,
    pub(crate) session_id: String,
}

impl OpenCodeSession {
    pub(crate) fn key(&self) -> String {
        self.database
            .join(&self.session_id)
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stamp {
    rows: i64,
    updated: i64,
}

pub(crate) fn stamps(source: &Connection) -> rusqlite::Result<HashMap<String, Stamp>> {
    let mut stamps: HashMap<String, Stamp> = HashMap::new();
    for table in ["message", "part"] {
        let mut statement = source.prepare(&format!(
            "SELECT session_id, COUNT(*), COALESCE(MAX(time_updated), 0) FROM {table} GROUP BY session_id"
        ))?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (session_id, rows, updated) = row?;
            let stamp = stamps.entry(session_id).or_insert(Stamp {
                rows: 0,
                updated: 0,
            });
            stamp.rows += rows;
            stamp.updated = stamp.updated.max(updated);
        }
    }
    Ok(stamps)
}

pub(crate) fn index_session(
    conn: &mut Connection,
    source: &Connection,
    session: &OpenCodeSession,
    stamp: Stamp,
    stored: Option<&FileRow>,
) -> rusqlite::Result<()> {
    if stored
        .is_some_and(|row| row.size == stamp.rows as u64 && row.mtime_ns == stamp.updated as u64)
    {
        return Ok(());
    }
    let Ok(loaded) = load_all_chunks(source, &session.session_id) else {
        return Ok(());
    };
    let tx = conn.transaction()?;
    let file_id = match stored {
        Some(row) => {
            SearchDb::delete_messages(&tx, row.id)?;
            row.id
        }
        None => {
            tx.execute(
                "INSERT INTO file(key, harness, size, mtime_ns, indexed_bytes, tail_len, tail_hash)
                 VALUES (?1, ?2, 0, 0, 0, 0, 0)",
                params![session.key(), SOURCE],
            )?;
            tx.last_insert_rowid()
        }
    };
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO msg(rowid, user, assistant, tool) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut ordinal = tx.prepare_cached(
            "INSERT INTO ordinal(file_id, n, created, id) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (number, chunk) in loaded.chunks.iter().enumerate() {
            let columns = columns_of(&chunk.messages);
            if columns.is_empty() {
                continue;
            }
            let number = number as u64;
            insert.execute(params![
                rowid(file_id, number),
                columns.user,
                columns.assistant,
                columns.tool
            ])?;
            ordinal.execute(params![file_id, number as i64, chunk.created, chunk.id])?;
        }
    }
    tx.execute(
        "UPDATE file SET size = ?2, mtime_ns = ?3, indexed_bytes = ?4 WHERE id = ?1",
        params![
            file_id,
            stamp.rows,
            stamp.updated,
            loaded.chunks.len() as i64
        ],
    )?;
    tx.commit()
}

pub(crate) fn cursor_at(conn: &Connection, file_id: i64, ordinal: u64) -> Option<Cursor> {
    let (created, id) = message_key(conn, file_id, ordinal)?;
    Some(Cursor::Row { created, id })
}

pub(crate) fn columns_at(
    conn: &Connection,
    file_id: i64,
    path: &Path,
    ordinal: u64,
) -> Option<Columns> {
    let (_, id) = message_key(conn, file_id, ordinal)?;
    let session_id = path.file_name()?.to_str()?;
    let source = open_read_only(path.parent()?).ok()?;
    let messages = load_message(&source, session_id, &id).ok()??;
    Some(columns_of(&messages))
}

fn message_key(conn: &Connection, file_id: i64, ordinal: u64) -> Option<(i64, String)> {
    conn.query_row(
        "SELECT created, id FROM ordinal WHERE file_id = ?1 AND n = ?2",
        params![file_id, ordinal as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .ok()?
}
