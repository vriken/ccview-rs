use crate::{
    harness::browser,
    tui::{
        app::App,
        input::copy::Cell,
        reader::{selection::View, verbosity::Level},
    },
};

impl App {
    pub(in crate::tui) fn open_link_at(&mut self, width: usize, level: Level, cell: Cell) {
        let View::Content(conversation) = self.transcript.view() else {
            return;
        };
        let Some(url) = conversation.link_at(width, level, cell.row, cell.col) else {
            return;
        };
        let message = match browser::open_with(&url, &mut *self.link_opener) {
            Ok(notice) | Err(notice) => notice,
        };
        self.show_alert(&message);
    }
}
