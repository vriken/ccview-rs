use super::super::{test_support::*, *};

fn ids(app: &App) -> Vec<String> {
    app.rows.iter().map(|&i| app.library.entries()[i].session_id.clone()).collect()
}

fn tab_labels(app: &App) -> Vec<String> {
    app.tabs.iter().map(|tab| tab.label.clone()).collect()
}

fn app_in(name: &str) -> (App, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("ccview-hide-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = app_with(
        name,
        vec![
            entry_in("a", "/w/one", HarnessId::Claude),
            entry_in("b", "/w/two", HarnessId::Claude),
            entry_in("c", "/w/two", HarnessId::Claude),
        ],
    )
    .with_hidden(HiddenSessions::load(dir.join("hidden.json")));
    app.favorites.toggle("b").unwrap();
    app.rebuild_view();
    (app, dir)
}

#[test]
fn x_hides_the_session_from_every_tab_and_the_selection_moves_to_the_neighbour() {
    let (mut app, dir) = app_in("tabs");
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected_entry().unwrap().session_id, "b");
    app.switch_tab(1);
    assert_eq!(ids(&app), ["b"]);
    app.switch_tab(-1);
    app.select_path(Some(PathBuf::from("/p/b.jsonl")));
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(ids(&app), ["a", "c"]);
    assert_eq!(app.selected_entry().unwrap().session_id, "c");
    app.switch_tab(1);
    assert!(app.rows.is_empty());
    app.switch_tab(-1);
    app.select_path(Some(PathBuf::from("/p/c.jsonl")));
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(ids(&app), ["a"]);
    assert_eq!(app.selected_entry().unwrap().session_id, "a");
    assert_eq!(tab_labels(&app), ["All", "★ Favorites", "Active", "one"]);
    app.switch_tab(1);
    assert!(app.rows.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn show_hidden_lists_them_dimmed_with_a_marker_and_x_restores_one() {
    let (mut app, dir) = app_in("show");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(ids(&app), ["a", "c"]);
    let footer = screen_lines(&mut app, 120, 35).pop().unwrap();
    assert!(footer.contains("1 hidden · H show"), "{footer}");

    press(&mut app, KeyCode::Char('H'));
    assert_eq!(ids(&app), ["a", "b", "c"]);
    let screen = screen_lines(&mut app, 120, 35).join("\n");
    assert!(screen.contains("hidden"));
    app.select_path(Some(PathBuf::from("/p/b.jsonl")));
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(ids(&app), ["a", "b", "c"]);
    assert_eq!(app.hidden_note(), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hidden_sessions_stay_hidden_after_a_restart() {
    let (mut app, dir) = app_in("restart");
    press(&mut app, KeyCode::Char('x'));
    drop(app);
    let restarted = app_with(
        "restart-2",
        vec![
            entry_in("a", "/w/one", HarnessId::Claude),
            entry_in("b", "/w/two", HarnessId::Claude),
        ],
    )
    .with_hidden(HiddenSessions::load(dir.join("hidden.json")));
    assert_eq!(ids(&restarted), ["b"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn x_does_nothing_from_the_tab_bar() {
    let (mut app, dir) = app_in("tabbar");
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(ids(&app).len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}
