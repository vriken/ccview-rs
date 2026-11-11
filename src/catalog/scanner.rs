use super::index::IndexCache;
use crate::{
    session::{entry::SessionEntry, lineage::Fingerprint},
    sources::{
        AgeFilter, SessionFile, SourceEntry,
        claude::scan::{FileMeta, FileRecord, entry_from, extend_record, tail_window},
        codex::{self, RolloutHead},
    },
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) const CLAUDE_SOURCE: &str = "claude";
pub(super) const CODEX_SOURCE: &str = "codex";

#[derive(Default)]
pub(crate) struct Scanner {
    index: IndexCache,
    pub(super) codex_thread_names: HashMap<String, String>,
    pub(crate) bytes_read: u64,
    pub(crate) files_opened: u64,
    age: AgeFilter,
}

impl Scanner {
    pub(crate) fn with_index(index: IndexCache) -> Self {
        Self {
            index,
            codex_thread_names: HashMap::new(),
            bytes_read: 0,
            files_opened: 0,
            age: AgeFilter::default(),
        }
    }

    pub(crate) fn age(&self) -> &AgeFilter {
        &self.age
    }

    pub(crate) fn set_age(&mut self, age: AgeFilter) {
        self.age = age;
    }

    pub(crate) fn scan(&mut self, file: &SessionFile) -> SessionEntry {
        let record = self.record_for(file);
        entry_from(file, record.meta).with_fingerprint(record.fingerprint)
    }

    pub(crate) fn scan_codex(
        &mut self,
        file: &SessionFile,
        thread_names: &HashMap<String, String>,
    ) -> Option<SessionEntry> {
        let cached = self.index.get(CODEX_SOURCE, &file.path).cloned();
        let record = match cached.filter(|record| matches_stat(record, file)) {
            Some(record) => record,
            None => {
                self.files_opened += 1;
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

    pub(crate) fn cached_codex_entries(
        &self,
        thread_names: &HashMap<String, String>,
    ) -> Vec<SessionEntry> {
        self.index
            .records(CODEX_SOURCE)
            .filter_map(|(path, record)| {
                let file = SessionFile {
                    path,
                    modified: UNIX_EPOCH + Duration::from_nanos(record.mtime_ns),
                    len: record.size,
                };
                let id = record.meta.session_id.as_deref()?;
                if !self.age.admits_id(file.modified, id) {
                    return None;
                }
                codex_entry(&file, &record.meta, thread_names)
            })
            .collect()
    }

    pub(crate) fn is_fresh(&self, source: &str, file: &SessionFile) -> bool {
        self.index
            .get(source, &file.path)
            .is_some_and(|record| matches_stat(record, file))
    }

    pub(crate) fn cached_entries(&self) -> Vec<SessionEntry> {
        self.index
            .records(CLAUDE_SOURCE)
            .filter_map(|(path, record)| {
                let file = SessionFile {
                    path,
                    modified: UNIX_EPOCH + Duration::from_nanos(record.mtime_ns),
                    len: record.size,
                };
                let id = file.path.file_stem()?.to_str()?;
                if !self.age.admits_id(file.modified, id) {
                    return None;
                }
                Some(
                    entry_from(&file, record.meta.clone())
                        .with_fingerprint(record.fingerprint.clone()),
                )
            })
            .collect()
    }

    pub(crate) fn forget_missing(&mut self, source: &str, files: &[SessionFile]) -> Vec<PathBuf> {
        let present: HashSet<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
        self.index.remove_missing(source, &present)
    }

    pub(crate) fn persist(&mut self) {
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
        self.files_opened += 1;
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
            asks_user: false,
            live: None,
            fingerprint: None,
            copy_of: None,
            copies: Vec::new(),
        }
    }
}

fn codex_meta(head: RolloutHead) -> FileMeta {
    FileMeta {
        session_id: Some(head.session_id),
        cwd: Some(head.cwd),
        first_prompt: head.first_prompt,
        last_user_message_ms: head.last_user_message_ms,
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
        last_user_message_ms: meta.last_user_message_ms,
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
