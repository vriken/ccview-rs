use crate::terminal::HostClient;
use crate::tui::App;

impl App {
    pub(crate) fn with_session_host(mut self, client: HostClient) -> Self {
        self.sessions = self.sessions.with_host(client);
        self.sessions.attach_existing();
        self.host_unreachable = self.sessions.host_unreachable();
        self
    }

    pub(in crate::tui) fn show_strip_session(&mut self, digit: char) {
        let Some(index) = digit.to_digit(10).and_then(|digit| digit.checked_sub(1)) else {
            return;
        };
        let Some(session) = self.sessions.all().get(index as usize) else {
            return;
        };
        let (id, session_id) = (session.id, session.session_id.clone());
        let linked_path = session_id.and_then(|session_id| {
            self.rows
                .iter()
                .map(|&row| &self.catalog.entries()[row])
                .find(|entry| entry.session_id == session_id)
                .map(|entry| entry.path.clone())
        });
        if linked_path.is_some() {
            self.select_path(linked_path);
        }
        self.show_embedded(id);
    }
}
