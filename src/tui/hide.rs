use super::App;
use crate::library::SessionEntry;

impl App {
    pub(super) fn is_listed(&self, entry: &SessionEntry) -> bool {
        (self.show_hidden || !self.hidden.contains(&entry.session_id))
            && !self.library.is_collapsed_copy(entry)
    }

    pub(super) fn toggle_hidden_selected(&mut self) {
        let Some(session_id) = self.selected_entry().map(|entry| entry.session_id.clone()) else {
            return;
        };
        if let Err(err) = self.hidden.toggle(&session_id) {
            self.notice = Some(format!("could not save hidden sessions: {err}"));
        }
        self.rebuild_view();
    }

    pub(super) fn toggle_show_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild_view();
    }

    pub(super) fn hidden_note(&self) -> Option<String> {
        if self.hidden.count() == 0 {
            return None;
        }
        let count = self
            .library
            .entries()
            .iter()
            .filter(|entry| self.hidden.contains(&entry.session_id))
            .count();
        match (count, self.show_hidden) {
            (0, _) => None,
            (count, false) => Some(format!("{count} hidden · H show")),
            (count, true) => Some(format!("{count} hidden · H hide")),
        }
    }
}

#[cfg(test)]
mod tests;
