use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Busy,
    Idle,
    Other(String),
    Unknown,
}

impl Status {
    pub(crate) fn label(&self) -> &str {
        match self {
            Status::Busy => "busy",
            Status::Idle => "idle",
            Status::Other(label) => label,
            Status::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveSession {
    pub(crate) pid: u32,
    pub(crate) pid_alive: bool,
    pub(crate) session_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) updated_at_ms: Option<u64>,
    pub(crate) status_updated_at_ms: Option<u64>,
    pub(crate) status: Status,
    pub(crate) name: Option<String>,
    pub(crate) name_source: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) entrypoint: Option<String>,
    pub(crate) transcript: Option<PathBuf>,
}
