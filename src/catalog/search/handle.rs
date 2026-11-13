use super::{
    db::open_reader,
    extract::Columns,
    hit,
    query::Query,
    rank::{BodyHit, Ranked, SnapshotEntry, Tier, rank},
    snippet::{Snippet, snippet},
};
use crate::session::cursor::Cursor;
use rusqlite::Connection;
use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
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

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct Hit {
    pub(crate) session_id: String,
    pub(crate) at: Cursor,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchResult {
    pub(crate) session_id: String,
    pub(crate) tier: Tier,
    pub(crate) matching_messages: u32,
    #[allow(dead_code)]
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

pub(crate) struct SearchHandle {
    requests: Sender<Request>,
    responses: Receiver<Response>,
}

impl SearchHandle {
    pub(crate) fn spawn(database: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (requests, incoming) = mpsc::channel::<Request>();
        let (outgoing, responses) = mpsc::channel::<Response>();
        thread::spawn(move || {
            let mut searcher = Searcher::new(database);
            while let Ok(first) = incoming.recv() {
                let request = incoming.try_iter().last().unwrap_or(first);
                let mut response =
                    searcher.search(&request.text, &request.snapshot, SystemTime::now());
                response.generation = request.generation;
                if outgoing.send(response).is_err() {
                    return;
                }
                wake();
            }
        });
        Self {
            requests,
            responses,
        }
    }

    pub(crate) fn query(&self, text: String, snapshot: Arc<[SnapshotEntry]>, generation: u64) {
        let _ = self.requests.send(Request {
            text,
            snapshot,
            generation,
        });
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
