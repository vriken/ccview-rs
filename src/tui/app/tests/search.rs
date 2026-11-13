use super::support::*;
use crate::{
    catalog::search::handle::SearchHandle,
    harness::ClaudeCode,
    session::harness_id::HarnessId,
    tui::{app::*, widgets::tabs::TabKey},
};
use crossterm::event::KeyCode;
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

struct Fixture {
    app: App,
    dir: PathBuf,
    before: Option<PathBuf>,
}

fn fixture(name: &str) -> Fixture {
    let dir = temp_dir(name);
    let fake = dir.join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cwd = dir.to_str().unwrap();
    let mut app = app_with(
        name,
        vec![
            entry_in("alpha", "/elsewhere", HarnessId::Claude),
            entry_in("beta1", cwd, HarnessId::Claude),
            entry_in("beta2", cwd, HarnessId::Claude),
        ],
    )
    .with_search(
        SearchHandle::spawn(dir.join("no-index.sqlite"), || {}),
        Arc::default(),
    );
    app.harness = Box::new(ClaudeCode::with_binary(fake.to_str().unwrap()));
    let tab = app
        .tabs
        .iter()
        .position(|tab| tab.key == TabKey::Dir(cwd.to_owned()))
        .unwrap();
    app.select_tab(tab);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.focus, Focus::Conversation);
    let before = app.selected_path.clone();
    press(&mut app, KeyCode::Char('/'));
    Fixture { app, dir, before }
}

fn finish(mut fixture: Fixture) {
    let ids: Vec<u64> = fixture
        .app
        .sessions
        .all()
        .iter()
        .map(|session| session.id)
        .collect();
    for id in ids {
        fixture.app.sessions.remove(id);
    }
    let _ = std::fs::remove_dir_all(fixture.dir);
}

#[test]
fn keys_typed_into_the_query_line_are_text_and_never_list_actions() {
    let mut fixture = fixture("search-typing");
    let app = &mut fixture.app;
    assert!(app.search.is_some());
    assert_eq!(app.focus, Focus::List);

    let typed = "qrxdnsiwcfoa?,/ ";
    for letter in typed.chars() {
        type_char(app, letter);
    }

    assert!(!app.should_quit());
    assert!(app.overlay.is_none());
    assert!(app.confirm.is_none());
    assert!(app.sessions.all().is_empty());
    assert_eq!(app.hidden.count(), 0);
    assert!(!app.catalog.dismissed().contains("beta2"));
    assert!(!app.favorites.contains_entry(app.selected_entry().unwrap()));
    assert_eq!(app.search_text(), typed);
    finish(fixture);
}

#[test]
fn esc_leaves_search_and_restores_the_tab_the_selection_and_the_transcript_focus() {
    let mut fixture = fixture("search-restore");
    let tab_before = fixture.app.active.clone();
    let app = &mut fixture.app;
    for letter in "alpha".chars() {
        type_char(app, letter);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while app.selected_entry().map(|entry| entry.session_id.as_str()) != Some("alpha") {
        assert!(std::time::Instant::now() < deadline, "no result for alpha");
        app.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.focus, Focus::List);

    press(app, KeyCode::Esc);

    assert!(app.search.is_none());
    assert_eq!(app.focus, Focus::Conversation);
    assert_eq!(app.active, tab_before);
    assert_eq!(app.selected_path, fixture.before);
    finish(fixture);
}
