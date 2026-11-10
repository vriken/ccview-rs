use crate::store::important::ImportantSessions;
use crate::tui::App;

impl App {
    pub fn with_important(mut self, important: ImportantSessions) -> Self {
        self.important = important;
        self.refresh_rows();
        self
    }

    pub(in crate::tui) fn toggle_important(&mut self) {
        if self.tab_bar_has_focus() {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if let Err(err) = self.important.toggle_entry(&entry) {
            self.notice = Some(format!("could not save important sessions: {err}"));
        }
        self.refresh_rows();
    }
}

#[cfg(test)]
mod tests;
