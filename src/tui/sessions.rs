use super::{identity::Key, runtime::Message};
use crate::{
    registry::{LiveSession, Status},
    terminal::{CommandSpec, ExitStatus, PtyHost, Size},
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
    pub session_id: Option<String>,
    pub status: Option<Status>,
    host: PtyHost,
    wake_pending: Arc<AtomicBool>,
    exit_seen: bool,
}

impl Embedded {
    pub fn host(&self) -> &PtyHost {
        &self.host
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
}

pub struct SessionManager {
    sessions: Vec<Embedded>,
    next_id: u64,
    wake: Sender<Message>,
}

impl SessionManager {
    pub fn new(wake: Sender<Message>) -> Self {
        Self {
            sessions: Vec::new(),
            next_id: 1,
            wake,
        }
    }

    pub fn launch(&mut self, spec: &CommandSpec, launched: Launched, size: Size) -> io::Result<u64> {
        let host = PtyHost::spawn(spec, size)?;
        let id = self.next_id;
        self.next_id += 1;
        let wake_pending = Arc::new(AtomicBool::new(false));
        let sender = self.wake.clone();
        let pending = Arc::clone(&wake_pending);
        host.set_waker(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                let _ = sender.send(Message::Output(id));
            }
        });
        let _ = self.wake.send(Message::Output(id));
        self.sessions.push(Embedded {
            id,
            title: launched.title,
            session_id: launched.session_id,
            status: None,
            host,
            wake_pending,
            exit_seen: false,
        });
        Ok(id)
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

    pub fn running_count(&self) -> usize {
        self.sessions.iter().filter(|session| session.is_running()).count()
    }

    pub fn remove(&mut self, id: u64) {
        self.sessions.retain(|session| session.id != id);
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
    use crate::registry;
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
    fn removing_a_running_session_kills_its_process() {
        let mut sessions = manager();
        let id = sessions.launch(&shell("exec sleep 60"), launched("a"), SIZE).unwrap();
        let pid = sessions.get(id).unwrap().host().process_id().unwrap();
        assert!(sessions.owns_pid(pid));
        assert_eq!(sessions.running_count(), 1);
        sessions.remove(id);
        assert!(sessions.is_empty());
        wait_until(|| !registry::pid_alive(pid));
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
        assert_eq!(sessions.running_count(), 0);
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
