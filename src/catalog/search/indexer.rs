use super::{
    Listed, Listing,
    db::{FileRow, SearchDb, is_corrupt, rowid},
    extract::{LineParser, columns_of},
    opencode::{self, OpenCodeSession},
};
use crate::sources::{claude::scan::tail_window, opencode::open_read_only};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const READ_BUFFER: usize = 64 * 1024;

pub(crate) struct Indexer {
    db: SearchDb,
}

impl Indexer {
    pub(crate) fn open(path: PathBuf) -> rusqlite::Result<Self> {
        Ok(Self {
            db: SearchDb::open(path)?,
        })
    }

    pub(crate) fn sync(
        &mut self,
        listing: &Listing,
        interrupted: &mut dyn FnMut() -> bool,
    ) -> rusqlite::Result<()> {
        match self.sync_once(listing, interrupted) {
            Err(error) if is_corrupt(&error) => {
                self.db.reset()?;
                self.sync_once(listing, interrupted)
            }
            other => other,
        }
    }

    fn sync_once(
        &mut self,
        listing: &Listing,
        interrupted: &mut dyn FnMut() -> bool,
    ) -> rusqlite::Result<()> {
        let stored: HashMap<String, FileRow> = self.db.files()?.into_iter().collect();
        let listed: HashSet<String> = listing
            .files
            .iter()
            .map(key_of)
            .chain(listing.opencode.iter().map(OpenCodeSession::key))
            .collect();
        for (key, row) in &stored {
            if !listed.contains(key) {
                forget(&mut self.db.conn, row.id)?;
            }
        }
        index_opencode(&mut self.db.conn, &listing.opencode, &stored, interrupted)?;
        let mut newest_first: Vec<&Listed> = listing.files.iter().collect();
        newest_first.sort_by_key(|listed| std::cmp::Reverse(listed.file.modified));
        for file in newest_first {
            if interrupted() {
                break;
            }
            index_file(&mut self.db.conn, file, stored.get(&key_of(file)))?;
        }
        Ok(())
    }
}

fn index_opencode(
    conn: &mut Connection,
    sessions: &[OpenCodeSession],
    stored: &HashMap<String, FileRow>,
    interrupted: &mut dyn FnMut() -> bool,
) -> rusqlite::Result<()> {
    let mut by_database: HashMap<&Path, Vec<&OpenCodeSession>> = HashMap::new();
    for session in sessions {
        by_database
            .entry(session.database.as_path())
            .or_default()
            .push(session);
    }
    for (database, sessions) in by_database {
        let Ok(source) = open_read_only(database) else {
            continue;
        };
        let Ok(stamps) = opencode::stamps(&source) else {
            continue;
        };
        for session in sessions {
            if interrupted() {
                return Ok(());
            }
            let stamp = stamps.get(&session.session_id).copied().unwrap_or_default();
            opencode::index_session(conn, &source, session, stamp, stored.get(&session.key()))?;
        }
    }
    Ok(())
}

fn key_of(listed: &Listed) -> String {
    listed.file.path.to_string_lossy().into_owned()
}

fn nanos(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as i64)
}

fn forget(conn: &mut Connection, file_id: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    SearchDb::delete_messages(&tx, file_id)?;
    tx.execute("DELETE FROM file WHERE id = ?1", [file_id])?;
    tx.commit()
}

fn index_file(
    conn: &mut Connection,
    listed: &Listed,
    row: Option<&FileRow>,
) -> rusqlite::Result<()> {
    let unchanged = |row: &FileRow| {
        row.size == listed.file.len && row.mtime_ns as i64 == nanos(listed.file.modified)
    };
    if row.is_some_and(unchanged) {
        return Ok(());
    }
    let Ok(mut file) = File::open(&listed.file.path) else {
        return Ok(());
    };
    let Ok(metadata) = file.metadata() else {
        return Ok(());
    };
    let len = metadata.len();
    let mtime_ns = nanos(metadata.modified().unwrap_or(UNIX_EPOCH));
    let tx = conn.transaction()?;
    let (file_id, start) = match row {
        Some(row) if resumable(row, listed, len) => (row.id, row.indexed_bytes),
        Some(row) => {
            SearchDb::delete_messages(&tx, row.id)?;
            (row.id, 0)
        }
        None => {
            tx.execute(
                "INSERT INTO file(key, harness, size, mtime_ns, indexed_bytes, tail_len, tail_hash)
                 VALUES (?1, ?2, 0, 0, 0, 0, 0)",
                params![key_of(listed), listed.source],
            )?;
            (tx.last_insert_rowid(), 0)
        }
    };
    let Some(end) = index_lines(&tx, &mut file, listed.parser, file_id, start, len)? else {
        return Ok(());
    };
    let Some((tail_len, tail_hash)) = tail_window(&listed.file.path, end) else {
        return Ok(());
    };
    tx.execute(
        "UPDATE file SET size = ?2, mtime_ns = ?3, indexed_bytes = ?4, tail_len = ?5, tail_hash = ?6
         WHERE id = ?1",
        params![
            file_id,
            len as i64,
            mtime_ns,
            end as i64,
            tail_len as i64,
            tail_hash as i64
        ],
    )?;
    tx.commit()
}

fn resumable(row: &FileRow, listed: &Listed, len: u64) -> bool {
    row.indexed_bytes > 0 && matches_record(row, &listed.file.path, len)
}

pub(crate) fn matches_record(row: &FileRow, path: &Path, len: u64) -> bool {
    len >= row.indexed_bytes
        && tail_window(path, row.indexed_bytes) == Some((row.tail_len, row.tail_hash))
}

fn index_lines(
    conn: &Connection,
    file: &mut File,
    parser: LineParser,
    file_id: i64,
    start: u64,
    len: u64,
) -> rusqlite::Result<Option<u64>> {
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Ok(None);
    }
    let mut reader = BufReader::with_capacity(READ_BUFFER, file.take(len.saturating_sub(start)));
    let mut insert = conn
        .prepare_cached("INSERT INTO msg(rowid, user, assistant, tool) VALUES (?1, ?2, ?3, ?4)")?;
    let mut offset = start;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => return Ok(None),
        };
        if line.last() != Some(&b'\n') {
            break;
        }
        if let Ok(text) = std::str::from_utf8(&line)
            && let Ok(messages) = parser(text)
        {
            let columns = columns_of(&messages);
            if !columns.is_empty() {
                insert.execute(params![
                    rowid(file_id, offset),
                    columns.user,
                    columns.assistant,
                    columns.tool
                ])?;
            }
        }
        offset += read as u64;
    }
    Ok(Some(offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::scanner::CLAUDE_SOURCE,
        sources::{
            AgeFilter, SessionFile, claude::scan::list_files_within,
            claude::transcript::parse_claude_line,
        },
    };
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::PermissionsExt,
        path::Path,
        time::Duration,
    };

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccview-search-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn user_line(text: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"content":"{text}"}}}}{}"#,
            "\n"
        )
    }

    fn listed(path: &Path) -> Listed {
        let metadata = fs::metadata(path).unwrap();
        Listed {
            source: CLAUDE_SOURCE,
            parser: parse_claude_line,
            file: SessionFile {
                path: path.to_path_buf(),
                modified: metadata.modified().unwrap(),
                len: metadata.len(),
            },
        }
    }

    fn sync(indexer: &mut Indexer, paths: &[&Path]) {
        let files: Vec<Listed> = paths.iter().map(|path| listed(path)).collect();
        indexer.sync(&Listing::files(files), &mut || false).unwrap();
    }

    #[test]
    fn an_appended_message_becomes_searchable_without_duplicating_earlier_ones() {
        let dir = scratch("append");
        let transcript = dir.join("s.jsonl");
        fs::write(&transcript, user_line("zebrafish migration")).unwrap();
        let mut indexer = Indexer::open(dir.join("search.sqlite")).unwrap();
        sync(&mut indexer, &[&transcript]);
        let first = indexer.db.hits("zebrafish");
        assert_eq!(first.len(), 1);

        OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap()
            .write_all(user_line("quokka rollout").as_bytes())
            .unwrap();
        sync(&mut indexer, &[&transcript]);
        assert_eq!(indexer.db.hits("quokka").len(), 1);
        assert_eq!(indexer.db.hits("zebrafish"), first);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rewritten_shorter_file_no_longer_matches_its_old_text() {
        let dir = scratch("rewrite");
        let transcript = dir.join("s.jsonl");
        let old = user_line("zebrafish migration") + &user_line("narwhal checkpoint");
        fs::write(&transcript, old).unwrap();
        let mut indexer = Indexer::open(dir.join("search.sqlite")).unwrap();
        sync(&mut indexer, &[&transcript]);
        assert_eq!(indexer.db.hits("narwhal").len(), 1);

        fs::write(&transcript, user_line("quokka")).unwrap();
        sync(&mut indexer, &[&transcript]);
        assert!(indexer.db.hits("zebrafish").is_empty());
        assert!(indexer.db.hits("narwhal").is_empty());
        assert_eq!(indexer.db.hits("quokka").len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_outside_the_age_filter_is_not_indexed_and_lowering_the_limit_purges_it() {
        let dir = scratch("age");
        let projects = dir.join("projects/p");
        fs::create_dir_all(&projects).unwrap();
        let recent = projects.join("recent.jsonl");
        let old = projects.join("old.jsonl");
        fs::write(&recent, user_line("zebrafish")).unwrap();
        fs::write(&old, user_line("narwhal")).unwrap();
        let sixty_days = Duration::from_secs(60 * 86_400);
        OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - sixty_days)
            .unwrap();
        let mut indexer = Indexer::open(dir.join("search.sqlite")).unwrap();
        let sync_within = |indexer: &mut Indexer, days: u64| {
            let age = AgeFilter::new(Some(Duration::from_secs(days * 86_400)), Default::default());
            let files: Vec<Listed> = list_files_within(&dir.join("projects"), &age)
                .into_iter()
                .map(|file| listed(&file.path))
                .collect();
            indexer.sync(&Listing::files(files), &mut || false).unwrap();
        };

        sync_within(&mut indexer, 30);
        assert_eq!(indexer.db.hits("zebrafish").len(), 1);
        assert!(indexer.db.hits("narwhal").is_empty());

        sync_within(&mut indexer, 90);
        assert_eq!(indexer.db.hits("narwhal").len(), 1);

        sync_within(&mut indexer, 30);
        assert!(indexer.db.hits("narwhal").is_empty());
        assert_eq!(indexer.db.hits("zebrafish").len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_database_its_wal_and_the_directory_are_private_to_the_owner() {
        let dir = scratch("modes");
        let transcript = dir.join("s.jsonl");
        fs::write(&transcript, user_line("zebrafish")).unwrap();
        let database = dir.join("cache/ccview-rs/search-v1.sqlite");
        let mut indexer = Indexer::open(database.clone()).unwrap();
        sync(&mut indexer, &[&transcript]);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let wal = dir.join("cache/ccview-rs/search-v1.sqlite-wal");
        assert!(wal.exists());
        assert_eq!(mode(&database), 0o600);
        assert_eq!(mode(&wal), 0o600);
        assert_eq!(mode(database.parent().unwrap()), 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_database_file_is_rebuilt_and_indexing_then_works() {
        let dir = scratch("corrupt");
        let transcript = dir.join("s.jsonl");
        fs::write(&transcript, user_line("zebrafish")).unwrap();
        let database = dir.join("search.sqlite");
        fs::write(&database, vec![b'x'; 8192]).unwrap();
        let mut indexer = Indexer::open(database).unwrap();
        sync(&mut indexer, &[&transcript]);
        assert_eq!(indexer.db.hits("zebrafish").len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
