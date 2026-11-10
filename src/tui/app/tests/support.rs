use crate::session::entry::TitleSource;
use crate::tui::*;
use std::time::UNIX_EPOCH;

pub fn entry_in(id: &str, cwd: &str, harness: HarnessId) -> SessionEntry {
    SessionEntry {
        path: PathBuf::from(format!("/p/{id}.jsonl")),
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        title: id.to_owned(),
        title_source: TitleSource::Prompt,
        last_prompt: None,
        last_used: UNIX_EPOCH,
        live: None,
        harness,
        ..SessionEntry::stub()
    }
}

pub fn app_with(name: &str, entries: Vec<SessionEntry>) -> App {
    let dir = std::env::temp_dir().join(format!("ccview-support-{}-{name}", std::process::id()));
    let mut app = App::new(
        Favorites::load(dir.join("f.json")),
        Identity::load(dir.join("c.json")),
        None,
        mpsc::channel().0,
    );
    app.apply(Update::Scanned(entries));
    app
}

pub fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

pub fn screen_lines(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}
