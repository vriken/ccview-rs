use crate::{
    harness::browser,
    tui::{
        app::App,
        input::copy::Cell,
        reader::{links, selection::View, verbosity::Level},
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
        self.open_url(&url);
    }

    pub(in crate::tui) fn open_child_link_at(&mut self, cell: Cell) {
        let Some(session) = self.running_shown() else {
            return;
        };
        let url = {
            let snapshot = session.host().snapshot();
            url_on_screen(snapshot.screen(), cell)
        };
        if let Some(url) = url {
            self.open_url(&url);
        }
    }

    fn open_url(&mut self, url: &str) {
        let message = match browser::open_with(url, &mut *self.link_opener) {
            Ok(notice) | Err(notice) => notice,
        };
        self.show_alert(&message);
    }
}

fn url_on_screen(screen: &vt100::Screen, cell: Cell) -> Option<String> {
    let (rows, columns) = screen.size();
    let (rows, columns) = (rows.get(), columns.get());
    let clicked = u16::try_from(cell.row).ok().filter(|row| *row < rows)?;
    let mut first = clicked;
    while first > 0 && screen.row_wrapped(first - 1) {
        first -= 1;
    }
    let mut last = clicked;
    while last + 1 < rows && screen.row_wrapped(last) {
        last += 1;
    }
    let mut text = String::new();
    let mut clicked_at = None;
    for row in first..=last {
        for col in 0..columns {
            let Some(screen_cell) = screen.cell(row, col) else {
                continue;
            };
            if screen_cell.is_wide_continuation() {
                continue;
            }
            if row == clicked && usize::from(col) <= cell.col {
                clicked_at = Some(text.chars().count());
            }
            match screen_cell.contents() {
                "" => text.push(' '),
                contents => text.push_str(contents),
            }
        }
    }
    links::url_at(&text, clicked_at?).map(str::to_owned)
}
