use super::*;
use crate::{
    harness::Launch,
    library::TitleSource,
    terminal::{CommandSpec, Size},
    ui_settings::TerminalApp,
};
use sessions::Launched;
use std::{
    fs,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

struct Env {
    dir: PathBuf,
}

impl Env {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn entry(id: &str, cwd: &str) -> SessionEntry {
    SessionEntry {
        path: PathBuf::from(format!("/p/{id}.jsonl")),
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        title: format!("title-{id}"),
        title_source: TitleSource::Prompt,
        last_prompt: None,
        last_used: UNIX_EPOCH,
        live: None,
        ..SessionEntry::stub()
    }
}

fn app(name: &str) -> (App, Env) {
    let dir = std::env::temp_dir().join(format!("ccview-settings-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(
        Favorites::load(dir.join("favorites.json")),
        Identity::load(dir.join("colors.json")),
        None,
        mpsc::channel().0,
    )
    .with_ui_settings(UiSettingsStore::load(dir.join("ui.json")))
    .with_hidden(HiddenSessions::load(dir.join("hidden.json")));
    app.view = ViewSetting::load(dir.join("view.json"));
    app.apply(Update::Scanned(vec![entry("a", "/w/one"), entry("b", "/w/two")]));
    (app, Env { dir })
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_chars(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn row(app: &mut App, index: usize) {
    press(app, KeyCode::Char(','));
    for _ in 0..index {
        press(app, KeyCode::Down);
    }
}

fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    test_support::screen_lines(app, width, height)
}

fn recording_child(dir: &Path) -> (CommandSpec, PathBuf) {
    let out = dir.join("stdin.bin");
    let script = dir.join("child.sh");
    fs::write(&script, format!("#!/bin/sh\nstty raw echo; exec cat > {}\n", out.display())).unwrap();
    (CommandSpec::new("sh").args([script.to_str().unwrap()]), out)
}

fn wait_for_bytes(path: &Path, expected: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while fs::read(path).unwrap_or_default() != expected {
        assert!(Instant::now() < deadline, "child saw {:?}", fs::read(path).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn comma_and_question_mark_open_the_overlays_and_escape_closes_them() {
    let (mut app, _env) = app("open");
    for focus_keys in [vec![], vec![KeyCode::Up], vec![KeyCode::Enter]] {
        for code in focus_keys {
            press(&mut app, code);
        }
        press(&mut app, KeyCode::Char(','));
        assert!(screen(&mut app, 120, 35).join("\n").contains("Identity colours"));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('?'));
        assert!(screen(&mut app, 120, 35).join("\n").contains("List rows"));
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none());
        app.focus = Focus::List;
    }
}

#[test]
fn an_open_overlay_owns_every_key_and_the_child_never_sees_them() {
    let (mut app, env) = app("isolation");
    let (spec, out) = recording_child(&env.dir);
    let launched = Launched { title: "child".to_owned(), session_id: None, harness: HarnessId::Claude };
    let id = app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
    app.show_embedded(id);

    app.handle_key(prefix::PREFIX);
    press(&mut app, KeyCode::Char(','));
    assert!(app.overlay.is_some());
    type_chars(&mut app, "qxrnlabc ");
    for code in [KeyCode::Enter, KeyCode::Left, KeyCode::Down, KeyCode::Tab, KeyCode::PageUp] {
        press(&mut app, code);
    }
    app.handle_key(prefix::PREFIX);
    app.handle_event(Event::Paste("pasted".to_owned()));
    assert!(app.overlay.is_some());
    press(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_none());
    assert_eq!((app.focus, app.should_quit), (Focus::Embedded, false));

    press(&mut app, KeyCode::Char('Z'));
    wait_for_bytes(&out, b"Z");
    app.sessions.remove(id);
}

#[test]
fn the_list_does_not_react_to_keys_while_an_overlay_is_open() {
    let (mut app, _env) = app("list-isolation");
    press(&mut app, KeyCode::Char(','));
    type_chars(&mut app, "qxsnrv");
    press(&mut app, KeyCode::Down);
    assert!(!app.should_quit && app.hidden.count() == 0 && app.confirm.is_none());
    assert_eq!(app.view.level, verbosity::Level::Chat);
    assert_eq!(app.selected_entry().map(|e| e.session_id.as_str()), Some("a"));
}

#[test]
fn view_level_and_mouse_capture_apply_immediately_and_persist() {
    let (mut app, env) = app("view-mouse");
    row(&mut app, 2);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.view.level, verbosity::Level::Tools);
    assert!(env.read("view.json").contains("tools"));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(!app.split.mouse_enabled());
    assert!(env.read("ui.json").contains("\"mouse\": false"));
    press(&mut app, KeyCode::Esc);
    assert!(!app.wants_mouse_capture());
}

#[test]
fn turning_identity_colours_off_removes_the_bars_and_persists() {
    let (mut app, env) = app("colours");
    let live = crate::registry::LiveSession {
        pid: 1,
        pid_alive: true,
        session_id: Some("a".to_owned()),
        cwd: None,
        started_at_ms: None,
        updated_at_ms: None,
        status_updated_at_ms: None,
        status: crate::registry::Status::Idle,
        name: None,
        name_source: None,
        version: None,
        kind: None,
        entrypoint: None,
        transcript: None,
    };
    app.apply(Update::Live(vec![live]));
    let bars = |app: &mut App| screen(app, 120, 35).iter().map(|line| line.matches('▌').count()).sum::<usize>();
    assert!(bars(&mut app) > 0);
    row(&mut app, 0);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    assert_eq!(bars(&mut app), 0);
    assert!(env.read("ui.json").contains("\"identity_colors\": false"));
}

#[test]
fn the_colour_picker_changes_the_selected_session_and_persists_the_override() {
    let (mut app, env) = app("picker");
    let key = identity::Key::Session("a".to_owned());
    let before = app.identity.index(&key);
    row(&mut app, 1);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Left);
    assert_eq!(app.identity.index(&key), (before + 1) % palette::SIZE);
    assert!(env.read("colors.json").contains(&format!(": {}", (before + 1) % palette::SIZE)));
}

#[test]
fn split_ratio_changes_and_resets_to_the_default() {
    let (mut app, env) = app("split");
    test_support::screen_lines(&mut app, 120, 35);
    row(&mut app, 4);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert!(app.split.ratio() > crate::ui_settings::DEFAULT_SPLIT_RATIO);
    assert!(!env.read("ui.json").contains("0.35"));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.split.ratio(), crate::ui_settings::DEFAULT_SPLIT_RATIO);
    assert!(env.read("ui.json").contains("0.35"));
}

#[test]
fn terminal_app_choice_persists_and_is_what_launches_use() {
    if external::app_override().is_some() {
        return;
    }
    let (mut app, env) = app("terminal");
    row(&mut app, 6);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.terminal_app(), TerminalApp::Ghostty);
    assert!(env.read("ui.json").contains("\"terminal_app\": \"ghostty\""));
}

#[test]
fn disabling_the_agent_view_option_drops_the_variable_from_new_launches() {
    let (mut app, env) = app("agent-view");
    let has_var = |app: &App| {
        app.harness
            .command(&Launch::New, Path::new("/tmp"))
            .unwrap()
            .env
            .iter()
            .any(|(key, _)| key == "CLAUDE_CODE_DISABLE_AGENT_VIEW")
    };
    assert!(has_var(&app));
    row(&mut app, 8);
    press(&mut app, KeyCode::Enter);
    assert!(!has_var(&app));
    assert!(env.read("ui.json").contains("\"disable_agent_view\": false"));
}

#[test]
fn a_hidden_session_comes_back_from_the_sub_list() {
    let (mut app, env) = app("unhide");
    press(&mut app, KeyCode::Char('x'));
    assert!(app.hidden.contains("a") && app.rows.len() == 1);
    row(&mut app, 7);
    let shown = screen(&mut app, 120, 35).join("\n");
    assert!(shown.contains("Hidden sessions (1)"), "{shown}");
    press(&mut app, KeyCode::Enter);
    let sub = screen(&mut app, 120, 35).join("\n");
    assert!(sub.contains("title-a") && sub.contains("/w/one"), "{sub}");
    press(&mut app, KeyCode::Enter);
    assert!(!app.hidden.contains("a") && app.rows.len() == 2);
    assert!(!env.read("hidden.json").contains('a'));
    assert!(screen(&mut app, 120, 35).join("\n").contains("No hidden sessions."));
    press(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_some());
}

#[test]
fn help_scrolls_and_every_zone_is_listed() {
    let (mut app, _env) = app("help");
    press(&mut app, KeyCode::Char('?'));
    let top = screen(&mut app, 60, 20).join("\n");
    assert!(top.contains("List rows") && !top.contains("Everywhere"));
    for _ in 0..3 {
        press(&mut app, KeyCode::PageDown);
    }
    let later = screen(&mut app, 60, 20).join("\n");
    assert_ne!(top, later);
    let text: String = help::body(72).lines.iter().map(|line| line.to_string() + "\n").collect();
    for zone in keys::Zone::HELP_ORDER {
        assert!(text.contains(zone.title()), "{}", zone.title());
    }
}

#[test]
fn every_footer_zone_fits_60_columns_with_help_and_quit_visible() {
    let (mut app, env) = app("footer");
    let footer = |app: &mut App| screen(app, 60, 20).pop().unwrap();
    let list = footer(&mut app);
    assert!(list.contains("? help") && list.contains("q quit") && list.contains("↑/↓ move"), "{list}");
    press(&mut app, KeyCode::Up);
    let tabs = footer(&mut app);
    assert!(tabs.contains("? help") && tabs.contains("q quit") && tabs.contains("←/→ tab"), "{tabs}");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    let transcript = footer(&mut app);
    assert!(transcript.contains("? help") && transcript.contains("q quit") && transcript.contains("←/esc list"), "{transcript}");

    let (spec, _out) = recording_child(&env.dir);
    let launched = Launched { title: "child".to_owned(), session_id: None, harness: HarnessId::Claude };
    let id = app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
    app.show_embedded(id);
    let embedded = footer(&mut app);
    assert!(embedded.contains("← back to list") && embedded.contains("ctrl+space ? help"), "{embedded}");
    app.handle_key(prefix::PREFIX);
    let armed = footer(&mut app);
    assert!(armed.contains("l list") && armed.contains("x detach"), "{armed}");
    app.sessions.remove(id);
}

#[test]
fn a_click_outside_the_overlay_closes_it_and_the_wheel_scrolls_help() {
    let (mut app, _env) = app("mouse");
    press(&mut app, KeyCode::Char('?'));
    screen(&mut app, 120, 35);
    let mouse = |kind, column, row| MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 60, 17));
    let scrolled = app.overlay.as_ref().unwrap().scroll;
    assert!(scrolled > 0);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 60, 17));
    assert!(app.overlay.is_some());
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
    assert!(app.overlay.is_none());
}
