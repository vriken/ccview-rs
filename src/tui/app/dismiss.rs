use crate::store::dismissed::Dismissed;
use crate::tui::app::{App, Focus, nav::ListFocus};

const NOT_ACTIVE: &str = "not active";

impl App {
    pub fn with_dismissed(mut self, dismissed: Dismissed) -> Self {
        self.catalog.set_dismissed(dismissed);
        self.rebuild_view();
        self
    }

    pub(in crate::tui) fn dismiss_selected(&mut self) {
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
        if self.catalog.dismissed().contains(&session_id) {
            return;
        }
        if pid.is_none() && !self.catalog.activity().contains_key(&session_id) {
            self.notice = Some(NOT_ACTIVE.to_owned());
            return;
        }
        let saved = self.catalog.dismiss(&session_id, pid);
        self.finish_dismissal_change(saved);
    }

    pub(in crate::tui) fn undismiss_selected(&mut self) {
        if !self.dismissal_keys_apply() {
            return;
        }
        let Some(session_id) = self.selected_entry().map(|entry| entry.session_id.clone()) else {
            return;
        };
        let saved = self.catalog.undismiss(&session_id);
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
