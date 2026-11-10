use super::super::{test_support::*, *};

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
    let selected = app.selected_entry().map_or("", |entry| entry.session_id.as_str());
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
    Outcome { focus, on_tab_bar, tab, selected }
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
        (FirstRow, KeyCode::Char('h'), expect(List, false, all(), "a")),
        (FirstRow, KeyCode::Char('['), expect(List, false, all(), "a")),
        (FirstRow, KeyCode::Char(']'), expect(List, false, all(), "a")),
        (SecondRow, KeyCode::Right, expect(Conversation, false, all(), "b")),
        (FirstRow, KeyCode::Enter, expect(Conversation, false, all(), "a")),
        (TabBar, KeyCode::Right, expect(List, true, favorites(), "-")),
        (TabBar, KeyCode::Char('l'), expect(List, true, favorites(), "-")),
        (TabBar, KeyCode::Char(']'), expect(List, true, favorites(), "-")),
        (TabBar, KeyCode::Left, expect(List, true, last_dir(), "b")),
        (TabBar, KeyCode::Char('h'), expect(List, true, last_dir(), "b")),
        (TabBar, KeyCode::Char('['), expect(List, true, last_dir(), "b")),
        (TabBar, KeyCode::Up, expect(List, true, all(), "a")),
        (TabBar, KeyCode::Down, expect(List, false, all(), "a")),
        (TabBar, KeyCode::Enter, expect(List, false, all(), "a")),
        (TabBar, KeyCode::Esc, expect(List, false, all(), "a")),
        (Transcript, KeyCode::Right, expect(Conversation, false, all(), "a")),
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

#[test]
fn the_tab_bar_focus_is_drawn_apart_from_the_merely_active_tab() {
    let mut app = app_at(Start::FirstRow);
    let on_rows = screen_lines(&mut app, 120, 35)[1].clone();
    assert!(!on_rows.contains('▶'), "{on_rows}");
    press(&mut app, KeyCode::Up);
    let on_tabs = screen_lines(&mut app, 120, 35)[1].clone();
    assert!(on_tabs.contains("▶All"), "{on_tabs}");
    let footer = screen_lines(&mut app, 120, 35).pop().unwrap();
    assert!(footer.contains("←/→ tab") && footer.contains("↓ rows"), "{footer}");
    press(&mut app, KeyCode::Down);
    let footer = screen_lines(&mut app, 120, 35).pop().unwrap();
    assert!(footer.contains("↑ tabs") && footer.contains("→ open"), "{footer}");
}
