use super::*;
use crate::{
    library::TitleSource,
    terminal::{CommandSpec, Size},
};
use ratatui::{Terminal, backend::TestBackend, style::Modifier};
use sessions::Launched;
use std::time::UNIX_EPOCH;

fn entry(id: &str, cwd: &str) -> SessionEntry {
    SessionEntry {
        path: PathBuf::from(format!("/p/{id}.jsonl")),
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        title: id.to_owned(),
        title_source: TitleSource::Prompt,
        last_prompt: None,
        last_used: UNIX_EPOCH,
        live: None,
    }
}

fn app(name: &str) -> App {
    let dir = std::env::temp_dir().join(format!("ccview-focus-{}-{name}", std::process::id()));
    let mut app = App::new(
        Favorites::load(dir.join("f.json")),
        Identity::load(dir.join("c.json")),
        None,
        mpsc::channel().0,
    );
    app.apply(Update::Scanned(vec![
        entry("a", "/w/one"),
        entry("b", "/w/two"),
        entry("c", "/w/two"),
    ]));
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn launch_embedded(app: &mut App) -> u64 {
    let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
    let launched = Launched { title: "t".to_owned(), session_id: None };
    let id = app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
    app.show_embedded(id);
    id
}

fn draw(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal
}

fn zone_corners(terminal: &Terminal<TestBackend>) -> Vec<ratatui::buffer::Cell> {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().filter(|cell| cell.symbol() == "╭").cloned().collect::<Vec<_>>())
        .find(|corners| !corners.is_empty())
        .unwrap()
}

fn accented_zones(terminal: &Terminal<TestBackend>) -> Vec<usize> {
    zone_corners(terminal)
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.modifier.contains(Modifier::BOLD))
        .map(|(zone, _)| zone)
        .collect()
}

const LIST_ZONE: usize = 0;
const RIGHT_ZONE: usize = 1;

#[test]
fn list_focus_moves_the_selection_and_switches_tabs_but_never_scrolls() {
    let mut app = app("list");
    app.max_scroll = 50;
    app.scroll = 10;
    app.follow_bottom = false;
    let first = app.selected_path.clone();
    press(&mut app, KeyCode::Down);
    assert_ne!(app.selected_path, first);
    assert_eq!(app.scroll, 10);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.active, TabKey::Favorites);
}

#[test]
fn transcript_focus_scrolls_and_never_moves_the_selection_or_tab() {
    let mut app = app("transcript");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.focus, Focus::Conversation);
    app.max_scroll = 50;
    app.scroll = 10;
    app.follow_bottom = false;
    let selected = app.selected_path.clone();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Right);
    assert_eq!((app.scroll, app.selected_path.clone(), app.active.clone()), (11, selected, TabKey::All));
}

#[test]
fn embedded_focus_forwards_scroll_and_tab_keys_to_the_child() {
    let mut app = app("embedded");
    launch_embedded(&mut app);
    let selected = app.selected_path.clone();
    for code in [KeyCode::Down, KeyCode::Right, KeyCode::PageDown] {
        press(&mut app, code);
    }
    assert_eq!((app.focus, app.selected_path.clone(), app.active.clone()), (Focus::Embedded, selected, TabKey::All));
}

#[test]
fn first_frame_focuses_the_list_and_exactly_one_zone_is_accented_in_every_state() {
    for (width, height) in [(120, 35), (60, 20)] {
        let mut app = app("accent");
        assert_eq!(app.focus, Focus::List);
        assert_eq!(accented_zones(&draw(&mut app, width, height)), [LIST_ZONE]);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, Focus::Conversation);
        assert_eq!(accented_zones(&draw(&mut app, width, height)), [RIGHT_ZONE]);

        press(&mut app, KeyCode::Esc);
        launch_embedded(&mut app);
        assert_eq!(accented_zones(&draw(&mut app, width, height)), [RIGHT_ZONE]);

        app.focus = Focus::List;
        assert_eq!(accented_zones(&draw(&mut app, width, height)), [LIST_ZONE]);
    }
}

#[test]
fn embedded_border_carries_the_session_identity_colour_and_brightness_shows_focus() {
    let mut app = app("tint");
    let id = launch_embedded(&mut app);
    let tint = app.identity.color(&app.sessions.get(id).unwrap().identity_key());
    let focused = zone_corners(&draw(&mut app, 120, 35))[RIGHT_ZONE].clone();
    assert_eq!(focused.fg, tint);
    assert!(focused.modifier.contains(Modifier::BOLD));
    app.focus = Focus::List;
    let unfocused = zone_corners(&draw(&mut app, 120, 35))[RIGHT_ZONE].clone();
    assert_eq!(unfocused.fg, tint);
    assert!(unfocused.modifier.contains(Modifier::DIM) && !unfocused.modifier.contains(Modifier::BOLD));
}

#[test]
fn footer_shows_only_the_focused_zones_keys() {
    let footer_text = |terminal: &Terminal<TestBackend>| -> String {
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        buffer.content().chunks(width).last().unwrap().iter().map(|cell| cell.symbol()).collect()
    };
    let mut app = app("footer");
    let list = footer_text(&draw(&mut app, 120, 35));
    assert!(list.contains("LIST") && list.contains("tab") && list.contains("quit"), "{list}");
    press(&mut app, KeyCode::Enter);
    let transcript = footer_text(&draw(&mut app, 120, 35));
    assert!(transcript.contains("TRANSCRIPT") && transcript.contains("scroll") && !transcript.contains(" tab "), "{transcript}");
    assert!(transcript.contains("quit"));
}

#[test]
fn c_cycles_the_colour_from_the_list_and_the_transcript_and_after_the_prefix() {
    let mut app = app("cycle");
    let key = identity::Key::Session("a".to_owned());
    let original = app.identity.index(&key);
    press(&mut app, KeyCode::Char('C'));
    assert_eq!(app.identity.index(&key), (original + 1) % palette::SIZE);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('C'));
    assert_eq!(app.identity.index(&key), (original + 2) % palette::SIZE);

    let id = launch_embedded(&mut app);
    let host_key = app.sessions.get(id).unwrap().identity_key();
    app.sync_identity();
    let before = app.identity.index(&host_key);
    app.handle_key(prefix::PREFIX);
    press(&mut app, KeyCode::Char('C'));
    assert_eq!(app.identity.index(&host_key), (before + 1) % palette::SIZE);
}

#[test]
fn a_session_keeps_its_colour_when_its_id_arrives() {
    let mut app = app("bind");
    let id = launch_embedded(&mut app);
    for n in 0..6 {
        let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
        let launched = Launched { title: format!("o{n}"), session_id: None };
        app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
    }
    app.sync_identity();
    let host_key = app.sessions.get(id).unwrap().identity_key();
    let before = app.identity.color(&host_key);
    let pid = app.sessions.get(id).unwrap().host().process_id().unwrap();
    let live = crate::registry::LiveSession {
        pid,
        pid_alive: true,
        session_id: Some("late-bound-id".to_owned()),
        cwd: None,
        started_at_ms: None,
        updated_at_ms: None,
        status_updated_at_ms: None,
        status: crate::registry::Status::Idle,
        name: None,
        name_source: None,
        version: None,
        kind: None,
        entrypoint: None,
        transcript: None,
    };
    app.apply(Update::Live(vec![live]));
    let bound_key = app.sessions.get(id).unwrap().identity_key();
    assert_eq!(bound_key, identity::Key::Session("late-bound-id".to_owned()));
    assert_eq!(app.identity.color(&bound_key), before);
}
