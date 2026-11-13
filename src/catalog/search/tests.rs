use super::{
    Listed, Listing,
    handle::Searcher,
    indexer::Indexer,
    opencode::OpenCodeSession,
    rank::{SnapshotEntry, Tier},
};
use crate::{
    catalog::scanner::CLAUDE_SOURCE,
    session::{cursor::Cursor, harness_id::HarnessId},
    sources::{SessionFile, claude::transcript::parse_claude_line},
};
use rusqlite::{Connection, params};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

struct Fixture {
    dir: PathBuf,
    indexer: Indexer,
    searcher: Searcher,
    snapshot: Vec<SnapshotEntry>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ccview-query-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let database = dir.join("cache/search.sqlite");
        Self {
            indexer: Indexer::open(database.clone()).unwrap(),
            searcher: Searcher::new(database),
            snapshot: Vec::new(),
            dir,
        }
    }

    fn session(&mut self, id: &str, title: &str, copy_of: Option<&str>, lines: &[&str]) -> PathBuf {
        let path = self.dir.join(format!("{id}.jsonl"));
        let body: String = lines.iter().map(|text| user_line(text)).collect();
        fs::write(&path, body).unwrap();
        self.snapshot.push(SnapshotEntry {
            path: path.clone(),
            session_id: id.to_owned(),
            title: title.to_owned(),
            cwd: "/work/app".to_owned(),
            harness: HarnessId::Claude,
            last_used: SystemTime::now(),
            copy_of: copy_of.map(str::to_owned),
        });
        path
    }

    fn index(&mut self, opencode: Vec<OpenCodeSession>) {
        let files = self
            .snapshot
            .iter()
            .filter(|entry| entry.harness == HarnessId::Claude)
            .map(|entry| listed(&entry.path))
            .collect();
        self.indexer
            .sync(&Listing { files, opencode }, &mut || false)
            .unwrap();
    }

    fn search(&mut self, text: &str) -> super::handle::Response {
        self.searcher
            .search(text, &self.snapshot, SystemTime::now())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
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

#[test]
fn a_title_match_ranks_above_a_session_with_more_body_hits() {
    let mut fixture = Fixture::new("tier");
    fixture.session(
        "body",
        "unrelated chore",
        None,
        &["zebrafish one", "zebrafish two", "zebrafish three"],
    );
    fixture.session("named", "zebrafish migration", None, &["nothing to see"]);
    let filler = vec!["alpha beta"; 40];
    fixture.session("filler", "other chore", None, &filler);
    fixture.index(Vec::new());

    let response = fixture.search("zebrafish");

    let order: Vec<(&str, Tier)> = response
        .results
        .iter()
        .map(|result| (result.session_id.as_str(), result.tier))
        .collect();
    assert_eq!(order, [("named", Tier::Name), ("body", Tier::Body)]);
    let body = &response.results[1];
    assert_eq!(body.matching_messages, 3);
    let snippet = body.snippet.as_ref().unwrap();
    let marked: Vec<String> = snippet
        .spans
        .iter()
        .map(|span| snippet.text[span.clone()].to_lowercase())
        .collect();
    assert_eq!(marked, ["zebrafish"]);
}

#[test]
fn an_abbreviation_finds_the_title_but_scattered_letters_do_not() {
    let mut fixture = Fixture::new("fuzzy");
    fixture.session("api", "abn-api migration", None, &["x"]);
    fixture.session(
        "garden",
        "my little garden is rather tiny; I opened nine",
        None,
        &["x"],
    );
    fixture.index(Vec::new());

    let found = fixture.search("abnapi");
    let ids: Vec<(&str, Tier)> = found
        .results
        .iter()
        .map(|result| (result.session_id.as_str(), result.tier))
        .collect();
    assert_eq!(ids, [("api", Tier::Fuzzy)]);

    assert!(
        fixture
            .search("migration")
            .results
            .iter()
            .all(|r| r.session_id == "api")
    );
    assert!(fixture.search("qzxkj").results.is_empty());
}

#[test]
fn hostile_input_never_raises_an_fts_error_or_panics() {
    let mut fixture = Fixture::new("hostile");
    fixture.session("s", "title", None, &["do not break foo bar", "plain words"]);
    fixture.index(Vec::new());
    let long = "word ".repeat(2000);
    let cases: [(&str, &str); 14] = [
        ("quote inside a word", "foo\"bar"),
        ("open paren", "("),
        ("operator word", "NOT"),
        ("operator word between terms", "foo OR bar AND NOT"),
        ("dangling field", "a:"),
        ("known field with no value", "title:"),
        ("lone dash", "-"),
        ("lone star", "*"),
        ("unterminated phrase", "\"foo bar"),
        ("column filter syntax", "{user} : foo"),
        ("near syntax", "NEAR(foo bar)"),
        ("only spaces", "   "),
        ("empty", ""),
        ("very long", &long),
    ];
    for (name, input) in cases {
        let response = fixture.search(input);
        let has_terms = !super::query::Query::parse(input).terms.is_empty();
        assert_eq!(response.body_searched, has_terms, "{name}");
    }
    assert_eq!(fixture.search("foo\"bar").results.len(), 1);
    assert_eq!(fixture.search("NOT").results.len(), 1);
    assert!(fixture.search("(").results.is_empty());
}

#[test]
fn a_lineage_counts_shared_messages_once_and_targets_the_visible_member() {
    let mut fixture = Fixture::new("lineage");
    let shared = ["quokka one", "quokka two", "quokka three"];
    fixture.session("older", "old copy", Some("newer"), &shared);
    let mut grown = shared.to_vec();
    grown.extend(["quokka four", "quokka five"]);
    fixture.session("newer", "new copy", None, &grown);
    fixture.index(Vec::new());

    let response = fixture.search("quokka");

    assert_eq!(response.results.len(), 1);
    let result = &response.results[0];
    assert_eq!(result.session_id, "newer");
    assert_eq!(result.matching_messages, 5);
    assert_eq!(result.hit.as_ref().unwrap().session_id, "newer");
}

#[test]
fn an_opencode_session_is_searchable_without_writing_to_its_database() {
    let mut fixture = Fixture::new("opencode");
    let database = fixture.dir.join("opencode.db");
    let source = Connection::open(&database).unwrap();
    source
        .execute_batch(
            "CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);",
        )
        .unwrap();
    let messages = [
        ("m1", 1000, "user", "hello there"),
        ("m2", 2000, "user", "rebuild the narwhal cache"),
    ];
    for (id, created, role, text) in messages {
        source
            .execute(
                "INSERT INTO message VALUES (?1, 'ses_1', ?2, ?2, ?3)",
                params![
                    id,
                    created,
                    format!(r#"{{"role":"{role}","time":{{"created":{created}}}}}"#)
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES (?1, ?2, 'ses_1', ?3, ?3, ?4)",
                params![
                    format!("p_{id}"),
                    id,
                    created,
                    format!(r#"{{"type":"text","text":"{text}"}}"#)
                ],
            )
            .unwrap();
    }
    drop(source);
    let before = fs::read(&database).unwrap();
    fixture.snapshot.push(SnapshotEntry {
        path: database.join("ses_1"),
        session_id: "ses_1".to_owned(),
        title: "cache work".to_owned(),
        cwd: "/work/app".to_owned(),
        harness: HarnessId::OpenCode,
        last_used: SystemTime::now() - Duration::from_secs(60),
        copy_of: None,
    });
    fixture.index(vec![OpenCodeSession {
        database: database.clone(),
        session_id: "ses_1".to_owned(),
    }]);

    let response = fixture.search("narwhal");

    assert_eq!(response.results.len(), 1);
    let result = &response.results[0];
    assert_eq!(result.session_id, "ses_1");
    let hit = result.hit.as_ref().unwrap();
    assert_eq!(
        hit.at,
        Cursor::Row {
            created: 2000,
            id: "m2".to_owned()
        }
    );
    let snippet = result.snippet.as_ref().unwrap();
    assert_eq!(&snippet.text[snippet.spans[0].clone()], "narwhal");
    assert_eq!(fs::read(&database).unwrap(), before);
}
