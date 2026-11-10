use super::{harness_id::HarnessId, lineage::Fingerprint, live::LiveSession};
use std::{path::PathBuf, sync::Arc, time::SystemTime};

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
            last_used: std::time::UNIX_EPOCH,
            live: None,
            fingerprint: None,
            copy_of: None,
            copies: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptHandle {
    Rollout(PathBuf),
    Database { path: PathBuf, session_id: String },
}

impl TranscriptHandle {
    pub fn entry_key(&self) -> PathBuf {
        match self {
            TranscriptHandle::Rollout(path) => path.clone(),
            TranscriptHandle::Database { path, session_id } => path.join(session_id),
        }
    }
}
