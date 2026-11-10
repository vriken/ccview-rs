use super::{identity::Key, outer_terminal, runtime::Message};
use crate::{
    registry::{LiveSession, Status},
    sources::HarnessId,
    terminal::{
        CommandSpec, ExitStatus, HostClient, PtyHost, SessionInfo, Size, SpawnRequest,
        TerminalSession,
    },
};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

pub struct Embedded {
    pub id: u64,
    pub title: String,
    pub terminal_title: Option<String>,
    pub session_id: Option<String>,
    pub harness: HarnessId,
    pub status: Option<Status>,
    host: Box<dyn TerminalSession>,
    daemon_id: Option<u64>,
    wake_pending: Arc<AtomicBool>,
    exit_seen: bool,
}

impl Embedded {
    pub fn display_title(&self) -> &str {
        self.terminal_title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or(&self.title)
    }

    pub fn host(&self) -> &dyn TerminalSession {
        self.host.as_ref()
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.host.exit_status()
    }

    pub fn is_running(&self) -> bool {
        self.host.exit_status().is_none()
    }

    pub fn identity_key(&self) -> Key {
        match &self.session_id {
            Some(session_id) => Key::Session(session_id.clone()),
            None => Key::Host(self.id),
        }
    }
}

pub struct Launched {
    pub title: String,
    pub session_id: Option<String>,
    pub harness: HarnessId,
}

pub struct SessionManager {
    sessions: Vec<Embedded>,
    next_id: u64,
    wake: Sender<Message>,
    client: Option<HostClient>,
}

impl SessionManager {
    pub fn new(wake: Sender<Message>) -> Self {
        Self {
            sessions: Vec::new(),
            next_id: 1,
            wake,
            client: None,
        }
    }

    pub fn with_host(mut self, client: HostClient) -> Self {
        self.client = Some(client);
        self
    }

    pub fn launch(&mut self, spec: &CommandSpec, launched: Launched, size: Size) -> io::Result<u64> {
        let options = outer_terminal::terminal_options();
        let Some(client) = self.client.clone() else {
            let host = PtyHost::spawn_with_options(spec, size, options)?;
            return Ok(self.add(Box::new(host), None, launched));
        };
        let info = client.spawn(SpawnRequest {
            spec: spec.clone(),
            options,
            size,
            title: launched.title.clone(),
            harness: launched.harness.as_str().to_owned(),
            session_id: launched.session_id.clone(),
        })?;
        let (_, remote) = client.attach(info.id, None)?;
        Ok(self.add(Box::new(remote), Some(info.id), launched))
    }

    pub fn attach_existing(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        for info in client.list().unwrap_or_default() {
            self.attach_info(&info);
        }
    }

    pub fn reattach_running(&mut self, session_id: &str, pid: Option<u32>) -> Option<u64> {
        let client = self.client.as_ref()?;
        let info = client.list().ok()?.into_iter().find(|info| {
            info.exit.is_none()
                && !self.is_attached(info.id)
                && (info.session_id.as_deref() == Some(session_id) || (pid.is_some() && info.pid == pid))
        })?;
        self.attach_info(&info)
    }

    fn is_attached(&self, daemon_id: u64) -> bool {
        self.sessions.iter().any(|session| session.daemon_id == Some(daemon_id))
    }

    fn attach_info(&mut self, info: &SessionInfo) -> Option<u64> {
        if self.is_attached(info.id) {
            return None;
        }
        let (_, remote) = self.client.as_ref()?.attach(info.id, None).ok()?;
        let launched = Launched {
            title: info.title.clone(),
            session_id: info.session_id.clone(),
            harness: HarnessId::parse(&info.harness).unwrap_or(HarnessId::Claude),
        };
        Some(self.add(Box::new(remote), Some(info.id), launched))
    }

    fn add(&mut self, host: Box<dyn TerminalSession>, daemon_id: Option<u64>, launched: Launched) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let wake_pending = Arc::new(AtomicBool::new(false));
        let sender = self.wake.clone();
        let pending = Arc::clone(&wake_pending);
        host.set_waker(Box::new(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                let _ = sender.send(Message::Output(id));
            }
        }));
        let _ = self.wake.send(Message::Output(id));
        self.sessions.push(Embedded {
            id,
            title: launched.title,
            terminal_title: None,
            session_id: launched.session_id,
            harness: launched.harness,
            status: None,
            host,
            daemon_id,
            wake_pending,
            exit_seen: false,
        });
        id
    }

    pub fn set_terminal_title(&mut self, id: u64, title: String) {
        if let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) {
            session.terminal_title = Some(title);
        }
    }

    pub fn all(&self) -> &[Embedded] {
        &self.sessions
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Embedded> {
        self.sessions.iter().find(|session| session.id == id)
    }

    pub fn remove(&mut self, id: u64) {
        let Some(position) = self.sessions.iter().position(|session| session.id == id) else {
            return;
        };
        let session = self.sessions.remove(position);
        if let (Some(client), Some(daemon_id), true) =
            (&self.client, session.daemon_id, session.exit_status().is_some())
        {
            let _ = client.dismiss(daemon_id);
        }
    }

    pub fn for_session_id(&self, session_id: &str) -> Option<u64> {
        self.sessions
            .iter()
            .rev()
            .find(|session| session.session_id.as_deref() == Some(session_id))
            .map(|session| session.id)
    }

    pub fn owns_pid(&self, pid: u32) -> bool {
        self.sessions
            .iter()
            .any(|session| session.is_running() && session.host.process_id() == Some(pid))
    }

    pub fn neighbor(&self, id: u64, step: isize) -> Option<u64> {
        let count = self.sessions.len() as isize;
        let current = self.sessions.iter().position(|session| session.id == id)? as isize;
        let next = (current + step).rem_euclid(count) as usize;
        Some(self.sessions[next].id)
    }

    pub fn bind_live(&mut self, live: &[LiveSession]) -> Vec<(u64, String)> {
        let mut newly_bound = Vec::new();
        for session in &mut self.sessions {
            let pid = session.host.process_id();
            let matched = live.iter().find(|entry| Some(entry.pid) == pid);
            session.status = matched.map(|entry| entry.status.clone());
            if let Some(session_id) = matched.and_then(|entry| entry.session_id.clone()) {
                if session.session_id.is_none() {
                    newly_bound.push((session.id, session_id.clone()));
                }
                session.session_id = Some(session_id);
            }
        }
        newly_bound
    }

    pub fn resize_all(&self, size: Size) {
        for session in &self.sessions {
            if session.host.snapshot().size() != size {
                let _ = session.host.resize(size.rows, size.cols);
            }
        }
    }

    pub fn acknowledge_output(&mut self, id: u64) -> bool {
        let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) else {
            return false;
        };
        session.wake_pending.store(false, Ordering::Release);
        let exited = session.host.exit_status().is_some();
        let newly_exited = exited && !session.exit_seen;
        session.exit_seen = exited;
        newly_exited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    const SIZE: Size = Size { rows: 10, cols: 40 };
    const TIMEOUT: Duration = Duration::from_secs(10);

    fn manager() -> SessionManager {
        SessionManager::new(mpsc::channel().0)
    }

    fn shell(script: &str) -> CommandSpec {
        CommandSpec::new("sh").args(["-c", script])
    }

    fn launched(title: &str) -> Launched {
        Launched {
            title: title.to_owned(),
            session_id: None,
            harness: HarnessId::Claude,
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !condition() {
            assert!(Instant::now() < deadline, "condition not met in time");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn exit_is_reported_once_and_the_screen_is_kept() {
        let mut sessions = manager();
        let id = sessions
            .launch(&shell("printf 'last words'; exit 3"), launched("a"), SIZE)
            .unwrap();
        wait_until(|| sessions.get(id).unwrap().exit_status().is_some());
        assert!(sessions.acknowledge_output(id));
        assert!(!sessions.acknowledge_output(id));
        let session = sessions.get(id).unwrap();
        assert_eq!(session.exit_status().unwrap().code, 3);
        assert!(session.host().snapshot().text().contains("last words"));
    }

    #[test]
    fn neighbor_wraps_in_both_directions() {
        let mut sessions = manager();
        let ids: Vec<u64> = (0..3)
            .map(|n| sessions.launch(&shell("exec sleep 60"), launched(&n.to_string()), SIZE).unwrap())
            .collect();
        assert_eq!(sessions.neighbor(ids[2], 1), Some(ids[0]));
        assert_eq!(sessions.neighbor(ids[0], -1), Some(ids[2]));
    }
}
