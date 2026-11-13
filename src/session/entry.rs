use super::{harness_id::HarnessId, lineage::Fingerprint, live::LiveSession};
use std::{path::PathBuf, sync::Arc, time::SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleSource {
    Custom,
    Agent,
    Derived,
    Prompt,
    SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DodSummary {
    pub(crate) open: usize,
    pub(crate) handoffs: usize,
    pub(crate) waits: usize,
    pub(crate) wait_label: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionEntry {
    pub(crate) harness: HarnessId,
    pub(crate) handle: TranscriptHandle,
    pub(crate) path: PathBuf,
    pub(crate) session_id: String,
    pub(crate) cwd: String,
    pub(crate) title: String,
    pub(crate) title_source: TitleSource,
    pub(crate) last_prompt: Option<String>,
    pub(crate) last_used: SystemTime,
    pub(crate) asks_user: bool,
    pub(crate) owed: usize,
    pub(crate) dod: Option<DodSummary>,
    pub(crate) live: Option<LiveSession>,
    pub(crate) fingerprint: Option<Arc<Fingerprint>>,
    pub(crate) copy_of: Option<String>,
    pub(crate) copies: Vec<String>,
}

#[cfg(test)]
impl SessionEntry {
    pub(crate) fn stub() -> Self {
        Self {
            harness: HarnessId::Claude,
            handle: TranscriptHandle::Rollout(PathBuf::new()),
            path: PathBuf::new(),
            session_id: String::new(),
            cwd: String::new(),
            title: String::new(),
            title_source: TitleSource::SessionId,
            last_prompt: None,
            last_used: std::time::UNIX_EPOCH,
            asks_user: false,
            owed: 0,
            dod: None,
            live: None,
            fingerprint: None,
            copy_of: None,
            copies: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptHandle {
    Rollout(PathBuf),
    Database { path: PathBuf, session_id: String },
}

impl TranscriptHandle {
    pub(crate) fn entry_key(&self) -> PathBuf {
        match self {
            TranscriptHandle::Rollout(path) => path.clone(),
            TranscriptHandle::Database { path, session_id } => path.join(session_id),
        }
    }
}
