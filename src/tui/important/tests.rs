use super::super::{test_support::*, *};
use crate::activity::{Activity, ActivityMap, ActivityState};
use std::time::Duration;

fn entry_at(id: &str, cwd: &str, age_rank: u64) -> SessionEntry {
    SessionEntry {
        last_used: std::time::UNIX_EPOCH + Duration::from_secs(age_rank),
        ..entry_in(id, cwd, HarnessId::Claude)
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ccview-important-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn app_in(name: &str, entries: Vec<SessionEntry>) -> (App, PathBuf) {
    let dir = temp_dir(name);
    let app =
        app_with(name, entries).with_important(ImportantSessions::load(dir.join("important.json")));
    (app, dir)
}

fn ids(app: &App) -> Vec<String> {
    app.rows
        .iter()
        .map(|&i| app.library.entries()[i].session_id.clone())
        .collect()
}

fn mark(app: &mut App, id: &str) {
    app.select_path(Some(PathBuf::from(format!("/p/{id}.jsonl"))));
    press(app, KeyCode::Char('i'));
}

fn activity(states: &[(&str, ActivityState)]) -> ActivityMap {
    states
        .iter()
        .map(|(id, state)| {
            let activity = Activity {
                state: state.clone(),
                background: false,
                attach_id: None,
                agents: Vec::new(),
                last_activity: None,
            };
            ((*id).to_owned(), activity)
        })
        .collect()
}

#[test]
fn an_important_session_stays_above_newer_ones_in_all_and_its_directory_tab() {
    let (mut app, dir) = app_in(
        "order",
        vec![
            entry_at("old", "/w/one", 10),
            entry_at("mid", "/w/one", 20),
            entry_at("new", "/w/one", 30),
            entry_at("older", "/w/one", 5),
        ],
    );
    assert_eq!(ids(&app), ["new", "mid", "old", "older"]);
    mark(&mut app, "old");
    assert_eq!(ids(&app), ["old", "new", "mid", "older"]);
    assert_eq!(app.selected_entry().unwrap().session_id, "old");
    mark(&mut app, "older");
    assert_eq!(ids(&app), ["old", "older", "new", "mid"]);
    app.switch_tab(3);
    assert_eq!(app.active, TabKey::Dir("/w/one".to_owned()));
    assert_eq!(ids(&app), ["old", "older", "new", "mid"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn needs_you_outranks_important_and_important_outranks_busy_and_idle_in_active() {
    let (mut app, dir) = app_in(
        "active",
        vec![
            entry_at("idle", "/w", 40),
            entry_at("busy", "/w", 30),
            entry_at("blocked", "/w", 10),
            entry_at("vip", "/w", 20),
        ],
    );
    app.apply(Update::Activity(activity(&[
        ("idle", ActivityState::Idle),
        ("busy", ActivityState::Busy),
        ("blocked", ActivityState::Blocked),
        ("vip", ActivityState::Idle),
    ])));
    mark(&mut app, "vip");
    app.switch_tab(2);
    assert_eq!(app.active, TabKey::Active);
    assert_eq!(ids(&app), ["blocked", "vip", "busy", "idle"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_flag_survives_a_restart_and_a_corrupt_file_loads_empty() {
    let entries = || vec![entry_at("a", "/w", 10), entry_at("b", "/w", 20)];
    let (mut app, dir) = app_in("restart", entries());
    mark(&mut app, "a");
    drop(app);
    let restarted = app_with("restart-2", entries())
        .with_important(ImportantSessions::load(dir.join("important.json")));
    assert_eq!(ids(&restarted), ["a", "b"]);

    std::fs::write(dir.join("important.json"), "{not json").unwrap();
    let mut corrupt = app_with("restart-3", entries())
        .with_important(ImportantSessions::load(dir.join("important.json")));
    assert_eq!(ids(&corrupt), ["b", "a"]);
    mark(&mut corrupt, "a");
    assert_eq!(ids(&corrupt), ["a", "b"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_flag_follows_a_lineage_when_the_visible_member_changes() {
    let dir = temp_dir("lineage");
    let mut important = ImportantSessions::load(dir.join("important.json"));
    let visible = SessionEntry {
        copies: vec!["old".to_owned()],
        ..entry_at("new", "/w", 20)
    };
    important.toggle_entry(&visible).unwrap();
    let newer_visible = SessionEntry {
        copies: vec!["new".to_owned(), "old".to_owned()],
        ..entry_at("newest", "/w", 30)
    };
    assert!(important.contains_entry(&newer_visible));
    important.toggle_entry(&newer_visible).unwrap();
    assert!(!important.contains("new") && !important.contains("old"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn i_does_nothing_from_the_tab_bar() {
    let (mut app, dir) = app_in(
        "tabbar",
        vec![entry_at("a", "/w", 10), entry_at("b", "/w", 20)],
    );
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Char('i'));
    assert_eq!(ids(&app), ["b", "a"]);
    assert!(!dir.join("important.json").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_frame_shows_the_marker_and_the_order_at_both_sizes() {
    let (mut app, dir) = app_in(
        "frame",
        vec![
            entry_at("alpha", "/w/app", 10),
            entry_at("omega", "/w/app", 20),
        ],
    );
    app.favorites.toggle("alpha").unwrap();
    mark(&mut app, "alpha");
    for (width, height) in [(120, 35), (60, 20)] {
        let screen = screen_lines(&mut app, width, height);
        let row_of = |title: &str| {
            screen
                .iter()
                .position(|line| line.contains(title) && !line.contains("TRANSCRIPT"))
                .unwrap()
        };
        let alpha = &screen[row_of("alpha")];
        assert!(alpha.contains("★") && alpha.contains("! alpha"), "{alpha}");
        assert!(!screen[row_of("omega")].contains("! omega"));
        assert!(row_of("alpha") < row_of("omega"), "{width}x{height}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
