use super::support::*;
use crate::{
    catalog::feed::Update,
    session::entry::TranscriptHandle,
    tui::{app::*, outer_terminal},
};
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};
use vt100::MouseProtocolMode;

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) {
    mouse(app, MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    mouse(app, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    mouse(app, MouseEventKind::Up(MouseButton::Left), to.0, to.1);
}

#[test]
fn dragging_across_wrapped_transcript_lines_copies_what_is_on_screen() {
    let dir = temp_dir("copy-transcript");
    let path = dir.join("s.jsonl");
    let words: Vec<String> = (10..70).map(|n| format!("w{n}")).collect();
    let text = words.join(" ");
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        ) + "\n",
    )
    .unwrap();
    let mut session = entry("s", "/w/copy");
    session.path = path.clone();
    session.handle = TranscriptHandle::Rollout(path);
    let session_path = session.path.clone();
    let mut app = app();
    app.apply(Update::Scanned(vec![session]));
    app.select_path(Some(session_path));
    app.focus = Focus::Conversation;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut lines = screen_lines(&mut app, 80, 20);
    while !lines.iter().any(|line| line.contains("w69")) {
        assert!(Instant::now() < deadline, "transcript never loaded");
        std::thread::sleep(Duration::from_millis(20));
        app.tick();
        lines = screen_lines(&mut app, 80, 20);
    }
    let area = app.text_area;
    let cells = |line: &str| -> Vec<char> { line.chars().collect() };
    let locate = |needle: &str| {
        lines
            .iter()
            .enumerate()
            .find_map(|(row, line)| {
                line.find(needle)
                    .map(|byte| (row, line[..byte].chars().count()))
            })
            .unwrap()
    };
    let (first_row, first_col) = locate("w10");
    let (last_row, last_col) = locate("w69");
    assert!(last_row > first_row + 1, "text must wrap over several rows");
    let end_col = last_col + 2;
    drag(
        &mut app,
        (first_col as u16, first_row as u16),
        (end_col as u16, last_row as u16),
    );
    let expected: Vec<String> = (first_row..=last_row)
        .map(|row| {
            let chars = cells(&lines[row]);
            let from = if row == first_row {
                first_col
            } else {
                area.x as usize
            };
            let to = if row == last_row {
                end_col + 1
            } else {
                (area.x + area.width) as usize
            };
            chars[from..to.min(chars.len())]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    let expected = expected.join("\n");
    assert!(expected.starts_with("w10") && expected.ends_with("w69"));
    assert_eq!(
        app.take_outer_output(),
        outer_terminal::clipboard_sequence("c", expected.as_bytes()).unwrap()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_drag_in_a_child_that_uses_the_mouse_reaches_the_child_and_selects_nothing() {
    let dir = temp_dir("copy-mouse-child");
    let out = dir.join("received");
    let spec = script_child(
        &dir,
        "child.sh",
        &format!(
            "stty raw -echo; printf '\\033[?1002h\\033[?1006h'; exec cat > {}",
            out.display()
        ),
    );
    let mut app = app();
    let id = launch_child(&mut app, &spec);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app
        .sessions
        .get(id)
        .unwrap()
        .host()
        .snapshot()
        .screen()
        .mouse_protocol_mode()
        == MouseProtocolMode::None
    {
        assert!(Instant::now() < deadline, "child never enabled the mouse");
        std::thread::sleep(Duration::from_millis(10));
    }
    drawn(&mut app, 120, 30);
    let inner = app.pane_inner;
    drag(
        &mut app,
        (inner.x + 1, inner.y + 1),
        (inner.x + 6, inner.y + 2),
    );
    wait_for_bytes(&out, b"\x1b[<32;");
    assert!(app.selection.is_none());
    assert!(app.take_outer_output().is_empty());
    app.sessions.remove(id);
    std::fs::remove_dir_all(dir).unwrap();
}
