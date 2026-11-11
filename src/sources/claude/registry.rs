use crate::session::live::{LiveSession, Status};
use crate::sys::file_stamp::{FileStamp, stamp_of};
use crate::sys::process::pid_alive;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn claude_sessions_dir() -> Option<PathBuf> {
    crate::sources::claude::paths::claude_config_dir().map(|dir| dir.join("sessions"))
}

pub(crate) fn session_id_for_pid(sessions_dir: &Path, pid: u32) -> Option<String> {
    read_session(&sessions_dir.join(format!("{pid}.json")))?.session_id
}

pub(crate) fn proc_start_for_pid(sessions_dir: &Path, pid: u32) -> Option<String> {
    let bytes = fs::read(sessions_dir.join(format!("{pid}.json"))).ok()?;
    let value = serde_json::from_slice::<Value>(&bytes).ok()?;
    let start = value.get("procStart")?.as_str()?;
    Some(start.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub(crate) fn live_sessions() -> Vec<LiveSession> {
    let (Some(sessions_dir), Some(projects_dir)) = (
        claude_sessions_dir(),
        crate::sources::claude::paths::projects_dir(),
    ) else {
        return Vec::new();
    };
    live_sessions_in(&sessions_dir, &projects_dir)
}

pub(crate) fn live_sessions_in(sessions_dir: &Path, projects_dir: &Path) -> Vec<LiveSession> {
    let mut sessions = all_sessions_in(sessions_dir, projects_dir);
    sessions.retain(|session| session.pid_alive);
    sessions
}

pub(crate) fn all_sessions_in(sessions_dir: &Path, projects_dir: &Path) -> Vec<LiveSession> {
    let mut sessions: Vec<LiveSession> = session_files(sessions_dir)
        .iter()
        .filter_map(|path| read_session(path))
        .collect();
    let transcripts = transcript_paths(projects_dir, &sessions);
    for session in &mut sessions {
        session.transcript = session
            .session_id
            .as_ref()
            .and_then(|id| transcripts.get(id).cloned());
    }
    sort_newest_first(&mut sessions);
    sessions
}

fn transcript_paths(projects_dir: &Path, sessions: &[LiveSession]) -> HashMap<String, PathBuf> {
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return HashMap::new();
    };
    let project_dirs: Vec<PathBuf> = projects.flatten().map(|project| project.path()).collect();
    sessions
        .iter()
        .filter_map(|session| session.session_id.as_deref())
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

fn session_files(sessions_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect()
}

fn read_session(path: &Path) -> Option<LiveSession> {
    let bytes = fs::read(path).ok()?;
    from_value(&serde_json::from_slice::<Value>(&bytes).ok()?)
}

fn with_transcript(mut session: LiveSession, projects_dir: &Path) -> LiveSession {
    session.transcript = session
        .session_id
        .as_deref()
        .and_then(|id| find_transcript(projects_dir, id));
    session
}

fn sort_newest_first(sessions: &mut [LiveSession]) {
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at_ms));
}

pub(crate) struct RegistryPoll {
    pub(crate) live: Vec<LiveSession>,
    pub(crate) changed: bool,
}

pub(crate) struct RegistryWatcher {
    sessions_dir: PathBuf,
    projects_dir: PathBuf,
    parsed: HashMap<PathBuf, (FileStamp, Option<LiveSession>)>,
    previous_live: Option<Vec<LiveSession>>,
}

impl RegistryWatcher {
    pub(crate) fn new(sessions_dir: PathBuf, projects_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            projects_dir,
            parsed: HashMap::new(),
            previous_live: None,
        }
    }

    pub(crate) fn from_env() -> Option<Self> {
        Some(Self::new(
            claude_sessions_dir()?,
            crate::sources::claude::paths::projects_dir()?,
        ))
    }

    pub(crate) fn poll(&mut self) -> RegistryPoll {
        let files = session_files(&self.sessions_dir);
        let mut changed = files.len() != self.parsed.len();
        let mut current = HashMap::with_capacity(files.len());
        for path in files {
            let Some(stamp) = stamp_of(&path) else {
                changed = true;
                continue;
            };
            let entry = match self.parsed.remove(&path) {
                Some((seen, session)) if seen == stamp => (stamp, session),
                _ => {
                    changed = true;
                    (stamp, read_session(&path))
                }
            };
            current.insert(path, entry);
        }
        changed |= !self.parsed.is_empty();
        self.parsed = current;
        let mut live: Vec<LiveSession> = Vec::new();
        for (_, session) in self.parsed.values_mut() {
            let Some(session) = session else { continue };
            session.pid_alive = pid_alive(session.pid);
            if session.pid_alive && session.transcript.is_none() {
                *session = with_transcript(session.clone(), &self.projects_dir);
            }
            if session.pid_alive {
                live.push(session.clone());
            }
        }
        sort_newest_first(&mut live);
        changed |= self.previous_live.as_ref() != Some(&live);
        self.previous_live = Some(live.clone());
        RegistryPoll { live, changed }
    }
}

pub(crate) fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    fs::read_dir(projects_dir)
        .ok()?
        .flatten()
        .map(|project| project.path().join(&file_name))
        .find(|candidate| candidate.is_file())
}

fn from_value(value: &Value) -> Option<LiveSession> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let millis = |key: &str| value.get(key).and_then(Value::as_u64);
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    Some(LiveSession {
        pid,
        pid_alive: pid_alive(pid),
        session_id: text("sessionId"),
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
        transcript: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        name: &str,
        registry_files: usize,
        project_dirs: usize,
        transcripts_per_dir: usize,
    ) -> (PathBuf, PathBuf) {
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
            let json = format!(
                r#"{{"pid":{},"sessionId":"{id}","status":"idle"}}"#,
                std::process::id()
            );
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
        assert!(found.iter().all(|session| {
            session
                .transcript
                .as_ref()
                .is_some_and(|path| path.is_file())
        }));
        fs::remove_dir_all(sessions.parent().unwrap()).unwrap();
    }

    use std::time::Duration;

    fn write(dir: &Path, name: &str, pid: u32, status: &str) {
        let json = format!(
            r#"{{"pid":{pid},"sessionId":"{name}","status":"{status}","kind":"interactive"}}"#
        );
        fs::write(dir.join(format!("{name}.json")), json).unwrap();
    }

    fn exited_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn the_watcher_reports_only_real_changes_and_drops_sessions_whose_process_exited() {
        let root = std::env::temp_dir().join(format!("ccview-registry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (sessions, projects) = (root.join("sessions"), root.join("projects"));
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&projects).unwrap();
        let me = std::process::id();
        write(&sessions, "alive", me, "idle");
        write(&sessions, "gone", exited_pid(), "busy");
        let mut watcher = RegistryWatcher::new(sessions.clone(), projects);

        let first = watcher.poll();
        assert!(first.changed);
        assert_eq!(first.live.len(), 1);
        assert_eq!(first.live[0].status, Status::Idle);
        assert!(!watcher.poll().changed);

        std::thread::sleep(Duration::from_millis(20));
        write(&sessions, "alive", me, "waiting");
        let flipped = watcher.poll();
        assert!(flipped.changed);
        assert_eq!(flipped.live[0].status, Status::Other("waiting".into()));
        assert!(!watcher.poll().changed);

        fs::remove_file(sessions.join("alive.json")).unwrap();
        let removed = watcher.poll();
        assert!(removed.changed && removed.live.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
