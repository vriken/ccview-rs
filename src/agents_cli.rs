use crate::{harness, registry};
use serde::Deserialize;
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CliSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub id: Option<String>,
    pub kind: Option<String>,
    pub state: Option<String>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
struct DaemonLock {
    pid: u32,
}

pub fn fetch() -> Option<Vec<CliSession>> {
    if !harness::claude_binary_is_overridden() && !daemon_running() {
        return None;
    }
    let mut child = Command::new(harness::claude_binary())
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

pub fn parse(json: &str) -> Option<Vec<CliSession>> {
    serde_json::from_str(json).ok()
}

fn daemon_running() -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let lock = PathBuf::from(home).join(".claude/daemon.lock");
    std::fs::read(lock)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<DaemonLock>(&bytes).ok())
        .is_some_and(|lock| registry::pid_alive(lock.pid))
}
