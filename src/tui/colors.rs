use super::{App, identity::Key};

impl App {
    pub(super) fn sync_identity(&mut self) {
        let mut keys: Vec<Key> = self
            .sessions
            .all()
            .iter()
            .filter(|session| session.is_running())
            .map(|session| session.identity_key())
            .collect();
        keys.extend(
            self.library
                .entries()
                .iter()
                .filter(|entry| entry.live.as_ref().is_some_and(|live| live.pid_alive))
                .map(|entry| Key::Session(entry.session_id.clone())),
        );
        self.identity.track(keys);
    }

    pub(super) fn cycle_selected_color(&mut self) {
        let Some(session_id) = self.selected_entry().map(|entry| entry.session_id.clone()) else {
            self.notice = Some("select a session first".to_owned());
            return;
        };
        self.cycle_color(Key::Session(session_id));
    }

    pub(super) fn cycle_shown_color(&mut self, id: u64) {
        if let Some(key) = self.sessions.get(id).map(|session| session.identity_key()) {
            self.cycle_color(key);
        }
    }

    fn cycle_color(&mut self, key: Key) {
        self.sync_identity();
        if let Err(err) = self.identity.cycle(&key) {
            self.notice = Some(format!("could not save colours: {err}"));
        }
    }

    pub(super) fn shown_color(&self) -> Option<ratatui::style::Color> {
        let session = self.shown.and_then(|id| self.sessions.get(id))?;
        Some(self.identity.color(&session.identity_key()))
    }

    pub(super) fn selected_color(&self) -> Option<ratatui::style::Color> {
        self.selected_entry()
            .map(|entry| self.identity.session_color(&entry.session_id))
    }
}
