use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

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

#[derive(Debug, Clone)]
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

pub fn claude_sessions_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| Path::new(&home).join(".claude/sessions"))
}

pub fn live_sessions() -> Vec<LiveSession> {
    let (Some(sessions_dir), Some(projects_dir)) =
        (claude_sessions_dir(), crate::discover::claude_projects_dir())
    else {
        return Vec::new();
    };
    live_sessions_in(&sessions_dir, &projects_dir)
}

pub fn live_sessions_in(sessions_dir: &Path, projects_dir: &Path) -> Vec<LiveSession> {
    let mut sessions = all_sessions_in(sessions_dir, projects_dir);
    sessions.retain(|session| session.pid_alive);
    sessions
}

pub fn all_sessions_in(sessions_dir: &Path, projects_dir: &Path) -> Vec<LiveSession> {
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<LiveSession> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter_map(|value| from_value(&value, projects_dir))
        .collect();
    sessions.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    sessions
}

pub fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    fs::read_dir(projects_dir)
        .ok()?
        .flatten()
        .map(|project| project.path().join(&file_name))
        .find(|candidate| candidate.is_file())
}

pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn from_value(value: &Value, projects_dir: &Path) -> Option<LiveSession> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let millis = |key: &str| value.get(key).and_then(Value::as_u64);
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let session_id = text("sessionId");
    let transcript = session_id
        .as_deref()
        .and_then(|id| find_transcript(projects_dir, id));
    Some(LiveSession {
        pid,
        pid_alive: pid_alive(pid),
        session_id,
        cwd: text("cwd"),
        started_at_ms: millis("startedAt"),
        updated_at_ms: millis("updatedAt"),
        status_updated_at_ms: millis("statusUpdatedAt"),
        status: match text("status").as_deref() {
            Some("busy") => Status::Busy,
            Some("idle") => Status::Idle,
            Some(other) => Status::Other(other.to_owned()),
            None => Status::Unknown,
        },
        name: text("name"),
        name_source: text("nameSource"),
        version: text("version"),
        kind: text("kind"),
        entrypoint: text("entrypoint"),
        transcript,
    })
}
