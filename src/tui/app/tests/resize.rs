use super::support::*;
use crate::tui::app::*;
use crate::tui::embedded::prefix;
use crossterm::event::KeyCode;

fn list_width_now(app: &App) -> u16 {
    app.regions.list.width
}

#[test]
fn angle_keys_resize_in_list_and_transcript_but_belong_to_the_child_when_embedded() {
    let mut app = app();
    drawn(&mut app, 120, 30);
    let initial = list_width_now(&app);
    type_char(&mut app, '>');
    drawn(&mut app, 120, 30);
    assert_eq!(list_width_now(&app), initial + 2);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.focus, Focus::Conversation);
    type_char(&mut app, '<');
    type_char(&mut app, '<');
    drawn(&mut app, 120, 30);
    assert_eq!(list_width_now(&app), initial - 2);
    type_char(&mut app, '=');
    drawn(&mut app, 120, 30);
    assert_eq!(list_width_now(&app), initial);

    let dir = temp_dir("angle");
    let (spec, out) = recording_child(&dir, "true");
    let id = launch_child(&mut app, &spec);
    type_char(&mut app, '>');
    wait_for_bytes(&out, b">");
    drawn(&mut app, 120, 30);
    assert_eq!(list_width_now(&app), initial);
    app.handle_key(prefix::PREFIX);
    type_char(&mut app, '>');
    drawn(&mut app, 120, 30);
    assert_eq!(list_width_now(&app), initial + 2);
    assert_eq!(read_file(&out), b">");
    app.sessions.remove(id);
    std::fs::remove_dir_all(dir).unwrap();
}
