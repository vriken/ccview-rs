use serde_json::Value;
use std::{
    collections::HashMap,
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
    crate::discover::claude_config_dir().map(|dir| dir.join("sessions"))
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
    let values: Vec<Value> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .collect();
    let transcripts = transcript_paths(projects_dir, &values);
    let mut sessions: Vec<LiveSession> = values
        .iter()
        .filter_map(|value| from_value(value, &transcripts))
        .collect();
    sessions.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    sessions
}

fn transcript_paths(projects_dir: &Path, values: &[Value]) -> HashMap<String, PathBuf> {
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return HashMap::new();
    };
    let project_dirs: Vec<PathBuf> = projects.flatten().map(|project| project.path()).collect();
    values
        .iter()
        .filter_map(|value| value.get("sessionId")?.as_str())
        .filter_map(|id| {
            let file_name = format!("{id}.jsonl");
            let found = project_dirs
                .iter()
                .map(|dir| dir.join(&file_name))
                .find(|candidate| candidate.is_file())?;
            Some((id.to_owned(), found))
        })
        .collect()
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

fn from_value(value: &Value, transcripts: &HashMap<String, PathBuf>) -> Option<LiveSession> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let millis = |key: &str| value.get(key).and_then(Value::as_u64);
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let session_id = text("sessionId");
    let transcript = session_id.as_deref().and_then(|id| transcripts.get(id).cloned());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, registry_files: usize, project_dirs: usize, transcripts_per_dir: usize) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("ccview-reg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (sessions, projects) = (root.join("sessions"), root.join("projects"));
        fs::create_dir_all(&sessions).unwrap();
        for dir in 0..project_dirs {
            let project = projects.join(format!("-work-p{dir}"));
            fs::create_dir_all(&project).unwrap();
            for n in 0..transcripts_per_dir {
                fs::write(project.join(format!("s{dir}-{n}.jsonl")), "").unwrap();
            }
        }
        for n in 0..registry_files {
            let (dir, id) = (n % project_dirs, format!("s{}-{n}", n % project_dirs));
            let json = format!(r#"{{"pid":{},"sessionId":"{id}","status":"idle"}}"#, std::process::id());
            fs::write(sessions.join(format!("{n}.json")), json).unwrap();
            assert!(projects.join(format!("-work-p{dir}/{id}.jsonl")).is_file());
        }
        (sessions, projects)
    }

    #[test]
    fn every_registry_session_gets_its_transcript_path() {
        let (sessions, projects) = fixture("paths", 6, 3, 8);
        let found = all_sessions_in(&sessions, &projects);
        assert_eq!(found.len(), 6);
        assert!(found.iter().all(|session| session.transcript.as_ref().is_some_and(|path| path.is_file())));
        fs::remove_dir_all(sessions.parent().unwrap()).unwrap();
    }
}
