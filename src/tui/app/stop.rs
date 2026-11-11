use crate::session::entry::SessionEntry;
use crate::sources::claude::registry::session_id_for_pid;
use crate::sys::process::{pid_alive, terminate};
use crate::tui::app::App;

const TITLE_CHARS: usize = 30;

pub(in crate::tui) enum StopTarget {
    Hosted(u64),
    Registry(u32),
    Nothing,
}

pub(in crate::tui) struct PendingStop {
    pub session_id: String,
    pub pid: Option<u32>,
    pub title: String,
    pub target: StopTarget,
}

pub(super) fn short_title(entry: &SessionEntry) -> String {
    let title = if entry.title.is_empty() {
        &entry.session_id
    } else {
        &entry.title
    };
    title.chars().take(TITLE_CHARS).collect()
}

impl App {
    pub(super) fn stop_target(&self, session_id: &str, pid: Option<u32>) -> StopTarget {
        if let Some(id) = self.sessions.for_session_id(session_id).filter(|&id| {
            self.sessions
                .get(id)
                .is_some_and(|session| session.is_running())
        }) {
            return StopTarget::Hosted(id);
        }
        match pid {
            Some(pid) => StopTarget::Registry(pid),
            None => StopTarget::Nothing,
        }
    }

    pub(in crate::tui) fn stop_and_dismiss(&mut self, pending: PendingStop) {
        let PendingStop {
            session_id,
            pid,
            title,
            target,
        } = pending;
        match target {
            StopTarget::Hosted(id) => {
                if let Err(error) = self.sessions.stop(id) {
                    let stopped_by_pid =
                        pid.is_some_and(|pid| self.stop_registered_process(pid, &session_id));
                    if !stopped_by_pid {
                        self.notice = Some(format!("could not stop {title}: {error}"));
                        return;
                    }
                }
                self.notice = Some(format!("stopping {title}"));
            }
            StopTarget::Registry(pid) => {
                self.notice = Some(if self.stop_registered_process(pid, &session_id) {
                    format!("stopping {title}")
                } else {
                    "process is no longer this session, nothing stopped".to_owned()
                });
            }
            StopTarget::Nothing => {}
        }
        self.record_dismissal(&session_id, pid);
    }

    fn stop_registered_process(&self, pid: u32, session_id: &str) -> bool {
        let Some(sessions_dir) = &self.claude_sessions_dir else {
            return false;
        };
        pid_alive(pid)
            && session_id_for_pid(sessions_dir, pid).as_deref() == Some(session_id)
            && terminate(pid).is_ok()
    }
}
