use crate::{
    sources::claude::paths,
    sys::{
        file_stamp::{FileStamp, stamp_of},
        process::pid_alive,
    },
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct CliSession {
    #[serde(rename = "sessionId")]
    pub(crate) session_id: String,
    pub(crate) id: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Deserialize)]
struct DaemonLock {
    pid: u32,
}

pub(crate) fn fetch() -> Option<Vec<CliSession>> {
    if !super::binary_is_overridden() && !daemon_running() {
        return None;
    }
    let mut child = Command::new(super::binary())
        .args(["agents", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        let _ = sender.send(output);
    });
    let output = receiver.recv_timeout(FETCH_TIMEOUT);
    if output.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    parse(&output.ok()?)
}

pub(crate) fn parse(json: &str) -> Option<Vec<CliSession>> {
    serde_json::from_str(json).ok()
}

#[derive(Deserialize, Default, Clone, PartialEq, Eq)]
struct JobState {
    state: Option<String>,
    tempo: Option<String>,
    needs: Option<String>,
}

#[derive(Default)]
pub(crate) struct JobWatch {
    seen: HashMap<PathBuf, (FileStamp, JobState)>,
}

impl JobWatch {
    pub(crate) fn changed(&mut self) -> bool {
        let mut current = HashMap::new();
        for path in job_state_files() {
            let Some(stamp) = stamp_of(&path) else {
                continue;
            };
            let state = match self.seen.get(&path) {
                Some((seen, state)) if *seen == stamp => state.clone(),
                _ => read_job_state(&path),
            };
            current.insert(path, (stamp, state));
        }
        let changed = current.len() != self.seen.len()
            || current
                .iter()
                .any(|(path, (_, state))| self.seen.get(path).is_none_or(|(_, old)| old != state));
        self.seen = current;
        changed
    }
}

fn read_job_state(path: &Path) -> JobState {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn job_state_files() -> Vec<PathBuf> {
    let Some(jobs) = paths::claude_config_dir().map(|dir| dir.join("jobs")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(jobs) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path().join("state.json"))
        .collect()
}

fn daemon_running() -> bool {
    let Some(config_dir) = paths::claude_config_dir() else {
        return false;
    };
    std::fs::read(config_dir.join("daemon.lock"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<DaemonLock>(&bytes).ok())
        .is_some_and(|lock| pid_alive(lock.pid))
}
