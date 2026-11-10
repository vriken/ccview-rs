use super::{
    feed::{Feed, Update},
    index::IndexCache,
    scanner::{CLAUDE_SOURCE, Scanner},
    worker::{CodexWatch, scan_claude, scan_codex},
};
use crate::sources::{AgeFilter, opencode};
use std::{
    collections::HashSet,
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, SystemTime},
};

const DAY: Duration = Duration::from_secs(86_400);
const LIMIT_DAYS: u32 = 30;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ccview-age-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        restore_permissions(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn restore_permissions(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o755));
        if entry.path().is_dir() {
            restore_permissions(&entry.path());
        }
    }
}

fn write_aged(path: &Path, content: &str, age: Duration) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

fn claude_file(projects: &Dir, id: &str, age: Duration) -> PathBuf {
    let path = projects.path(&format!("-w-app/{id}.jsonl"));
    write_aged(&path, r#"{"type":"user","cwd":"/w/app"}"#, age);
    path
}

fn limit(keep: &[&str]) -> AgeFilter {
    AgeFilter::new(
        Some(DAY * LIMIT_DAYS),
        keep.iter().map(|id| (*id).to_owned()).collect(),
    )
}

fn feed() -> (Feed, mpsc::Receiver<Update>) {
    let (sender, receiver) = mpsc::channel();
    let feed = Feed {
        sender,
        wake: Arc::new(|| {}),
    };
    (feed, receiver)
}

fn drained(receiver: &mpsc::Receiver<Update>) -> (Vec<String>, Vec<PathBuf>) {
    let mut scanned = Vec::new();
    let mut removed = Vec::new();
    for update in receiver.try_iter() {
        match update {
            Update::Scanned(entries) => {
                scanned.extend(entries.into_iter().map(|entry| entry.session_id))
            }
            Update::Removed(paths) => removed.extend(paths),
            _ => {}
        }
    }
    scanned.sort();
    (scanned, removed)
}

#[test]
fn a_transcript_just_outside_the_limit_is_neither_listed_nor_opened() {
    let projects = Dir::new("claude-edge");
    claude_file(&projects, "inside", 29 * DAY + DAY * 23 / 24);
    let outside = claude_file(&projects, "outside", 30 * DAY + DAY / 24);
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o000)).unwrap();
    let (feed, receiver) = feed();
    let mut scanner = Scanner::default();
    scanner.set_age(limit(&[]));

    assert!(scan_claude(&mut scanner, &feed, &projects.0));

    let (scanned, _) = drained(&receiver);
    assert_eq!(scanned, vec!["inside"]);
    assert_eq!(scanner.files_opened, 1);
}

#[test]
fn starred_important_or_running_ids_load_however_old_and_no_limit_loads_all() {
    let projects = Dir::new("claude-keep");
    for id in ["kept", "dropped"] {
        claude_file(&projects, id, 200 * DAY);
    }
    let (feed, receiver) = feed();
    let mut scanner = Scanner::default();
    scanner.set_age(limit(&["kept"]));
    assert!(scan_claude(&mut scanner, &feed, &projects.0));
    assert_eq!(drained(&receiver).0, vec!["kept"]);

    scanner.set_age(AgeFilter::default());
    assert!(scan_claude(&mut scanner, &feed, &projects.0));
    assert_eq!(drained(&receiver).0, vec!["dropped"]);
}

#[test]
fn changing_the_limit_removes_rows_and_restores_them_and_the_index_follows() {
    let projects = Dir::new("claude-setting");
    let index_path = projects.path("cache/index.json");
    let old = claude_file(&projects, "old", 90 * DAY);
    claude_file(&projects, "new", DAY);
    let (feed, receiver) = feed();
    let mut scanner = Scanner::with_index(IndexCache::load(index_path.clone()));
    scanner.set_age(AgeFilter::default());
    assert!(scan_claude(&mut scanner, &feed, &projects.0));
    scanner.persist();
    assert_eq!(drained(&receiver).0, vec!["new", "old"]);
    assert!(
        IndexCache::load(index_path.clone())
            .get(CLAUDE_SOURCE, &old)
            .is_some()
    );

    scanner.set_age(limit(&[]));
    assert!(scan_claude(&mut scanner, &feed, &projects.0));
    scanner.persist();
    assert_eq!(drained(&receiver).1, vec![old.clone()]);
    assert!(
        IndexCache::load(index_path.clone())
            .get(CLAUDE_SOURCE, &old)
            .is_none()
    );

    scanner.set_age(AgeFilter::default());
    assert!(scan_claude(&mut scanner, &feed, &projects.0));
    assert_eq!(drained(&receiver).0, vec!["old"]);
}

#[test]
fn codex_rollouts_follow_the_same_rule_and_the_id_comes_from_the_file_name() {
    let home = Dir::new("codex");
    let rollout = |id: &str, age: Duration| {
        write_aged(
            &home.path(&format!(
                "sessions/2026/01/01/rollout-2026-01-01T10-00-00-{id}.jsonl"
            )),
            &format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"/w/app"}}}}"#),
            age,
        );
    };
    rollout("fresh", DAY);
    rollout("stale", 90 * DAY);
    rollout("stale-kept", 90 * DAY);
    let (feed, receiver) = feed();
    let mut scanner = Scanner::default();
    scanner.set_age(limit(&["stale-kept"]));

    assert!(scan_codex(
        &mut scanner,
        &feed,
        &home.0,
        &mut CodexWatch::default()
    ));

    assert_eq!(drained(&receiver).0, vec!["fresh", "stale-kept"]);
    assert_eq!(scanner.files_opened, 2);
}

#[test]
fn opencode_rows_follow_the_same_rule_inside_the_query() {
    let dir = Dir::new("opencode");
    let database = dir.path("opencode.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, parent_id TEXT, title TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .unwrap();
    let millis = |age: Duration| {
        (SystemTime::now() - age)
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    };
    for (id, age) in [
        ("ses_fresh", DAY),
        ("ses_stale", 90 * DAY),
        ("ses_kept", 90 * DAY),
    ] {
        connection
            .execute(
                "INSERT INTO session VALUES (?1, '/w/app', NULL, 'chat', ?2, NULL)",
                rusqlite::params![id, millis(age)],
            )
            .unwrap();
    }
    drop(connection);

    let ids = |age: &AgeFilter| -> HashSet<String> {
        opencode::list_sessions_within(&database, age)
            .unwrap()
            .into_iter()
            .map(|entry| entry.session_id)
            .collect()
    };

    assert_eq!(
        ids(&limit(&["ses_kept"])),
        HashSet::from(["ses_fresh".to_owned(), "ses_kept".to_owned()])
    );
    assert_eq!(ids(&AgeFilter::default()).len(), 3);
}
