use super::{
    feed::{Feed, Update},
    index::IndexCache,
    live,
    scanner::{CLAUDE_SOURCE, CODEX_SOURCE, Scanner},
    search::{self, Listed},
};
use crate::{
    session::entry::SessionEntry,
    sources::{
        AgeFilter, SessionFile,
        claude::scan::list_files_within,
        codex::{self, list_codex_files_within},
        opencode,
    },
    sys::file_stamp::{FileStamp, stamp_of},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, SystemTime},
};

const SCAN_BATCH: usize = 16;
const SOURCES_REFRESH: Duration = Duration::from_secs(1);

pub(crate) fn spawn(
    projects_dir: PathBuf,
    age: AgeFilter,
    wake: impl Fn() + Send + Sync + 'static,
) -> (Receiver<Update>, Sender<AgeFilter>) {
    let (sender, receiver) = mpsc::channel();
    let (age_sender, age_changes) = mpsc::channel::<AgeFilter>();
    let feed = Feed {
        sender,
        wake: Arc::new(wake),
    };
    let codex_home = codex::codex_home();
    let mut scanner = Scanner::with_index(IndexCache::load_default());
    scanner.set_age(age);
    let mut cached = scanner.cached_entries();
    if let Some(home) = &codex_home {
        cached.extend(scanner.cached_codex_entries(&codex::thread_names(home)));
    }
    if !cached.is_empty() && !feed.send(Update::Scanned(cached)) {
        return (receiver, age_sender);
    }
    live::spawn(feed.clone(), projects_dir.clone());
    thread::spawn(move || {
        let mut first_pass = true;
        let mut codex_watch = CodexWatch::default();
        let mut opencode_watch = OpenCodeWatch::default();
        let mut indexer = None;
        let mut sent_listing = Vec::new();
        loop {
            if !scan_claude(&mut scanner, &feed, &projects_dir) {
                return;
            }
            if let Some(home) = &codex_home
                && !scan_codex(&mut scanner, &feed, home, &mut codex_watch)
            {
                return;
            }
            if !scan_opencode(&feed, scanner.age(), &mut opencode_watch) {
                return;
            }
            scanner.persist();
            if first_pass {
                first_pass = false;
                if !feed.send(Update::InitialScanDone) {
                    return;
                }
                indexer = Some(search::spawn_indexer(search::default_path()));
            }
            if let Some(sender) = &indexer {
                let listing = scanner.listed();
                let signature = signature_of(&listing);
                if signature != sent_listing {
                    if sender.send(listing).is_err() {
                        indexer = None;
                    }
                    sent_listing = signature;
                }
            }
            match age_changes.recv_timeout(SOURCES_REFRESH) {
                Ok(first) => {
                    let age = age_changes.try_iter().last().unwrap_or(first);
                    if age != *scanner.age() {
                        scanner.set_age(age);
                        opencode_watch.database_stamps.clear();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => thread::sleep(SOURCES_REFRESH),
            }
        }
    });
    (receiver, age_sender)
}

pub(super) fn scan_claude(scanner: &mut Scanner, feed: &Feed, projects_dir: &Path) -> bool {
    let files = list_files_within(projects_dir, scanner.age());
    if projects_dir.is_dir() {
        let removed = scanner.forget_missing(CLAUDE_SOURCE, &files);
        if !removed.is_empty() && !feed.send(Update::Removed(removed)) {
            return false;
        }
    }
    let stale: Vec<&SessionFile> = files
        .iter()
        .filter(|file| !scanner.is_fresh(CLAUDE_SOURCE, file))
        .collect();
    for chunk in stale.chunks(SCAN_BATCH) {
        let entries = chunk.iter().map(|file| scanner.scan(file)).collect();
        if !feed.send(Update::Scanned(entries)) {
            return false;
        }
    }
    true
}

fn signature_of(listing: &[Listed]) -> Vec<(PathBuf, u64, SystemTime)> {
    listing
        .iter()
        .map(|listed| {
            (
                listed.file.path.clone(),
                listed.file.len,
                listed.file.modified,
            )
        })
        .collect()
}

type EntrySnapshot = HashMap<PathBuf, (String, String, SystemTime)>;

fn snapshot_of(entries: &[SessionEntry]) -> EntrySnapshot {
    entries
        .iter()
        .map(|entry| {
            (
                entry.path.clone(),
                (entry.title.clone(), entry.cwd.clone(), entry.last_used),
            )
        })
        .collect()
}

fn changed_since(entries: Vec<SessionEntry>, seen: &EntrySnapshot) -> Vec<SessionEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            let key = (entry.title.clone(), entry.cwd.clone(), entry.last_used);
            seen.get(&entry.path) != Some(&key)
        })
        .collect()
}

#[derive(Default)]
pub(super) struct CodexWatch {
    names_stamp: Option<FileStamp>,
    names: HashMap<String, String>,
}

impl CodexWatch {
    fn thread_names(&mut self, codex_home: &Path) -> &HashMap<String, String> {
        let stamp = stamp_of(&codex_home.join(codex::THREAD_INDEX_FILE));
        if stamp != self.names_stamp {
            self.names = codex::thread_names(codex_home);
            self.names_stamp = stamp;
        }
        &self.names
    }
}

pub(super) fn scan_codex(
    scanner: &mut Scanner,
    feed: &Feed,
    codex_home: &Path,
    watch: &mut CodexWatch,
) -> bool {
    let files = list_codex_files_within(codex_home, scanner.age());
    let removed = scanner.forget_missing(CODEX_SOURCE, &files);
    if !removed.is_empty() && !feed.send(Update::Removed(removed)) {
        return false;
    }
    let thread_names = watch.thread_names(codex_home).clone();
    let names_changed = scanner.codex_thread_names != thread_names;
    let mut entries = Vec::new();
    for file in &files {
        if names_changed || !scanner.is_fresh(CODEX_SOURCE, file) {
            entries.extend(scanner.scan_codex(file, &thread_names));
        }
    }
    scanner.codex_thread_names = thread_names;
    entries.is_empty() || feed.send(Update::Scanned(entries))
}

#[derive(Default)]
struct OpenCodeWatch {
    database_stamps: Vec<Option<FileStamp>>,
    seen: EntrySnapshot,
}

fn database_stamps(database: &Path) -> Vec<Option<FileStamp>> {
    let mut write_ahead_log = database.as_os_str().to_owned();
    write_ahead_log.push("-wal");
    vec![stamp_of(database), stamp_of(Path::new(&write_ahead_log))]
}

fn scan_opencode(feed: &Feed, age: &AgeFilter, watch: &mut OpenCodeWatch) -> bool {
    let Some(database) = opencode::database_path().filter(|path| path.is_file()) else {
        return true;
    };
    let stamps = database_stamps(&database);
    if stamps == watch.database_stamps {
        return true;
    }
    let Ok(listed) = opencode::list_sessions_within(&database, age) else {
        return true;
    };
    watch.database_stamps = stamps;
    let entries: Vec<SessionEntry> = listed.into_iter().map(SessionEntry::from).collect();
    let current = snapshot_of(&entries);
    let removed: Vec<PathBuf> = watch
        .seen
        .keys()
        .filter(|path| !current.contains_key(*path))
        .cloned()
        .collect();
    let changed = changed_since(entries, &watch.seen);
    watch.seen = current;
    (removed.is_empty() || feed.send(Update::Removed(removed)))
        && (changed.is_empty() || feed.send(Update::Scanned(changed)))
}
