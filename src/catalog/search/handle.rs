use super::{
    db::{FileRow, offset_of, open_reader, rowid},
    extract::Columns,
    hit,
    indexer::matches_record,
    query::Query,
    rank::{BodyHit, Ranked, SnapshotEntry, Tier, rank},
    snippet::{Snippet, snippet},
};
use crate::session::cursor::Cursor;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::SystemTime,
};

const SNIPPET_RESULTS: usize = 50;
const HITS_SQL: &str = "
    WITH h AS MATERIALIZED (
        SELECT rowid AS r, bm25(msg, 4.0, 2.0, 1.0) AS s FROM msg WHERE msg MATCH ?1)
    SELECT file.key, COUNT(*), MIN(s), r FROM h JOIN file ON file.id = (r >> 40)
    GROUP BY file.id";

const NEXT_SQL: &str =
    "SELECT rowid FROM msg WHERE msg MATCH ?1 AND rowid BETWEEN ?2 AND ?3 ORDER BY rowid LIMIT 1";
const PREVIOUS_SQL: &str = "SELECT rowid FROM msg WHERE msg MATCH ?1 AND rowid BETWEEN ?2 AND ?3 ORDER BY rowid DESC LIMIT 1";

#[derive(Debug, Clone)]
pub(crate) struct Hit {
    pub(crate) session_id: String,
    pub(crate) at: Cursor,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchResult {
    pub(crate) session_id: String,
    pub(crate) tier: Tier,
    pub(crate) matching_messages: u32,
    pub(crate) hit: Option<Hit>,
    pub(crate) snippet: Option<Snippet>,
}

#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) generation: u64,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) body_searched: bool,
}

struct Request {
    text: String,
    snapshot: Arc<[SnapshotEntry]>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    At,
    Next,
    Previous,
}

#[derive(Debug, Clone)]
pub(crate) struct Locate {
    pub(crate) token: u64,
    pub(crate) text: String,
    pub(crate) path: PathBuf,
    pub(crate) from: Option<u64>,
    pub(crate) which: Which,
}

#[derive(Debug)]
pub(crate) struct Located {
    pub(crate) token: u64,
    pub(crate) offset: Option<u64>,
    pub(crate) intact: bool,
}

enum Incoming {
    Search(Request),
    Locate(Locate),
}

pub(crate) struct SearchHandle {
    requests: Sender<Incoming>,
    responses: Receiver<Response>,
    located: Receiver<Located>,
}

impl SearchHandle {
    pub(crate) fn spawn(database: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (requests, incoming) = mpsc::channel::<Incoming>();
        let (outgoing, responses) = mpsc::channel::<Response>();
        let (found, located) = mpsc::channel::<Located>();
        thread::spawn(move || {
            let mut searcher = Searcher::new(database);
            while let Ok(first) = incoming.recv() {
                let mut search = None;
                let mut locate = None;
                for request in std::iter::once(first).chain(incoming.try_iter()) {
                    match request {
                        Incoming::Search(request) => search = Some(request),
                        Incoming::Locate(request) => locate = Some(request),
                    }
                }
                if let Some(request) = locate
                    && found.send(searcher.locate(&request)).is_err()
                {
                    return;
                }
                if let Some(request) = search {
                    let mut response =
                        searcher.search(&request.text, &request.snapshot, SystemTime::now());
                    response.generation = request.generation;
                    if outgoing.send(response).is_err() {
                        return;
                    }
                }
                wake();
            }
        });
        Self {
            requests,
            responses,
            located,
        }
    }

    pub(crate) fn query(&self, text: String, snapshot: Arc<[SnapshotEntry]>, generation: u64) {
        let _ = self.requests.send(Incoming::Search(Request {
            text,
            snapshot,
            generation,
        }));
    }

    pub(crate) fn locate(&self, request: Locate) {
        let _ = self.requests.send(Incoming::Locate(request));
    }

    pub(crate) fn located(&self) -> Option<Located> {
        self.located.try_iter().last()
    }

    pub(crate) fn latest(&self) -> Option<Response> {
        self.responses.try_iter().last()
    }
}

pub(crate) struct Searcher {
    path: PathBuf,
    reader: Option<(Connection, u64)>,
}

impl Searcher {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, reader: None }
    }

    pub(crate) fn search(
        &mut self,
        text: &str,
        snapshot: &[SnapshotEntry],
        now: SystemTime,
    ) -> Response {
        let query = Query::parse(text);
        let mut response = Response {
            generation: 0,
            results: Vec::new(),
            body_searched: false,
        };
        if query.is_empty() {
            return response;
        }
        let expression = query.fts();
        let body = match &expression {
            Some(expression) => match self.body_hits(expression) {
                Some(body) => {
                    response.body_searched = true;
                    body
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let ranked = rank(&query, snapshot, &body, now);
        let conn = self.reader.as_ref().map(|(conn, _)| conn);
        response.results = ranked
            .iter()
            .enumerate()
            .map(|(position, ranked)| {
                result_of(conn, &query, snapshot, ranked, position < SNIPPET_RESULTS)
            })
            .collect();
        response
    }

    pub(crate) fn locate(&mut self, request: &Locate) -> Located {
        let missing = Located {
            token: request.token,
            offset: None,
            intact: false,
        };
        let expression = Query::parse(&request.text).fts();
        let (Some(expression), Some(conn)) = (expression, self.reader()) else {
            return missing;
        };
        let Some(file) = file_by_key(conn, &request.path) else {
            return missing;
        };
        let length = std::fs::metadata(&request.path).map_or(0, |meta| meta.len());
        if !matches_record(&file, &request.path, length) {
            return missing;
        }
        let offset = match (request.which, request.from) {
            (Which::At, from) => from.filter(|offset| *offset < file.indexed_bytes),
            (Which::Next, from) => neighbour(conn, &expression, file.id, from, true),
            (Which::Previous, from) => neighbour(conn, &expression, file.id, from, false),
        };
        Located {
            token: request.token,
            offset,
            intact: true,
        }
    }

    fn body_hits(&mut self, expression: &str) -> Option<Vec<BodyHit>> {
        let conn = self.reader()?;
        let hits = Self::query_hits(conn, expression);
        if hits.is_none() {
            self.reader = None;
        }
        hits
    }

    fn query_hits(conn: &Connection, expression: &str) -> Option<Vec<BodyHit>> {
        let mut statement = conn.prepare_cached(HITS_SQL).ok()?;
        let rows = statement
            .query_map([expression], |row| {
                Ok(BodyHit {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    matching_messages: row.get(1)?,
                    bm25: row.get(2)?,
                    rowid: row.get(3)?,
                })
            })
            .ok()?;
        rows.collect::<Result<_, _>>().ok()
    }

    fn reader(&mut self) -> Option<&Connection> {
        let inode = std::fs::metadata(&self.path).ok()?.ino();
        if self
            .reader
            .as_ref()
            .is_none_or(|(_, opened)| *opened != inode)
        {
            self.reader = open_reader(&self.path).ok().map(|conn| (conn, inode));
        }
        self.reader.as_ref().map(|(conn, _)| conn)
    }
}

fn file_by_key(conn: &Connection, path: &Path) -> Option<FileRow> {
    conn.query_row(
        "SELECT id, size, mtime_ns, indexed_bytes, tail_len, tail_hash FROM file WHERE key = ?1",
        [path.to_string_lossy()],
        |row| FileRow::read(row, 0),
    )
    .optional()
    .ok()?
}

fn neighbour(
    conn: &Connection,
    expression: &str,
    file_id: i64,
    from: Option<u64>,
    forward: bool,
) -> Option<u64> {
    let first = rowid(file_id, 0);
    let last = rowid(file_id + 1, 0) - 1;
    let (sql, low, high) = match (forward, from) {
        (true, None) => (NEXT_SQL, first, last),
        (true, Some(from)) => (NEXT_SQL, rowid(file_id, from) + 1, last),
        (false, None) => (PREVIOUS_SQL, first, last),
        (false, Some(from)) => (PREVIOUS_SQL, first, rowid(file_id, from) - 1),
    };
    let found: Option<i64> = conn
        .prepare_cached(sql)
        .ok()?
        .query_row(params![expression, low, high], |row| row.get(0))
        .optional()
        .ok()?;
    found.map(offset_of)
}

fn result_of(
    conn: Option<&Connection>,
    query: &Query,
    snapshot: &[SnapshotEntry],
    ranked: &Ranked,
    with_snippet: bool,
) -> SearchResult {
    let visible = &snapshot[ranked.visible];
    let located = ranked.best_hit.and_then(|(member, rowid)| {
        let conn = conn?;
        let entry = &snapshot[member];
        let at = hit::cursor_of(conn, entry.harness, rowid)?;
        let snippet = with_snippet
            .then(|| hit::columns_at(conn, entry.harness, &entry.path, rowid))
            .flatten()
            .and_then(|columns| snippet_of(&columns, query));
        Some((
            Hit {
                session_id: entry.session_id.clone(),
                at,
            },
            snippet,
        ))
    });
    let (hit, snippet) = match located {
        Some((hit, snippet)) => (Some(hit), snippet),
        None => (None, None),
    };
    SearchResult {
        session_id: visible.session_id.clone(),
        tier: ranked.tier,
        matching_messages: ranked.matching_messages,
        hit,
        snippet,
    }
}

fn snippet_of(columns: &Columns, query: &Query) -> Option<Snippet> {
    let candidates = [
        ("user", &columns.user),
        ("assistant", &columns.assistant),
        ("tool", &columns.tool),
    ];
    let mut fallback = None;
    for (name, text) in candidates {
        let wanted = query.columns.is_empty() || query.columns.contains(&name);
        if !wanted || text.is_empty() {
            continue;
        }
        let found = snippet(text, &query.terms);
        if !found.spans.is_empty() {
            return Some(found);
        }
        fallback.get_or_insert(found);
    }
    fallback
}
