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

#[test]
fn clicking_a_wrapped_url_opens_it_once_while_dragging_over_it_only_selects() {
    let dir = temp_dir("click-link");
    let path = dir.join("s.jsonl");
    let url = format!("https://docs.example/{}", "a".repeat(100));
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"see {url}. then more"}}]}}}}"#
        ) + "\n",
    )
    .unwrap();
    let mut session = entry("s", "/w/link");
    session.path = path.clone();
    session.handle = TranscriptHandle::Rollout(path);
    let session_path = session.path.clone();
    let mut app = app();
    app.apply(Update::Scanned(vec![session]));
    app.select_path(Some(session_path));
    app.focus = Focus::Conversation;
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let record = opened.clone();
    app.link_opener = Box::new(move |url| {
        record.borrow_mut().push(url.to_owned());
        Ok(())
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut lines = screen_lines(&mut app, 80, 20);
    while !lines.iter().any(|line| line.contains("then more")) {
        assert!(Instant::now() < deadline, "transcript never loaded");
        std::thread::sleep(Duration::from_millis(20));
        app.tick();
        lines = screen_lines(&mut app, 80, 20);
    }
    let first_row = lines
        .iter()
        .position(|line| line.contains("https://docs.example/"))
        .unwrap();
    let second_row = first_row + 1;
    assert!(
        lines[second_row].contains('a') && !lines[second_row].contains("https"),
        "the url must wrap onto a second row"
    );
    let column = |row: usize, needle: &str| {
        let byte = lines[row].find(needle).unwrap();
        lines[row][..byte].chars().count() as u16
    };
    let first_col = column(first_row, "https");
    let beside_col = first_col - 1;
    let second_col = column(second_row, "a");

    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        beside_col,
        first_row as u16,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        beside_col,
        first_row as u16,
    );
    assert!(opened.borrow().is_empty());

    drag(
        &mut app,
        (first_col, first_row as u16),
        (first_col + 5, first_row as u16),
    );
    assert!(opened.borrow().is_empty());
    assert!(!app.take_outer_output().is_empty());

    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        second_col,
        second_row as u16,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        second_col,
        second_row as u16,
    );
    assert_eq!(*opened.borrow(), [url]);
    std::fs::remove_dir_all(dir).unwrap();
}

fn recorded_opener(app: &mut App) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let record = opened.clone();
    app.link_opener = Box::new(move |url| {
        record.borrow_mut().push(url.to_owned());
        Ok(())
    });
    opened
}

fn click(app: &mut App, column: u16, row: u16) {
    mouse(app, MouseEventKind::Down(MouseButton::Left), column, row);
    mouse(app, MouseEventKind::Up(MouseButton::Left), column, row);
}

#[test]
fn clicking_a_url_wrapped_over_two_pane_rows_opens_the_whole_url_once() {
    let dir = temp_dir("pane-link");
    let url = format!("https://docs.example/{}", "b".repeat(90));
    let javascript = "javascript:alert(1)";
    let spec = script_child(
        &dir,
        "child.sh",
        &format!(
            "while [ \"$(stty size)\" = \"10 40\" ]; do sleep 0.02; done; printf 'see {url} ok\\r\\nfile:///etc/passwd {javascript}\\r\\n'; exec sleep 30"
        ),
    );
    let mut app = app();
    let id = launch_child(&mut app, &spec);
    drawn(&mut app, 120, 30);
    wait_for_screen(&app, id, "javascript");
    let opened = recorded_opener(&mut app);
    let inner = app.pane_inner;
    let at = |col: u16, row: u16| (inner.x + col, inner.y + row);
    assert_eq!(
        app.sessions
            .get(id)
            .unwrap()
            .host()
            .snapshot()
            .screen()
            .size()
            .1
            .get(),
        inner.width,
    );

    let (x, y) = at(1, 0);
    click(&mut app, x, y);
    let (x, y) = at(8, 0);
    click(&mut app, x, y);
    assert_eq!(opened.borrow().as_slice(), std::slice::from_ref(&url));

    opened.borrow_mut().clear();
    let (x, y) = at(2, 1);
    click(&mut app, x, y);
    assert_eq!(*opened.borrow(), [url]);

    opened.borrow_mut().clear();
    for row in 0..4 {
        let (x, y) = at(2, row + 2);
        click(&mut app, x, y);
    }
    assert!(opened.borrow().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn clicking_a_url_in_a_child_that_uses_the_mouse_goes_to_the_child_and_opens_nothing() {
    let dir = temp_dir("pane-link-mouse");
    let out = dir.join("received");
    let spec = script_child(
        &dir,
        "child.sh",
        &format!(
            "stty raw -echo; printf '\\033[?1002h\\033[?1006hhttps://docs.example/x\\r\\n'; exec cat > {}",
            out.display()
        ),
    );
    let mut app = app();
    let id = launch_child(&mut app, &spec);
    wait_for_screen(&app, id, "docs.example");
    drawn(&mut app, 120, 30);
    let opened = recorded_opener(&mut app);
    let inner = app.pane_inner;
    click(&mut app, inner.x + 3, inner.y);
    wait_for_bytes(&out, b"\x1b[<0;");
    assert!(opened.borrow().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
