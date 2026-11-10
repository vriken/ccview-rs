use crate::session::harness_id::HarnessId;
use crate::store::favorite_dirs::FavoriteDirs;
use crate::tui::widgets::tabs;
use crate::tui::{app::tests::support::*, app::*};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    FirstRow,
    SecondRow,
    TabBar,
    Transcript,
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    focus: Focus,
    on_tab_bar: bool,
    tab: TabKey,
    selected: &'static str,
}

fn app_at(start: Start) -> App {
    let mut app = app_with(
        "nav",
        vec![
            entry_in("a", "/w/one", HarnessId::Claude),
            entry_in("b", "/w/two", HarnessId::Claude),
        ],
    );
    match start {
        Start::FirstRow => {}
        Start::SecondRow => press(&mut app, KeyCode::Down),
        Start::TabBar => press(&mut app, KeyCode::Up),
        Start::Transcript => press(&mut app, KeyCode::Enter),
    }
    app
}

fn outcome(app: &App) -> Outcome {
    let selected = app
        .selected_entry()
        .map_or("", |entry| entry.session_id.as_str());
    Outcome {
        focus: app.focus,
        on_tab_bar: app.tab_bar_has_focus(),
        tab: app.active.clone(),
        selected: match selected {
            "a" => "a",
            "b" => "b",
            _ => "-",
        },
    }
}

fn expect(focus: Focus, on_tab_bar: bool, tab: TabKey, selected: &'static str) -> Outcome {
    Outcome {
        focus,
        on_tab_bar,
        tab,
        selected,
    }
}

#[test]
fn keys_route_by_what_is_focused_and_only_the_tab_bar_switches_tabs() {
    use Focus::{Conversation, List};
    use Start::{FirstRow, SecondRow, TabBar, Transcript};
    let all = || TabKey::All;
    let favorites = || TabKey::Favorites;
    let last_dir = || TabKey::Dir("/w/two".to_owned());
    let table = [
        (FirstRow, KeyCode::Up, expect(List, true, all(), "a")),
        (SecondRow, KeyCode::Up, expect(List, false, all(), "a")),
        (FirstRow, KeyCode::Down, expect(List, false, all(), "b")),
        (FirstRow, KeyCode::Left, expect(List, false, all(), "a")),
        (
            FirstRow,
            KeyCode::Char('h'),
            expect(List, false, all(), "a"),
        ),
        (
            FirstRow,
            KeyCode::Char('['),
            expect(List, false, all(), "a"),
        ),
        (
            FirstRow,
            KeyCode::Char(']'),
            expect(List, false, all(), "a"),
        ),
        (
            SecondRow,
            KeyCode::Right,
            expect(Conversation, false, all(), "b"),
        ),
        (
            FirstRow,
            KeyCode::Enter,
            expect(Conversation, false, all(), "a"),
        ),
        (TabBar, KeyCode::Right, expect(List, true, favorites(), "-")),
        (
            TabBar,
            KeyCode::Char('l'),
            expect(List, true, favorites(), "-"),
        ),
        (
            TabBar,
            KeyCode::Char(']'),
            expect(List, true, favorites(), "-"),
        ),
        (TabBar, KeyCode::Left, expect(List, true, last_dir(), "b")),
        (
            TabBar,
            KeyCode::Char('h'),
            expect(List, true, last_dir(), "b"),
        ),
        (
            TabBar,
            KeyCode::Char('['),
            expect(List, true, last_dir(), "b"),
        ),
        (TabBar, KeyCode::Up, expect(List, true, all(), "a")),
        (TabBar, KeyCode::Down, expect(List, false, all(), "a")),
        (TabBar, KeyCode::Enter, expect(List, false, all(), "a")),
        (TabBar, KeyCode::Esc, expect(List, false, all(), "a")),
        (
            Transcript,
            KeyCode::Right,
            expect(Conversation, false, all(), "a"),
        ),
        (Transcript, KeyCode::Left, expect(List, false, all(), "a")),
        (Transcript, KeyCode::Esc, expect(List, false, all(), "a")),
        (Transcript, KeyCode::Tab, expect(List, false, all(), "a")),
    ];
    for (start, key, expected) in table {
        let mut app = app_at(start);
        press(&mut app, key);
        assert_eq!(outcome(&app), expected, "{start:?} + {key:?}");
    }
}

fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

#[test]
fn shift_arrows_switch_tabs_from_rows_tab_bar_and_transcript_without_changing_focus() {
    use Focus::{Conversation, List};
    use Start::{FirstRow, TabBar, Transcript};
    let table = [
        (
            FirstRow,
            KeyCode::Right,
            expect(List, false, TabKey::Favorites, "-"),
        ),
        (
            TabBar,
            KeyCode::Right,
            expect(List, true, TabKey::Favorites, "-"),
        ),
        (
            Transcript,
            KeyCode::Right,
            expect(Conversation, false, TabKey::Favorites, "-"),
        ),
        (
            FirstRow,
            KeyCode::Left,
            expect(List, false, TabKey::Dir("/w/two".to_owned()), "b"),
        ),
        (
            TabBar,
            KeyCode::Left,
            expect(List, true, TabKey::Dir("/w/two".to_owned()), "b"),
        ),
        (
            Transcript,
            KeyCode::Left,
            expect(Conversation, false, TabKey::Dir("/w/two".to_owned()), "b"),
        ),
    ];
    for (start, code, expected) in table {
        let mut app = app_at(start);
        press_with(&mut app, code, KeyModifiers::SHIFT);
        assert_eq!(outcome(&app), expected, "{start:?} + shift {code:?}");
    }
}

fn dir_app(name: &str) -> (App, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "ccview-nav-{}-{name}/favorite_dirs.json",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    let mut newest = entry_in("a", "/w/one", HarnessId::Claude);
    newest.last_used = UNIX_EPOCH + std::time::Duration::from_secs(30);
    let mut middle = entry_in("b", "/w/two", HarnessId::Claude);
    middle.last_used = UNIX_EPOCH + std::time::Duration::from_secs(20);
    let mut oldest = entry_in("c", "/w/three", HarnessId::Claude);
    oldest.last_used = UNIX_EPOCH + std::time::Duration::from_secs(10);
    let app = app_with(name, vec![newest, middle, oldest])
        .with_favorite_dirs(FavoriteDirs::load(path.clone()));
    (app, path)
}

fn labels(app: &App) -> Vec<String> {
    app.tabs.iter().map(|tab| tab.label.clone()).collect()
}

#[test]
fn starred_directories_lead_the_directory_tabs_and_survive_a_restart() {
    let (mut app, path) = dir_app("dirs");
    assert_eq!(labels(&app)[3..], ["one", "two", "three"]);

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected_entry().unwrap().cwd, "/w/three");
    press(&mut app, KeyCode::Char('S'));
    assert_eq!(labels(&app)[3..], ["★ three", "one", "two"]);

    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    assert!(app.tab_bar_has_focus());
    app.switch_tab(1);
    app.switch_tab(1);
    app.switch_tab(1);
    app.switch_tab(1);
    assert_eq!(app.active, TabKey::Dir("/w/one".to_owned()));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(labels(&app)[3..], ["★ one", "★ three", "two"]);
    assert!(!app.favorites.contains("a"));

    let mut restarted = app_with("dirs-restart", app.catalog.entries().to_vec())
        .with_favorite_dirs(FavoriteDirs::load(path.clone()));
    assert_eq!(labels(&restarted)[3..], ["★ one", "★ three", "two"]);
    let bar = screen_lines(&mut restarted, 200, 20)[1].clone();
    assert!(bar.contains("★ one"), "{bar}");
    let narrow: String = tabs::render_bar(&restarted.tabs, &restarted.active, 60, false)
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(
        narrow.chars().count() <= 60 && narrow.contains("★ one"),
        "{narrow}"
    );
    assert!(!path.with_file_name("favorites.json").exists());
    press(&mut restarted, KeyCode::Up);
    press(&mut restarted, KeyCode::Char('s'));
    assert_eq!(labels(&restarted)[3..], ["★ one", "★ three", "two"]);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn the_selection_follows_its_session_when_a_rescan_moves_it() {
    let mut older = entry_in("a", "/w/one", HarnessId::Claude);
    older.last_used = UNIX_EPOCH + std::time::Duration::from_secs(10);
    let mut newer = entry_in("b", "/w/one", HarnessId::Claude);
    newer.last_used = UNIX_EPOCH + std::time::Duration::from_secs(20);
    let mut app = app_with("selection-follows", vec![older.clone(), newer]);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected_row(), Some(1));
    assert_eq!(app.selected_path, Some(older.path.clone()));

    older.last_used = UNIX_EPOCH + std::time::Duration::from_secs(30);
    app.apply(Update::Scanned(vec![older.clone()]));
    assert_eq!(app.selected_path, Some(older.path));
    assert_eq!(app.selected_row(), Some(0));
}
