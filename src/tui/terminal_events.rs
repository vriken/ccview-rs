use super::{App, Focus, list, outer_terminal};
use crate::terminal::TerminalEvent;
use std::time::{Duration, Instant};

const ALERT_DURATION: Duration = Duration::from_secs(5);
const CTRL_C_GRACE: Duration = Duration::from_secs(1);
const ALERT_MAX_CHARS: usize = 120;

fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

impl App {
    pub fn take_outer_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.outer_output)
    }

    pub(super) fn show_alert(&mut self, text: &str) {
        let text = list::truncate(&printable(text), ALERT_MAX_CHARS);
        self.alert = Some((text, Instant::now()));
    }

    pub(super) fn expire_alert(&mut self) -> bool {
        let expired = self
            .alert
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() >= ALERT_DURATION);
        if expired {
            self.alert = None;
        }
        expired
    }

    pub(super) fn drain_terminal_events(&mut self, id: u64) -> bool {
        let Some(session) = self.sessions.get(id) else {
            return false;
        };
        let events = session.host().take_events();
        let session_title = session.title.clone();
        let mut changed = false;
        for event in events {
            match event {
                TerminalEvent::Title(title) => {
                    self.sessions.set_terminal_title(id, printable(&title));
                    changed = true;
                }
                TerminalEvent::Clipboard { selection, data } => {
                    if let Some(sequence) = outer_terminal::clipboard_sequence(&selection, &data) {
                        self.outer_output.extend(sequence);
                    }
                }
                TerminalEvent::Bell => {
                    self.outer_output.extend(outer_terminal::bell());
                    self.show_alert(&format!("bell · {session_title}"));
                    changed = true;
                }
                TerminalEvent::Notification { title, body } => {
                    self.outer_output.extend(outer_terminal::bell());
                    let text = match title.filter(|title| !title.is_empty()) {
                        Some(title) => format!("{title}: {body}"),
                        None => format!("{session_title}: {body}"),
                    };
                    self.show_alert(&text);
                    changed = true;
                }
            }
        }
        changed
    }

    pub(super) fn sync_child_focus(&mut self) {
        let wanted = (self.terminal_focused && self.focus == Focus::Embedded)
            .then_some(self.shown)
            .flatten()
            .filter(|&id| self.sessions.get(id).is_some_and(|session| session.is_running()));
        if wanted == self.focused_child {
            return;
        }
        if let Some(session) = self.focused_child.and_then(|id| self.sessions.get(id)) {
            let _ = session.host().send_focus(false);
        }
        if let Some(session) = wanted.and_then(|id| self.sessions.get(id)) {
            let _ = session.host().send_focus(true);
        }
        self.focused_child = wanted;
    }

    pub(super) fn reap_clean_exits(&mut self) -> bool {
        let finished: Vec<u64> = self
            .sessions
            .all()
            .iter()
            .filter(|session| session.exit_status().is_some_and(|exit| exit.success()))
            .map(|session| session.id)
            .collect();
        for &id in &finished {
            self.finish_cleanly_exited(id);
        }
        !finished.is_empty()
    }

    fn finish_cleanly_exited(&mut self, id: u64) {
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let title = session.title.clone();
        let session_id = session.session_id.clone();
        let was_shown = self.shown == Some(id);
        let was_focused = was_shown && self.focus == Focus::Embedded;
        self.sessions.remove(id);
        if self.focused_child == Some(id) {
            self.focused_child = None;
        }
        if was_shown {
            self.shown = None;
        }
        if let Some(path) = session_id.and_then(|session_id| {
            self.library
                .entries()
                .iter()
                .find(|entry| entry.session_id == session_id)
                .map(|entry| entry.path.clone())
        }) {
            self.select_path(Some(path));
        }
        if was_shown {
            self.shown = self
                .selected_entry()
                .and_then(|entry| self.sessions.for_session_id(&entry.session_id));
        }
        if was_focused {
            self.focus = Focus::List;
            self.ignore_ctrl_c_until = Some(Instant::now() + CTRL_C_GRACE);
        }
        self.prefix.reset();
        self.show_alert(&format!("session ended · {title}"));
    }
}
