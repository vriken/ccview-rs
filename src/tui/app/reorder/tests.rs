use crate::store::order::ManualOrder;
use crate::tui::{app::tests::support::*, app::*};
use crate::{session::entry::SessionEntry, session::harness_id::HarnessId};
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::{path::PathBuf, time::Duration};

fn entry_at(id: &str, age_rank: u64) -> SessionEntry {
    SessionEntry {
        last_used: std::time::UNIX_EPOCH + Duration::from_secs(age_rank),
        ..entry_in(id, "/w/one", HarnessId::Claude)
    }
}

fn entries() -> Vec<SessionEntry> {
    vec![
        entry_at("a", 10),
        entry_at("b", 20),
        entry_at("c", 30),
        entry_at("d", 40),
    ]
}

fn app_in(name: &str, order_file: &std::path::Path) -> App {
    app_with(name, entries()).with_order(ManualOrder::load(order_file.to_path_buf()))
}

fn order_file(name: &str) -> PathBuf {
    temp_dir(&format!("reorder-{name}")).join("order.json")
}

fn ids(app: &App) -> Vec<String> {
    app.rows
        .iter()
        .map(|&i| app.catalog.entries()[i].session_id.clone())
        .collect()
}

fn mouse(app: &mut App, kind: MouseEventKind, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column: 5,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn row_y(app: &App, index: u16) -> u16 {
    app.regions.list.y + 2 + index * 3
}

#[test]
fn dragging_a_row_to_a_new_place_changes_the_order_and_survives_a_restart() {
    let file = order_file("drag");
    let mut app = app_in("drag", &file);
    drawn(&mut app, 120, 30);
    assert_eq!(ids(&app), ["d", "c", "b", "a"]);
    let (from, to) = (row_y(&app, 2), row_y(&app, 0));
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), from);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), to);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), to);
    assert_eq!(ids(&app), ["b", "d", "c", "a"]);
    assert_eq!(app.selected_entry().unwrap().session_id, "b");

    let restarted = app_in("drag-2", &file);
    assert_eq!(ids(&restarted), ["b", "d", "c", "a"]);

    let (from, to) = (row_y(&app, 0), row_y(&app, 2));
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), from);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), to);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), to);
    assert_eq!(ids(&app), ["d", "c", "b", "a"]);
}

#[test]
fn unlisted_sessions_keep_the_default_order_below_and_a_vanished_id_is_kept_but_ignored() {
    let file = order_file("rest");
    std::fs::write(&file, r#"{"all":["c","gone","a"]}"#).unwrap();
    let mut app = app_in("rest", &file);
    assert_eq!(ids(&app), ["c", "a", "d", "b"]);

    app.select_path(Some(PathBuf::from("/p/a.jsonl")));
    app.handle_event(Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Up,
        KeyModifiers::ALT,
    )));
    assert_eq!(ids(&app), ["a", "c", "d", "b"]);
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(saved.contains("gone"), "{saved}");
}

#[test]
fn a_plain_click_selects_without_reordering() {
    let file = order_file("click");
    let mut app = app_in("click", &file);
    drawn(&mut app, 120, 30);
    let y = row_y(&app, 2);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), y);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), y);
    assert_eq!(ids(&app), ["d", "c", "b", "a"]);
    assert_eq!(app.selected_entry().unwrap().session_id, "b");
    assert!(!file.exists());

    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), y);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), y + 1);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), y + 1);
    assert_eq!(ids(&app), ["d", "c", "b", "a"]);
    assert!(!file.exists());
}
