use super::{App, Focus, ListFocus};
use crate::dismissed::Dismissed;

const RUNNING_IN_PANE: &str = "running in a pane";
const NOT_ACTIVE: &str = "not active";

impl App {
    pub fn with_dismissed(mut self, dismissed: Dismissed) -> Self {
        self.library.set_dismissed(dismissed);
        self.rebuild_view();
        self
    }

    pub(super) fn dismiss_selected(&mut self) {
        if !self.dismissal_keys_apply() {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let session_id = entry.session_id.clone();
        let pid = entry
            .live
            .as_ref()
            .filter(|live| live.pid_alive)
            .map(|live| live.pid);
        let owned_by_pane = self.sessions.for_session_id(&session_id).is_some_and(|id| {
            self.sessions
                .get(id)
                .is_some_and(|session| session.is_running())
        }) || pid.is_some_and(|pid| self.sessions.owns_pid(pid));
        if owned_by_pane {
            self.notice = Some(RUNNING_IN_PANE.to_owned());
            return;
        }
        if self.library.dismissed().contains(&session_id) {
            return;
        }
        if pid.is_none() && !self.library.activity().contains_key(&session_id) {
            self.notice = Some(NOT_ACTIVE.to_owned());
            return;
        }
        let saved = self.library.dismiss(&session_id, pid);
        self.finish_dismissal_change(saved);
    }

    pub(super) fn undismiss_selected(&mut self) {
        if !self.dismissal_keys_apply() {
            return;
        }
        let Some(session_id) = self.selected_entry().map(|entry| entry.session_id.clone()) else {
            return;
        };
        let saved = self.library.undismiss(&session_id);
        self.finish_dismissal_change(saved);
    }

    fn dismissal_keys_apply(&self) -> bool {
        self.focus != Focus::Embedded
            && !(self.focus == Focus::List && self.list_focus == ListFocus::Tabs)
    }

    fn finish_dismissal_change(&mut self, saved: std::io::Result<()>) {
        if let Err(err) = saved {
            self.notice = Some(format!("could not save dismissed sessions: {err}"));
        }
        self.rebuild_view();
    }
}

#[cfg(test)]
mod tests;
