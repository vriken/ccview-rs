use rusqlite::{Connection, ErrorCode, params};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const FORMAT_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const OFFSET_BITS: u32 = 40;

pub(crate) struct FileRow {
    pub(crate) id: i64,
    pub(crate) size: u64,
    pub(crate) mtime_ns: u64,
    pub(crate) indexed_bytes: u64,
    pub(crate) tail_len: u64,
    pub(crate) tail_hash: u64,
}

pub(crate) struct SearchDb {
    path: PathBuf,
    pub(crate) conn: Connection,
}

pub(crate) fn rowid(file_id: i64, offset: u64) -> i64 {
    (file_id << OFFSET_BITS) | offset as i64
}

pub(crate) fn is_corrupt(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

impl SearchDb {
    pub(crate) fn open(path: PathBuf) -> rusqlite::Result<Self> {
        match Self::try_open(&path) {
            Ok(Some(db)) => Ok(db),
            Ok(None) => Self::rebuilt(&path),
            Err(error) if is_corrupt(&error) => Self::rebuilt(&path),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn reset(&mut self) -> rusqlite::Result<()> {
        drop(std::mem::replace(
            &mut self.conn,
            Connection::open_in_memory()?,
        ));
        *self = Self::rebuilt(&self.path.clone())?;
        Ok(())
    }

    fn rebuilt(path: &Path) -> rusqlite::Result<Self> {
        remove_database(path);
        Self::try_open(path)?.ok_or(rusqlite::Error::InvalidQuery)
    }

    fn try_open(path: &Path) -> rusqlite::Result<Option<Self>> {
        crate::store::create_private_file(path)
            .map_err(|_| rusqlite::Error::InvalidPath(path.to_path_buf()))?;
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => {
                conn.execute_batch(&format!(
                    "BEGIN;
                     CREATE TABLE IF NOT EXISTS file(
                        id INTEGER PRIMARY KEY, key TEXT UNIQUE, harness TEXT, size INTEGER,
                        mtime_ns INTEGER, indexed_bytes INTEGER, tail_len INTEGER, tail_hash INTEGER);
                     CREATE VIRTUAL TABLE IF NOT EXISTS msg USING fts5(
                        user, assistant, tool, content='', contentless_delete=1,
                        tokenize='unicode61 remove_diacritics 2');
                     PRAGMA user_version = {FORMAT_VERSION};
                     COMMIT;"
                ))?;
            }
            FORMAT_VERSION => {}
            _ => return Ok(None),
        }
        Ok(Some(Self {
            path: path.to_path_buf(),
            conn,
        }))
    }

    pub(crate) fn files(&self) -> rusqlite::Result<Vec<(String, FileRow)>> {
        self.conn
            .prepare(
                "SELECT key, id, size, mtime_ns, indexed_bytes, tail_len, tail_hash FROM file",
            )?
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    FileRow {
                        id: row.get(1)?,
                        size: row.get::<_, i64>(2)? as u64,
                        mtime_ns: row.get::<_, i64>(3)? as u64,
                        indexed_bytes: row.get::<_, i64>(4)? as u64,
                        tail_len: row.get::<_, i64>(5)? as u64,
                        tail_hash: row.get::<_, i64>(6)? as u64,
                    },
                ))
            })?
            .collect()
    }

    pub(crate) fn delete_messages(conn: &Connection, file_id: i64) -> rusqlite::Result<()> {
        let first = rowid(file_id, 0);
        let last = rowid(file_id, (1 << OFFSET_BITS) - 1);
        conn.execute(
            "DELETE FROM msg WHERE rowid BETWEEN ?1 AND ?2",
            params![first, last],
        )
        .map(drop)
    }

    #[cfg(test)]
    pub(crate) fn hits(&self, query: &str) -> Vec<i64> {
        self.conn
            .prepare("SELECT rowid FROM msg WHERE msg MATCH ?1 ORDER BY rowid")
            .unwrap()
            .query_map([query], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}

fn remove_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = fs::remove_file(name);
    }
}
