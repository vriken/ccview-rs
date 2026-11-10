use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
    Other(String),
    Unknown,
}

impl Status {
    pub fn label(&self) -> &str {
        match self {
            Status::Busy => "busy",
            Status::Idle => "idle",
            Status::Other(label) => label,
            Status::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub pid: u32,
    pub pid_alive: bool,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub started_at_ms: Option<u64>,
    pub updated_at_ms: Option<u64>,
    pub status_updated_at_ms: Option<u64>,
    pub status: Status,
    pub name: Option<String>,
    pub name_source: Option<String>,
    pub version: Option<String>,
    pub kind: Option<String>,
    pub entrypoint: Option<String>,
    pub transcript: Option<PathBuf>,
}
