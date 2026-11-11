use crate::session::entry::TitleSource;
use crate::terminal::{CommandSpec, Size};
use crate::tui::app::*;
use crate::tui::embedded::manager::Launched;
use crate::{
    catalog::feed::Update,
    session::{entry::SessionEntry, harness_id::HarnessId},
    store::favorites::Favorites,
    tui::style::identity::Identity,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use std::time::UNIX_EPOCH;
use std::{path::PathBuf, sync::mpsc, time::Instant};

pub(crate) fn entry_in(id: &str, cwd: &str, harness: HarnessId) -> SessionEntry {
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

pub(crate) fn app_with(name: &str, entries: Vec<SessionEntry>) -> App {
    let dir = std::env::temp_dir().join(format!("ccview-support-{}-{name}", std::process::id()));
    let mut app = App::new(
        Favorites::load(dir.join("f.json")),
        Identity::load(dir.join("c.json")),
        None,
        mpsc::channel().0,
    );
    app.claude_sessions_dir = Some(dir.join("claude-sessions"));
    app.apply(Update::Scanned(entries));
    app
}

pub(crate) fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

pub(crate) fn screen_lines(app: &mut App, width: u16, height: u16) -> Vec<String> {
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

pub(crate) fn entry(id: &str, cwd: &str) -> SessionEntry {
    SessionEntry {
        path: PathBuf::from(format!("/p/{id}.jsonl")),
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        title: id.to_owned(),
        title_source: TitleSource::Prompt,
        last_prompt: None,
        last_used: UNIX_EPOCH,
        live: None,
        ..SessionEntry::stub()
    }
}

pub(crate) fn app() -> App {
    let mut app = App::new(
        Favorites::load(
            std::env::temp_dir().join(format!("ccview-keys-{}/f.json", std::process::id())),
        ),
        Identity::load(
            std::env::temp_dir().join(format!("ccview-keys-{}/c.json", std::process::id())),
        ),
        None,
        mpsc::channel().0,
    );
    app.claude_sessions_dir = Some(std::env::temp_dir().join(format!(
        "ccview-keys-{}/claude-sessions",
        std::process::id()
    )));
    app.apply(Update::Scanned(vec![
        entry("a", "/w/one"),
        entry("b", "/w/two"),
    ]));
    app
}

pub(crate) fn type_char(app: &mut App, c: char) {
    press(app, KeyCode::Char(c));
}

pub(crate) fn wait_for_screen(app: &App, id: u64, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app
        .sessions
        .get(id)
        .unwrap()
        .host()
        .snapshot()
        .text()
        .contains(needle)
    {
        assert!(Instant::now() < deadline, "never saw {needle:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn script_child(dir: &std::path::Path, name: &str, body: &str) -> CommandSpec {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    CommandSpec::new("sh").args([path.to_str().unwrap()])
}

pub(crate) fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ccview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn launch_child(app: &mut App, spec: &CommandSpec) -> u64 {
    let launched = Launched {
        title: "child".to_owned(),
        session_id: None,
        harness: HarnessId::Claude,
    };
    let id = app
        .sessions
        .launch(spec, launched, Size { rows: 10, cols: 40 })
        .unwrap();
    app.show_embedded(id);
    id
}

pub(crate) fn read_file(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

pub(crate) fn wait_for_bytes(path: &std::path::Path, needle: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !read_file(path).windows(needle.len()).any(|w| w == needle) {
        assert!(
            Instant::now() < deadline,
            "never saw {needle:?} in {:?}",
            read_file(path)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn recording_child(dir: &std::path::Path, prelude: &str) -> (CommandSpec, PathBuf) {
    let out = dir.join("stdin.bin");
    let body = format!("stty raw echo; {prelude}; exec cat > {}", out.display());
    (script_child(dir, "child.sh", &body), out)
}

pub(crate) fn drawn(
    app: &mut App,
    width: u16,
    height: u16,
) -> ratatui::Terminal<ratatui::backend::TestBackend> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal
}
