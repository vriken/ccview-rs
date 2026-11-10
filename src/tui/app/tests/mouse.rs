use super::support::*;
use crate::catalog::feed::Update;
use crate::tui::app::*;
use crossterm::event::{Event, KeyModifiers};

fn terminal_lines(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> Vec<String> {
    let width = terminal.backend().buffer().area.width as usize;
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

fn click(app: &mut App, column: u16, row: u16) {
    app.handle_event(Event::Mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn clicking_a_list_row_selects_it_and_wheel_over_the_list_moves_the_selection() {
    let mut app = app();
    app.apply(Update::Scanned(
        (0..12)
            .map(|n| entry(&format!("s{n:02}"), "/w/many"))
            .collect(),
    ));
    app.select_row(8);
    let terminal = drawn(&mut app, 120, 30);
    let lines = terminal_lines(&terminal);
    let target = app
        .rows
        .iter()
        .position(|&i| app.catalog.entries()[i].session_id == "s03")
        .unwrap();
    let screen_row = lines
        .iter()
        .position(|line| line.contains("s03"))
        .expect("s03 visible") as u16;
    click(&mut app, 5, screen_row);
    assert_eq!(app.selected_row(), Some(target));
    let before = app.selected_row().unwrap();
    app.handle_event(Event::Mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollDown,
        column: 5,
        row: screen_row,
        modifiers: KeyModifiers::NONE,
    }));
    assert_eq!(app.selected_row(), Some(before + 1));
}

#[test]
fn wheel_over_a_plain_child_scrolls_its_scrollback_and_over_the_transcript_scrolls_the_transcript()
{
    use crossterm::event::{MouseEvent, MouseEventKind};
    let dir = temp_dir("wheel");
    let spec = script_child(
        &dir,
        "child.sh",
        "i=1; while [ $i -le 80 ]; do printf 'L%03d\\r\\n' $i; i=$((i+1)); done; exec sleep 30",
    );
    let mut app = app();
    let id = launch_child(&mut app, &spec);
    wait_for_screen(&app, id, "L080");
    drawn(&mut app, 120, 30);
    let wheel = |app: &mut App, kind, column| {
        app.handle_event(Event::Mouse(MouseEvent {
            kind,
            column,
            row: 10,
            modifiers: KeyModifiers::NONE,
        }));
    };
    wheel(&mut app, MouseEventKind::ScrollUp, 80);
    assert_eq!(
        app.sessions
            .get(id)
            .unwrap()
            .host()
            .snapshot()
            .scrollback_offset(),
        3
    );
    wheel(&mut app, MouseEventKind::ScrollDown, 80);
    assert_eq!(
        app.sessions
            .get(id)
            .unwrap()
            .host()
            .snapshot()
            .scrollback_offset(),
        0
    );
    app.sessions.remove(id);
    app.shown = None;
    app.focus = Focus::List;
    drawn(&mut app, 120, 30);
    app.max_scroll = 100;
    app.follow_bottom = false;
    app.scroll = 50;
    wheel(&mut app, MouseEventKind::ScrollUp, 80);
    assert_eq!(app.scroll, 47);
    wheel(&mut app, MouseEventKind::ScrollDown, 80);
    assert_eq!(app.scroll, 50);
    std::fs::remove_dir_all(dir).unwrap();
}
