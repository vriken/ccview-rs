use crate::catalog::feed::Update;
use crate::session::entry::SessionEntry;
use crate::session::live::{LiveSession, Status};
use crate::sources::AgeFilter;
use crate::tui::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{fs, sync::mpsc::Receiver};

fn running(session_id: &str) -> LiveSession {
    LiveSession {
        pid: 1,
        pid_alive: true,
        session_id: Some(session_id.to_owned()),
        cwd: None,
        started_at_ms: None,
        updated_at_ms: None,
        status_updated_at_ms: None,
        status: Status::Idle,
        name: None,
        name_source: None,
        version: None,
        kind: None,
        entrypoint: None,
        transcript: None,
    }
}

fn app_with_link(name: &str) -> (App, Receiver<AgeFilter>) {
    let dir = std::env::temp_dir().join(format!("ccview-age-app-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("favorites.json"), r#"["starred"]"#).unwrap();
    fs::write(dir.join("important.json"), r#"["flagged"]"#).unwrap();
    let (sender, receiver) = mpsc::channel();
    let app = App::new(
        Favorites::load(dir.join("favorites.json")),
        Identity::load(dir.join("colors.json")),
        None,
        mpsc::channel().0,
    )
    .with_ui_settings(UiSettingsStore::load(dir.join("ui.json")))
    .with_important(ImportantSessions::load(dir.join("important.json")))
    .with_age_updates(sender, AgeFilter::default());
    (app, receiver)
}

#[test]
fn starred_important_and_running_sessions_are_asked_for_whatever_their_age() {
    let (mut app, receiver) = app_with_link("keep");
    app.apply(Update::Live(vec![running("busy-one")]));

    app.publish_age_filter();

    let filter = receiver.try_iter().last().unwrap();
    for id in ["starred", "flagged", "busy-one"] {
        assert!(filter.keep_ids().contains(id), "{id}");
    }
    assert!(!filter.keep_ids().contains("other"));
    assert!(filter.cutoff().is_some());
}

#[test]
fn changing_the_setting_reaches_the_scanner_at_once() {
    let (mut app, receiver) = app_with_link("setting");
    app.publish_age_filter();
    let before = receiver.try_iter().last().unwrap().cutoff().unwrap();

    press(&mut app, KeyCode::Char(','));
    for _ in 0..9 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Right);

    let after = receiver.try_iter().last().unwrap().cutoff().unwrap();
    assert!(after < before);
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn a_held_session_is_asked_for_whatever_its_age() {
    let dir = std::env::temp_dir().join(format!("ccview-age-hold-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let held_entry = SessionEntry {
        session_id: "waiting".to_owned(),
        ..SessionEntry::stub()
    };
    let mut on_hold = OnHoldSessions::load(dir.join("on_hold.json"));
    on_hold.toggle_entry(&held_entry).unwrap();
    let (sender, receiver) = mpsc::channel();
    let mut app = App::new(
        Favorites::load(dir.join("favorites.json")),
        Identity::load(dir.join("colors.json")),
        None,
        mpsc::channel().0,
    )
    .with_ui_settings(UiSettingsStore::load(dir.join("ui.json")))
    .with_on_hold(on_hold)
    .with_age_updates(sender, AgeFilter::default());

    app.publish_age_filter();

    let filter = receiver.try_iter().last().unwrap();
    assert!(filter.keep_ids().contains("waiting"));
}
