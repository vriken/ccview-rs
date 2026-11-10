use crate::{
    activity::ActivityMap,
    index_cache::{FileRecord, IndexCache},
    lineage::{self, Fingerprint},
    live,
    parse::{self, Block, Role},
    registry::LiveSession,
    stamp::{Stamp, stamp_of},
    sources::{
        HarnessId, SourceEntry, TranscriptHandle,
        codex::{self, RolloutHead},
        opencode,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CUSTOM_TITLE_PREFIX: &[u8] = br#"{"type":"custom-title""#;
const AGENT_NAME_PREFIX: &[u8] = br#"{"type":"agent-name""#;
const AI_TITLE_PREFIX: &[u8] = br#"{"type":"ai-title""#;
const PROMPT_SEARCH_LINE_LIMIT: usize = 300;
const PROMPT_MAX_CHARS: usize = 160;
const CLAUDE_SOURCE: &str = "claude";
const CODEX_SOURCE: &str = "codex";
const TAIL_WINDOW_BYTES: u64 = 4096;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const SHORT_ID_CHARS: usize = 8;
const SCAN_BATCH: usize = 16;
const SOURCES_REFRESH: Duration = Duration::from_secs(1);
const USER_TYPE_MARKER: &[u8] = br#""type":"user""#;
const TOOL_RESULT_MARKER: &[u8] = b"\"tool_result\"";
const RECENT_USER_LINES: usize = 8;
const IGNORED_PROMPT_PREFIXES: [&str; 3] = ["<", "[Request interrupted", "Caveat:"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSource {
    Custom,
    Agent,
    Derived,
    Prompt,
    SessionId,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub harness: HarnessId,
    pub handle: TranscriptHandle,
    pub path: PathBuf,
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub title_source: TitleSource,
    pub last_prompt: Option<String>,
    pub last_used: SystemTime,
    pub live: Option<LiveSession>,
    pub fingerprint: Option<Arc<Fingerprint>>,
    pub copy_of: Option<String>,
    pub copies: Vec<String>,
}

#[cfg(test)]
impl SessionEntry {
    pub fn stub() -> Self {
        Self {
            harness: HarnessId::Claude,
            handle: TranscriptHandle::Rollout(PathBuf::new()),
            path: PathBuf::new(),
            session_id: String::new(),
            cwd: String::new(),
            title: String::new(),
            title_source: TitleSource::SessionId,
            last_prompt: None,
            last_used: UNIX_EPOCH,
            live: None,
            fingerprint: None,
            copy_of: None,
            copies: Vec::new(),
        }
    }
}

impl SessionEntry {
    fn with_fingerprint(mut self, fingerprint: Fingerprint) -> Self {
        self.fingerprint = (!fingerprint.is_empty()).then(|| Arc::new(fingerprint));
        self
    }
}

impl From<SourceEntry> for SessionEntry {
    fn from(entry: SourceEntry) -> Self {
        Self {
            harness: entry.harness,
            path: entry.handle.entry_key(),
            handle: entry.handle,
            session_id: entry.session_id,
            cwd: entry.cwd,
            title: entry.title,
            title_source: entry.title_source,
            last_prompt: None,
            last_used: entry.last_used,
            live: None,
            fingerprint: None,
            copy_of: None,
            copies: Vec::new(),
        }
    }
}

pub enum Update {
    Scanned(Vec<SessionEntry>),
    Removed(Vec<PathBuf>),
    InitialScanDone,
    Live(Vec<LiveSession>),
    Activity(ActivityMap),
}

#[derive(Default)]
pub struct Library {
    entries: Vec<SessionEntry>,
    live: Vec<LiveSession>,
    raw_activity: ActivityMap,
    activity: ActivityMap,
    expanded: bool,
    groups: Vec<lineage::Group>,
}

impl Library {
    pub fn activity(&self) -> &ActivityMap {
        &self.activity
    }

    pub fn set_activity(&mut self, activity: ActivityMap) {
        self.raw_activity = activity;
        self.derive_activity();
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub fn live(&self) -> &[LiveSession] {
        &self.live
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.apply_groups();
    }

    pub fn is_collapsed_copy(&self, entry: &SessionEntry) -> bool {
        !self.expanded && entry.copy_of.is_some()
    }

    pub fn merge(&mut self, scanned: Vec<SessionEntry>) {
        let mut positions: HashMap<PathBuf, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.path.clone(), position))
            .collect();
        for mut entry in scanned {
            entry.live = live_for(&self.live, &entry.session_id);
            match positions.get(&entry.path) {
                Some(&position) => self.entries[position] = entry,
                None => {
                    positions.insert(entry.path.clone(), self.entries.len());
                    self.entries.push(entry);
                }
            }
        }
        self.entries
            .sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| a.path.cmp(&b.path)));
        self.regroup();
    }

    pub fn remove(&mut self, paths: &[PathBuf]) {
        let gone: HashSet<&PathBuf> = paths.iter().collect();
        self.entries.retain(|entry| !gone.contains(&entry.path));
        self.regroup();
    }

    pub fn set_live(&mut self, live: Vec<LiveSession>) {
        self.live = live;
        self.apply_groups();
    }

    fn regroup(&mut self) {
        let members: Vec<lineage::Member> = self
            .entries
            .iter()
            .map(|entry| lineage::Member {
                path: &entry.path,
                last_used: entry.last_used,
                fingerprint: entry.fingerprint.as_deref(),
            })
            .collect();
        self.groups = lineage::group(&members)
            .into_iter()
            .filter(|group| !group.copies.is_empty())
            .collect();
        self.apply_groups();
    }

    fn apply_groups(&mut self) {
        for entry in &mut self.entries {
            entry.live = live_for(&self.live, &entry.session_id);
            entry.copy_of = None;
            entry.copies.clear();
        }
        let live: Vec<Option<LiveSession>> = self.entries.iter().map(|entry| entry.live.clone()).collect();
        for group in &self.groups {
            let visible_id = self.entries[group.visible].session_id.clone();
            for &copy in &group.copies {
                self.entries[copy].copy_of = Some(visible_id.clone());
            }
            if self.expanded {
                continue;
            }
            let copy_ids: Vec<String> = group.copies.iter().map(|&copy| self.entries[copy].session_id.clone()).collect();
            let visible = &mut self.entries[group.visible];
            visible.copies = copy_ids;
            if visible.live.is_none() {
                visible.live = group.copies.iter().find_map(|&copy| live[copy].clone());
            }
        }
        self.derive_activity();
    }

    fn derive_activity(&mut self) {
        let mut effective = self.raw_activity.clone();
        if !self.expanded {
            for entry in self.entries.iter().filter(|entry| !entry.copies.is_empty()) {
                let strongest = std::iter::once(&entry.session_id)
                    .chain(&entry.copies)
                    .filter_map(|id| self.raw_activity.get(id))
                    .max_by_key(|activity| (activity.is_active(), activity.state.precedence()))
                    .cloned();
                for id in &entry.copies {
                    effective.remove(id);
                }
                match strongest {
                    Some(activity) => effective.insert(entry.session_id.clone(), activity),
                    None => effective.remove(&entry.session_id),
                };
            }
        }
        self.activity = effective;
    }
}

fn live_for(live: &[LiveSession], session_id: &str) -> Option<LiveSession> {
    live.iter()
        .find(|session| session.session_id.as_deref() == Some(session_id))
        .cloned()
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub cwd: Option<String>,
    pub custom_title: Option<String>,
    pub agent_name: Option<String>,
    pub derived_title: Option<String>,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Deserialize)]
struct CwdOnly {
    cwd: Option<String>,
}

#[derive(Default)]
pub struct Scanner {
    index: IndexCache,
    codex_thread_names: HashMap<String, String>,
    pub bytes_read: u64,
}

impl Scanner {
    pub fn with_index(index: IndexCache) -> Self {
        Self { index, codex_thread_names: HashMap::new(), bytes_read: 0 }
    }

    pub fn scan(&mut self, file: &SessionFile) -> SessionEntry {
        let record = self.record_for(file);
        entry_from(file, record.meta).with_fingerprint(record.fingerprint)
    }

    pub fn scan_codex(
        &mut self,
        file: &SessionFile,
        thread_names: &HashMap<String, String>,
    ) -> Option<SessionEntry> {
        let cached = self.index.get(CODEX_SOURCE, &file.path).cloned();
        let record = match cached.filter(|record| matches_stat(record, file)) {
            Some(record) => record,
            None => {
                let head = codex::read_rollout_head(&file.path);
                let record = FileRecord {
                    size: file.len,
                    mtime_ns: nanos_since_epoch(file.modified),
                    meta: head.map(codex_meta).unwrap_or_default(),
                    ..FileRecord::default()
                };
                self.index.insert(CODEX_SOURCE, &file.path, record.clone());
                record
            }
        };
        codex_entry(file, &record.meta, thread_names)
    }

    pub fn cached_codex_entries(&self, thread_names: &HashMap<String, String>) -> Vec<SessionEntry> {
        self.index
            .records(CODEX_SOURCE)
            .filter_map(|(path, record)| {
                let file = SessionFile {
                    path,
                    modified: UNIX_EPOCH + Duration::from_nanos(record.mtime_ns),
                    len: record.size,
                };
                codex_entry(&file, &record.meta, thread_names)
            })
            .collect()
    }

    pub fn is_fresh(&self, source: &str, file: &SessionFile) -> bool {
        self.index
            .get(source, &file.path)
            .is_some_and(|record| matches_stat(record, file))
    }

    pub fn cached_entries(&self) -> Vec<SessionEntry> {
        self.index
            .records(CLAUDE_SOURCE)
            .map(|(path, record)| {
                let file = SessionFile {
                    path,
                    modified: UNIX_EPOCH + Duration::from_nanos(record.mtime_ns),
                    len: record.size,
                };
                entry_from(&file, record.meta.clone()).with_fingerprint(record.fingerprint.clone())
            })
            .collect()
    }

    pub fn forget_missing(&mut self, source: &str, files: &[SessionFile]) -> Vec<PathBuf> {
        let present: HashSet<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
        self.index.remove_missing(source, &present)
    }

    pub fn persist(&mut self) {
        let _ = self.index.save_if_dirty();
    }

    fn record_for(&mut self, file: &SessionFile) -> FileRecord {
        let cached = self.index.get(CLAUDE_SOURCE, &file.path).cloned();
        if let Some(record) = cached.as_ref().filter(|record| matches_stat(record, file)) {
            return record.clone();
        }
        let mut record = match cached {
            Some(record) if self.prefix_intact(&record, file) => record,
            _ => FileRecord::default(),
        };
        self.bytes_read += extend_record(&file.path, &mut record);
        record.size = file.len;
        record.mtime_ns = nanos_since_epoch(file.modified);
        self.index.insert(CLAUDE_SOURCE, &file.path, record.clone());
        record
    }

    fn prefix_intact(&mut self, record: &FileRecord, file: &SessionFile) -> bool {
        let grew = file.len > record.size && file.len >= record.scanned_bytes;
        if !grew || record.scanned_bytes == 0 {
            return false;
        }
        self.bytes_read += record.tail_len;
        tail_window(&file.path, record.scanned_bytes)
            .is_some_and(|tail| tail == (record.tail_len, record.tail_hash))
    }
}

fn codex_meta(head: RolloutHead) -> FileMeta {
    FileMeta {
        session_id: Some(head.session_id),
        cwd: Some(head.cwd),
        first_prompt: head.first_prompt,
        ..FileMeta::default()
    }
}

fn codex_entry(
    file: &SessionFile,
    meta: &FileMeta,
    thread_names: &HashMap<String, String>,
) -> Option<SessionEntry> {
    let head = RolloutHead {
        session_id: meta.session_id.clone()?,
        cwd: meta.cwd.clone().unwrap_or_default(),
        first_prompt: meta.first_prompt.clone(),
    };
    let source = codex::entry_from_head(file.path.clone(), file.modified, head, thread_names);
    Some(source.into())
}

fn matches_stat(record: &FileRecord, file: &SessionFile) -> bool {
    record.size == file.len && record.mtime_ns == nanos_since_epoch(file.modified)
}

fn nanos_since_epoch(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

fn tail_window(path: &Path, end: u64) -> Option<(u64, u64)> {
    let length = end.min(TAIL_WINDOW_BYTES);
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(end - length)).ok()?;
    let mut window = vec![0; length as usize];
    file.read_exact(&mut window).ok()?;
    let hash = window
        .iter()
        .fold(FNV_OFFSET, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME));
    Some((length, hash))
}

#[derive(Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub len: u64,
}

pub fn list_files(projects_dir: &Path) -> Vec<SessionFile> {
    let mut files = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return files;
    };
    for project in projects.flatten() {
        let Ok(children) = fs::read_dir(project.path()) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(metadata) = child.metadata() else {
                continue;
            };
            files.push(SessionFile {
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                len: metadata.len(),
                path,
            });
        }
    }
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    files
}

pub fn list_codex_files(codex_home: &Path) -> Vec<SessionFile> {
    codex::list_rollouts(codex_home)
        .into_iter()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            Some(SessionFile {
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                len: metadata.len(),
                path,
            })
        })
        .collect()
}

pub fn spawn(projects_dir: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Receiver<Update> {
    let (sender, receiver) = mpsc::channel();
    let feed = Feed {
        sender,
        wake: Arc::new(wake),
    };
    let codex_home = codex::codex_home();
    let mut scanner = Scanner::with_index(IndexCache::load_default());
    let mut cached = scanner.cached_entries();
    if let Some(home) = &codex_home {
        cached.extend(scanner.cached_codex_entries(&codex::thread_names(home)));
    }
    if !cached.is_empty() && !feed.send(Update::Scanned(cached)) {
        return receiver;
    }
    live::spawn(feed.clone(), projects_dir.clone());
    thread::spawn(move || {
        let mut first_pass = true;
        let mut codex_watch = CodexWatch::default();
        let mut opencode_watch = OpenCodeWatch::default();
        loop {
            if !scan_claude(&mut scanner, &feed, &projects_dir) {
                return;
            }
            if let Some(home) = &codex_home
                && !scan_codex(&mut scanner, &feed, home, &mut codex_watch)
            {
                return;
            }
            if !scan_opencode(&feed, &mut opencode_watch) {
                return;
            }
            scanner.persist();
            if first_pass {
                first_pass = false;
                if !feed.send(Update::InitialScanDone) {
                    return;
                }
            }
            thread::sleep(SOURCES_REFRESH);
        }
    });
    receiver
}

#[derive(Clone)]
pub struct Feed {
    sender: mpsc::Sender<Update>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Feed {
    pub fn send(&self, update: Update) -> bool {
        let delivered = self.sender.send(update).is_ok();
        if delivered {
            (self.wake)();
        }
        delivered
    }
}

fn scan_claude(scanner: &mut Scanner, feed: &Feed, projects_dir: &Path) -> bool {
    let files = list_files(projects_dir);
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

type EntrySnapshot = HashMap<PathBuf, (String, String, SystemTime)>;

fn snapshot_of(entries: &[SessionEntry]) -> EntrySnapshot {
    entries
        .iter()
        .map(|entry| (entry.path.clone(), (entry.title.clone(), entry.cwd.clone(), entry.last_used)))
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
struct CodexWatch {
    names_stamp: Option<Stamp>,
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

fn scan_codex(scanner: &mut Scanner, feed: &Feed, codex_home: &Path, watch: &mut CodexWatch) -> bool {
    let files = list_codex_files(codex_home);
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
    database_stamps: Vec<Option<Stamp>>,
    seen: EntrySnapshot,
}

fn database_stamps(database: &Path) -> Vec<Option<Stamp>> {
    let mut write_ahead_log = database.as_os_str().to_owned();
    write_ahead_log.push("-wal");
    vec![stamp_of(database), stamp_of(Path::new(&write_ahead_log))]
}

fn scan_opencode(feed: &Feed, watch: &mut OpenCodeWatch) -> bool {
    let Some(database) = opencode::database_path().filter(|path| path.is_file()) else {
        return true;
    };
    let stamps = database_stamps(&database);
    if stamps == watch.database_stamps {
        return true;
    }
    let Ok(listed) = opencode::list_sessions_in(&database) else {
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

fn entry_from(file: &SessionFile, meta: FileMeta) -> SessionEntry {
    let session_id = file
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    let (title, title_source) = choose_title(&meta, &session_id);
    let cwd = meta
        .cwd
        .clone()
        .unwrap_or_else(|| cwd_from_project_dir(&file.path));
    SessionEntry {
        harness: HarnessId::Claude,
        handle: TranscriptHandle::Rollout(file.path.clone()),
        path: file.path.clone(),
        session_id,
        cwd,
        title,
        title_source,
        last_prompt: meta.last_prompt.clone(),
        last_used: file.modified,
        live: None,
        fingerprint: None,
        copy_of: None,
        copies: Vec::new(),
    }
}

fn choose_title(meta: &FileMeta, session_id: &str) -> (String, TitleSource) {
    let candidates = [
        (&meta.custom_title, TitleSource::Custom),
        (&meta.agent_name, TitleSource::Agent),
        (&meta.derived_title, TitleSource::Derived),
        (&meta.first_prompt, TitleSource::Prompt),
    ];
    for (candidate, source) in candidates {
        if let Some(title) = candidate
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty() && !is_date_only(t))
        {
            return (title.to_owned(), source);
        }
    }
    let short_id: String = session_id.chars().take(SHORT_ID_CHARS).collect();
    (short_id, TitleSource::SessionId)
}

fn is_date_only(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn cwd_from_project_dir(path: &Path) -> String {
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    dir_name.replace('-', "/")
}

fn extend_record(path: &Path, record: &mut FileRecord) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    if file.seek(SeekFrom::Start(record.scanned_bytes)).is_err() {
        return 0;
    }
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::new();
    let mut bytes_read = 0u64;
    let mut recent_user_lines: VecDeque<Vec<u8>> = VecDeque::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes_read += read as u64,
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        record.scanned_bytes += line.len() as u64;
        record.lines_scanned += 1;
        record.fingerprint.add_line(&line);
        let meta = &mut record.meta;
        if line.starts_with(CUSTOM_TITLE_PREFIX) {
            meta.custom_title = string_field(&line, "customTitle").or(meta.custom_title.take());
        } else if line.starts_with(AGENT_NAME_PREFIX) {
            meta.agent_name = string_field(&line, "agentName").or(meta.agent_name.take());
        } else if line.starts_with(AI_TITLE_PREFIX) {
            meta.derived_title = string_field(&line, "aiTitle").or(meta.derived_title.take());
        } else if record.lines_scanned <= PROMPT_SEARCH_LINE_LIMIT
            && (meta.first_prompt.is_none() || meta.cwd.is_none())
        {
            read_user_line(&line, meta);
        }
        if contains_bytes(&line, USER_TYPE_MARKER) && !contains_bytes(&line, TOOL_RESULT_MARKER) {
            if recent_user_lines.len() == RECENT_USER_LINES {
                recent_user_lines.pop_front();
            }
            recent_user_lines.push_back(line.clone());
        }
    }
    let latest_prompt = recent_user_lines
        .into_iter()
        .rev()
        .find_map(|user_line| String::from_utf8(user_line).ok().and_then(|text| genuine_prompt(&text)));
    if latest_prompt.is_some() {
        record.meta.last_prompt = latest_prompt;
    }
    let (tail_len, tail_hash) = tail_window(path, record.scanned_bytes).unwrap_or_default();
    record.tail_len = tail_len;
    record.tail_hash = tail_hash;
    bytes_read + tail_len
}

fn string_field(line: &[u8], field: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    value.get(field)?.as_str().map(str::to_owned)
}

fn read_user_line(line: &[u8], meta: &mut FileMeta) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    if !text.contains(r#""type":"user""#) {
        return;
    }
    if meta.cwd.is_none() {
        meta.cwd = serde_json::from_str::<CwdOnly>(text).ok().and_then(|c| c.cwd);
    }
    if meta.first_prompt.is_none() {
        meta.first_prompt = genuine_prompt(text);
    }
}

fn genuine_prompt(line: &str) -> Option<String> {
    let message = parse::parse_line(line).ok()??;
    if message.role != Role::User || message.is_meta {
        return None;
    }
    let text = message.blocks.into_iter().find_map(|block| match block {
        Block::Text(text) if is_genuine_prompt(&text) => Some(text),
        _ => None,
    })?;
    Some(single_line(&text, PROMPT_MAX_CHARS))
}

fn is_genuine_prompt(text: &str) -> bool {
    let trimmed = text.trim_start();
    !trimmed.is_empty()
        && !IGNORED_PROMPT_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
}

fn single_line(text: &str, max_chars: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let kept: String = joined.chars().take(max_chars).collect();
    format!("{kept}…")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };

    fn meta_of(lines: &[&str]) -> FileMeta {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ccview-lib-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        let mut record = FileRecord::default();
        extend_record(&path, &mut record);
        fs::remove_file(&path).unwrap();
        record.meta
    }

    const WRAPPER: &str =
        r#"{"type":"user","cwd":"/work/app","message":{"content":"<command-name>/x</command-name>"}}"#;
    const PROMPT: &str =
        r#"{"type":"user","cwd":"/work/app","message":{"content":"fix   the\nlogin bug"}}"#;

    #[test]
    fn prompt_skips_wrappers_and_collapses_whitespace() {
        let meta = meta_of(&[WRAPPER, PROMPT]);
        assert_eq!(meta.first_prompt.as_deref(), Some("fix the login bug"));
        assert_eq!(meta.cwd.as_deref(), Some("/work/app"));
    }

    #[test]
    fn last_title_wins_and_priority_is_custom_agent_derived_prompt_id() {
        let derived_old = r#"{"type":"ai-title","aiTitle":"old"}"#;
        let derived_new = r#"{"type":"ai-title","aiTitle":"new"}"#;
        let meta = meta_of(&[PROMPT, derived_old, derived_new]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "new");

        let named = r#"{"type":"agent-name","agentName":"agent"}"#;
        let meta = meta_of(&[PROMPT, derived_new, named]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "agent");

        let custom = r#"{"type":"custom-title","customTitle":"mine"}"#;
        let meta = meta_of(&[PROMPT, derived_new, named, custom]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "mine");

        let meta = meta_of(&[PROMPT]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "fix the login bug");

        let meta = meta_of(&[WRAPPER, r#"{"type":"mode"}"#]);
        assert_eq!(
            choose_title(&meta, "abcdef123456"),
            ("abcdef12".to_owned(), TitleSource::SessionId)
        );
    }

    #[test]
    fn date_only_titles_fall_through_to_the_first_prompt() {
        let date_title = r#"{"type":"ai-title","aiTitle":"2026-11-10"}"#;
        let meta = meta_of(&[PROMPT, date_title]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "fix the login bug");
        let blank = r#"{"type":"ai-title","aiTitle":"  "}"#;
        let meta = meta_of(&[PROMPT, blank]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "fix the login bug");
        assert!(is_date_only("2026-11-10") && !is_date_only("2026-11-10 notes") && !is_date_only("2026-1-10"));
    }

    #[test]
    fn a_trailing_interrupt_marker_does_not_erase_the_last_real_prompt() {
        let later = r#"{"type":"user","message":{"content":"now add tests"}}"#;
        let interrupt = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        let meta = meta_of(&[PROMPT, later, interrupt]);
        assert_eq!(meta.last_prompt.as_deref(), Some("now add tests"));
    }

    #[test]
    fn last_prompt_is_the_final_genuine_user_message_not_a_tool_result() {
        let later = r#"{"type":"user","message":{"content":"now add tests"}}"#;
        let tool = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        let meta = meta_of(&[PROMPT, later, tool]);
        assert_eq!(meta.last_prompt.as_deref(), Some("now add tests"));
    }

    pub(super) struct Projects(PathBuf);

    impl Projects {
        pub(super) fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "ccview-proj-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("-w-app")).unwrap();
            Self(root)
        }

        pub(super) fn file(&self, name: &str) -> PathBuf {
            self.0.join("-w-app").join(format!("{name}.jsonl"))
        }

        pub(super) fn append(&self, name: &str, text: &str) {
            use std::io::Write;
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.file(name))
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }

        pub(super) fn files(&self) -> Vec<SessionFile> {
            list_files(&self.0)
        }
    }

    impl Drop for Projects {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn scan_one(scanner: &mut Scanner, projects: &Projects, name: &str) -> SessionEntry {
        let path = projects.file(name);
        let file = projects.files().into_iter().find(|f| f.path == path).unwrap();
        scanner.scan(&file)
    }

    fn fingerprint(entry: &SessionEntry) -> (String, String, TitleSource, Option<String>, SystemTime) {
        (
            entry.cwd.clone(),
            entry.title.clone(),
            entry.title_source,
            entry.last_prompt.clone(),
            entry.last_used,
        )
    }

    #[test]
    fn appended_lines_merge_without_rereading_the_prefix_and_equal_a_full_rescan() {
        let projects = Projects::new();
        let padding = format!(r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#, "x".repeat(20000));
        projects.append("s", &format!("{PROMPT}\n{padding}\n{}\n", r#"{"type":"ai-title","aiTitle":"old"}"#));
        let mut incremental = Scanner::default();
        scan_one(&mut incremental, &projects, "s");
        let first_read = incremental.bytes_read;

        projects.append(
            "s",
            &format!(
                "{}\n{}\n",
                r#"{"type":"custom-title","customTitle":"renamed"}"#,
                r#"{"type":"user","message":{"content":"and then this"}}"#
            ),
        );
        let merged = scan_one(&mut incremental, &projects, "s");
        let full = scan_one(&mut Scanner::default(), &projects, "s");

        assert_eq!(fingerprint(&merged), fingerprint(&full));
        assert_eq!(merged.title, "renamed");
        assert_eq!(merged.last_prompt.as_deref(), Some("and then this"));
        assert!(incremental.bytes_read - first_read < 10_000, "re-read the prefix");
    }

    #[test]
    fn partial_last_line_is_not_consumed_until_it_is_complete() {
        let projects = Projects::new();
        projects.append("s", &format!("{PROMPT}\n"));
        projects.append("s", r#"{"type":"custom-title","customTi"#);
        let mut scanner = Scanner::default();
        assert_eq!(scan_one(&mut scanner, &projects, "s").title, "fix the login bug");

        projects.append("s", "tle\":\"done\"}\n");
        assert_eq!(scan_one(&mut scanner, &projects, "s").title, "done");
    }

    #[test]
    fn rewritten_or_shrunk_files_are_rescanned_in_full() {
        let projects = Projects::new();
        projects.append("s", &format!("{PROMPT}\n"));
        let mut scanner = Scanner::default();
        scan_one(&mut scanner, &projects, "s");

        fs::write(
            projects.file("s"),
            format!("{}\n{}\n", r#"{"type":"user","message":{"content":"totally different start"}}"#, r#"{"type":"mode"}"#),
        )
        .unwrap();
        assert_eq!(scan_one(&mut scanner, &projects, "s").title, "totally different start");
    }

    #[test]
    fn same_length_rewrite_with_new_mtime_is_not_trusted() {
        let projects = Projects::new();
        projects.append("s", &format!("{PROMPT}\n"));
        let mut scanner = Scanner::default();
        scan_one(&mut scanner, &projects, "s");
        let rewritten = format!("{PROMPT}\n").replace("login", "logout");
        fs::write(projects.file("s"), rewritten).unwrap();
        let file = File::options().write(true).open(projects.file("s")).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(scan_one(&mut scanner, &projects, "s").title, "fix the logout bug");
    }

    #[test]
    fn persisted_index_serves_unchanged_files_without_reading_and_orders_by_fresh_mtime() {
        let projects = Projects::new();
        projects.append("old", &format!("{PROMPT}\n"));
        projects.append("new", &format!("{PROMPT}\n"));
        let index_path = projects.0.join("index/index-v1.json");
        let mut first = Scanner::with_index(IndexCache::load(index_path.clone()));
        for file in projects.files() {
            first.scan(&file);
        }
        first.persist();

        let touched = File::options().write(true).open(projects.file("old")).unwrap();
        touched.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
        projects.append("other", &format!("{PROMPT}\n"));

        let mut second = Scanner::with_index(IndexCache::load(index_path));
        assert_eq!(second.cached_entries().len(), 2);
        let files = projects.files();
        assert!(second.is_fresh(CLAUDE_SOURCE, files.iter().find(|f| f.path == projects.file("new")).unwrap()));
        let mut library = Library::default();
        library.merge(second.cached_entries());
        library.merge(files.iter().map(|file| second.scan(file)).collect());

        let order: Vec<&str> = library.entries().iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(order[0], "old");
        assert_eq!(order.len(), 3);
        let before = second.bytes_read;
        scan_one(&mut second, &projects, "new");
        assert_eq!(second.bytes_read, before, "reread an unchanged file");
    }

    #[test]
    fn deleted_files_leave_the_library_and_the_index() {
        let projects = Projects::new();
        projects.append("a", &format!("{PROMPT}\n"));
        projects.append("b", &format!("{PROMPT}\n"));
        let index_path = projects.0.join("index/index-v1.json");
        let mut scanner = Scanner::with_index(IndexCache::load(index_path.clone()));
        let mut library = Library::default();
        library.merge(projects.files().iter().map(|file| scanner.scan(file)).collect());
        scanner.persist();

        fs::remove_file(projects.file("a")).unwrap();
        let removed = scanner.forget_missing(CLAUDE_SOURCE, &projects.files());
        library.remove(&removed);
        scanner.persist();

        assert_eq!(library.entries().len(), 1);
        assert_eq!(Scanner::with_index(IndexCache::load(index_path)).cached_entries().len(), 1);
    }

    fn source_entry(harness: HarnessId, id: &str, modified_secs: u64) -> SessionEntry {
        let handle = match harness {
            HarnessId::OpenCode => TranscriptHandle::Database {
                path: PathBuf::from("/data/opencode.db"),
                session_id: id.to_owned(),
            },
            _ => TranscriptHandle::Rollout(PathBuf::from(format!("/data/{id}.jsonl"))),
        };
        SourceEntry {
            harness,
            session_id: id.to_owned(),
            cwd: "/w".to_owned(),
            title: id.to_owned(),
            title_source: TitleSource::Prompt,
            last_used: UNIX_EPOCH + Duration::from_secs(modified_secs),
            handle,
        }
        .into()
    }

    #[test]
    fn entries_of_every_harness_share_one_list_ordered_by_last_used() {
        let mut library = Library::default();
        library.merge(vec![source_entry(HarnessId::Claude, "claude-old", 10)]);
        library.merge(vec![
            source_entry(HarnessId::OpenCode, "opencode-newest", 30),
            source_entry(HarnessId::Codex, "codex-mid", 20),
        ]);
        library.merge(vec![source_entry(HarnessId::Claude, "claude-old", 40)]);

        let order: Vec<(&str, HarnessId)> = library
            .entries()
            .iter()
            .map(|entry| (entry.session_id.as_str(), entry.harness))
            .collect();
        assert_eq!(
            order,
            [
                ("claude-old", HarnessId::Claude),
                ("opencode-newest", HarnessId::OpenCode),
                ("codex-mid", HarnessId::Codex),
            ]
        );
    }

    #[test]
    fn opencode_sessions_in_one_database_have_distinct_entry_keys() {
        let first = source_entry(HarnessId::OpenCode, "ses_a", 1);
        let second = source_entry(HarnessId::OpenCode, "ses_b", 1);
        assert_ne!(first.path, second.path);
        let mut library = Library::default();
        library.merge(vec![first, second]);
        assert_eq!(library.entries().len(), 2);
    }

    #[test]
    fn codex_rollouts_come_from_the_index_until_they_change_and_names_apply_live() {
        let home = std::env::temp_dir().join(format!("ccview-codex-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/01/01");
        fs::create_dir_all(&day).unwrap();
        let rollout = day.join("rollout-2026-01-01T10-00-00-sess-1.jsonl");
        fs::write(
            &rollout,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"sess-1","cwd":"/work/app"}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the build"}]}}"#, "\n",
            ),
        )
        .unwrap();
        let index_path = home.join("index/index-v1.json");

        let mut first = Scanner::with_index(IndexCache::load(index_path.clone()));
        let file = list_codex_files(&home).remove(0);
        let entry = first.scan_codex(&file, &HashMap::new()).unwrap();
        assert_eq!((entry.harness, entry.session_id.as_str(), entry.cwd.as_str()), (HarnessId::Codex, "sess-1", "/work/app"));
        assert_eq!(entry.title, "fix the build");
        first.persist();

        fs::remove_file(&rollout).unwrap();
        let second = Scanner::with_index(IndexCache::load(index_path));
        let names = HashMap::from([("sess-1".to_owned(), "renamed".to_owned())]);
        let cached = second.cached_codex_entries(&names);
        assert_eq!(cached.len(), 1);
        assert_eq!((cached[0].title.as_str(), cached[0].title_source), ("renamed", TitleSource::Agent));
        assert!(second.is_fresh(CODEX_SOURCE, &file));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_rescans_resend_only_changed_rollouts_or_all_when_names_change() {
        let home = std::env::temp_dir().join(format!("ccview-codex-resend-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/01/01");
        fs::create_dir_all(&day).unwrap();
        for id in ["s1", "s2"] {
            fs::write(
                day.join(format!("rollout-2026-01-01T10-00-00-{id}.jsonl")),
                format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"/work/app"}}}}"#) + "\n",
            )
            .unwrap();
        }
        let (sender, receiver) = mpsc::channel();
        let feed = Feed {
            sender,
            wake: Arc::new(|| {}),
        };
        let mut watch = CodexWatch::default();
        let mut scanner = Scanner::default();
        let sent = |receiver: &Receiver<Update>| -> usize {
            receiver
                .try_iter()
                .map(|update| match update {
                    Update::Scanned(entries) => entries.len(),
                    _ => 0,
                })
                .sum()
        };
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 2);
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 0);
        fs::write(
            day.join("rollout-2026-01-01T10-00-00-s1.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"s1","cwd":"/work/other"}}"#.to_owned() + "\n",
        )
        .unwrap();
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 1);
        fs::write(home.join("session_index.jsonl"), r#"{"id":"s2","thread_name":"renamed"}"#.to_owned() + "\n").unwrap();
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 2);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn merging_thousands_of_entries_again_stays_fast() {
        let entries = |n: usize| -> Vec<SessionEntry> {
            (0..n).map(|i| source_entry(HarnessId::Codex, &format!("s{i}"), i as u64)).collect()
        };
        let mut library = Library::default();
        library.merge(entries(5000));
        let started = Instant::now();
        library.merge(entries(5000));
        assert_eq!(library.entries().len(), 5000);
        assert!(started.elapsed() < Duration::from_millis(250), "{:?}", started.elapsed());
    }

    #[test]
    fn cwd_falls_back_to_decoded_directory_name() {
        let path = Path::new("/p/-Users-me-app/abc.jsonl");
        assert_eq!(cwd_from_project_dir(path), "/Users/me/app");
    }
}

#[cfg(test)]
mod lineage_tests;
