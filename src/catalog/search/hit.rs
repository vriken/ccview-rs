use super::{
    db::{file_id_of, file_row, offset_of},
    extract::{Columns, columns_of},
    indexer::matches_record,
    opencode, parser_for,
};
use crate::session::{cursor::Cursor, harness_id::HarnessId};
use rusqlite::Connection;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn cursor_of(conn: &Connection, harness: HarnessId, rowid: i64) -> Option<Cursor> {
    match harness {
        HarnessId::OpenCode => opencode::cursor_at(conn, file_id_of(rowid), offset_of(rowid)),
        _ => Some(Cursor::Offset(offset_of(rowid))),
    }
}

pub(crate) fn columns_at(
    conn: &Connection,
    harness: HarnessId,
    path: &Path,
    rowid: i64,
) -> Option<Columns> {
    let file_id = file_id_of(rowid);
    let offset = offset_of(rowid);
    if harness == HarnessId::OpenCode {
        return opencode::columns_at(conn, file_id, path, offset);
    }
    let record = file_row(conn, file_id).ok()??;
    let length = fs::metadata(path).ok()?.len();
    if offset >= record.indexed_bytes || !matches_record(&record, path, length) {
        return None;
    }
    let parser = parser_for(harness.as_str())?;
    let line = read_line(path, offset)?;
    Some(columns_of(&parser(&line).ok()?))
}

fn read_line(path: &Path, offset: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut line = Vec::new();
    BufReader::new(file.take(MAX_LINE_BYTES))
        .read_until(b'\n', &mut line)
        .ok()?;
    if line.last() != Some(&b'\n') {
        return None;
    }
    String::from_utf8(line).ok()
}
