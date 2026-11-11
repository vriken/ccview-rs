use super::support::*;
use crate::terminal::{CommandSpec, Size};
use crate::tui::app::*;
use crate::tui::embedded::manager::Launched;
use crate::{
    catalog::feed::Update, session::harness_id::HarnessId, store::favorites::Favorites,
    tui::style::identity::Identity,
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use std::{path::PathBuf, sync::mpsc, time::Instant};

fn activity_with(
    state: crate::session::activity::ActivityState,
    background: bool,
    agents: usize,
) -> crate::session::activity::Activity {
    crate::session::activity::Activity {
        state,
        background,
        attach_id: background.then(|| "bg1".to_owned()),
        agents: (0..agents)
            .map(|_| crate::session::activity::RunningAgent {
                agent_type: "explore".into(),
                description: String::new(),
            })
            .collect(),
        last_activity: None,
    }
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn wait_until_dead(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while crate::sys::process::pid_alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} still alive");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn app_with_wake() -> (App, Receiver<LoopEvent>) {
    let (sender, receiver) = mpsc::channel();
    let mut app = App::new(
        Favorites::load(
            std::env::temp_dir().join(format!("ccview-wake-{}/f.json", std::process::id())),
        ),
        Identity::load(
            std::env::temp_dir().join(format!("ccview-wake-{}/c.json", std::process::id())),
        ),
        None,
        sender,
    );
    app.apply(Update::Scanned(vec![
        entry("a", "/w/one"),
        entry("b", "/w/two"),
    ]));
    (app, receiver)
}

fn pump_until(
    app: &mut App,
    receiver: &Receiver<LoopEvent>,
    mut condition: impl FnMut(&App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(app) {
        assert!(Instant::now() < deadline, "condition not met in time");
        if let Ok(LoopEvent::Output(id)) = receiver.recv_timeout(Duration::from_millis(20)) {
            app.output_ready(id);
        }
        app.tick();
    }
}

#[test]
fn embedded_session_receives_every_navigation_key() {
    let mut app = app();
    let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
    let launched = Launched {
        title: "t".to_owned(),
        session_id: None,
        harness: HarnessId::Claude,
    };
    let id = app
        .sessions
        .launch(&spec, launched, Size { rows: 10, cols: 40 })
        .unwrap();
    app.show_embedded(id);
    for code in [
        KeyCode::Right,
        KeyCode::Esc,
        KeyCode::Char('q'),
        KeyCode::Char('l'),
        KeyCode::Tab,
    ] {
        press(&mut app, code);
        assert_eq!(
            (app.focus, app.active.clone(), app.should_quit),
            (Focus::Embedded, TabKey::All, false),
            "{code:?}"
        );
    }
}

#[test]
fn attach_runs_claude_attach_with_the_background_id_and_only_for_background_sessions() {
    use crate::session::activity::ActivityState;
    let dir = std::env::temp_dir().join(format!("ccview-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\necho \"ran: $@\"\nexec sleep 60\n").unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&fake)
        .status()
        .unwrap();

    let mut app = app();
    app.harness = Box::new(harness::ClaudeCode::with_binary(fake.to_str().unwrap()));
    let cwd = dir.to_str().unwrap();
    app.apply(Update::Scanned(vec![entry("bgsession", cwd)]));
    let mut activity = crate::session::activity::ActivityMap::new();
    activity.insert(
        "bgsession".to_owned(),
        activity_with(ActivityState::Blocked, true, 0),
    );
    activity.insert("a".to_owned(), activity_with(ActivityState::Busy, false, 0));
    app.apply(Update::Activity(activity));

    app.select_path(Some(PathBuf::from("/p/a.jsonl")));
    type_char(&mut app, 'a');
    assert!(app.sessions.is_empty());
    assert_eq!(app.notice.as_deref(), Some("not a background session"));

    app.select_path(Some(PathBuf::from("/p/bgsession.jsonl")));
    type_char(&mut app, 'a');
    let id = app.shown.expect("attach is shown embedded");
    wait_for_screen(&app, id, "ran: attach bg1");
    assert_eq!(app.focus, Focus::Embedded);
    app.sessions.remove(id);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn left_at_an_empty_prompt_returns_to_the_list_without_reaching_the_child() {
    let dir = temp_dir("left-empty");
    let (spec, out) = recording_child(&dir, "printf '│ > '");
    let (mut app, receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    wait_for_screen(&app, id, "│ >");
    press(&mut app, KeyCode::Left);
    assert_eq!(app.focus, Focus::List);
    assert!(app.sessions.get(id).unwrap().is_running());
    app.show_embedded(id);
    type_char(&mut app, 'z');
    wait_for_bytes(&out, b"z");
    assert_eq!(read_file(&out), b"z");
    drop(receiver);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn left_after_typed_text_or_with_a_hidden_cursor_goes_to_the_child() {
    let dir = temp_dir("left-typed");
    let (spec, out) = recording_child(&dir, "printf '> '");
    let (mut app, _receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    wait_for_screen(&app, id, ">");
    for c in "abc".chars() {
        type_char(&mut app, c);
    }
    wait_for_screen(&app, id, "> abc");
    press(&mut app, KeyCode::Left);
    assert_eq!(app.focus, Focus::Embedded);
    wait_for_bytes(&out, b"abc\x1b[D");
    drop(app);

    let hidden_dir = temp_dir("left-hidden");
    let (spec, out) = recording_child(&hidden_dir, "printf '\\033[?25l> '");
    let (mut app, _receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    wait_for_screen(&app, id, ">");
    press(&mut app, KeyCode::Left);
    assert_eq!(app.focus, Focus::Embedded);
    wait_for_bytes(&out, b"\x1b[D");
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(hidden_dir).unwrap();
}

#[test]
fn clean_exit_returns_to_the_list_drops_the_host_and_swallows_repeated_ctrl_c() {
    let dir = temp_dir("exit-zero");
    let spec = script_child(
        &dir,
        "child.sh",
        "stty raw -echo; printf ready; dd bs=1 count=2 of=/dev/null 2>/dev/null; exit 0",
    );
    let (mut app, receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    let pid = app.sessions.get(id).unwrap().host().process_id().unwrap();
    wait_for_screen(&app, id, "ready");
    app.handle_key(ctrl_c());
    app.handle_key(ctrl_c());
    pump_until(&mut app, &receiver, |app| app.sessions.is_empty());
    assert_eq!(
        (app.focus, app.shown, app.should_quit),
        (Focus::List, None, false)
    );
    assert!(app.alert.as_ref().unwrap().0.starts_with("session ended"));
    app.handle_key(ctrl_c());
    assert!(!app.should_quit);
    wait_until_dead(pid);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failing_exit_keeps_the_pane_and_a_later_ctrl_c_only_dismisses_it() {
    let dir = temp_dir("exit-three");
    let spec = script_child(
        &dir,
        "child.sh",
        "stty raw -echo; printf ready; dd bs=1 count=2 of=/dev/null 2>/dev/null; exit 3",
    );
    let (mut app, receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    wait_for_screen(&app, id, "ready");
    app.handle_key(ctrl_c());
    app.handle_key(ctrl_c());
    pump_until(&mut app, &receiver, |app| {
        app.sessions
            .get(id)
            .is_some_and(|s| s.exit_status().is_some())
    });
    assert_eq!(app.sessions.get(id).unwrap().exit_status().unwrap().code, 3);
    assert_eq!((app.focus, app.shown), (Focus::Embedded, Some(id)));
    app.handle_key(ctrl_c());
    assert!(app.sessions.is_empty());
    assert!(!app.should_quit);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn child_events_reach_the_outer_terminal_and_the_strip() {
    let dir = temp_dir("events");
    let spec = script_child(
        &dir,
        "child.sh",
        "printf '\\033]0;my title\\007\\033]52;c;aGVsbG8=\\007\\033]9;build done\\007'; exec sleep 30",
    );
    let (mut app, receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    pump_until(&mut app, &receiver, |app| {
        !app.outer_output.is_empty() && app.alert.is_some()
    });
    let outer = String::from_utf8_lossy(&app.outer_output).into_owned();
    assert!(outer.contains("\x1b]52;c;aGVsbG8=\x1b\\"), "{outer:?}");
    assert!(outer.contains('\x07'));
    assert_eq!(app.sessions.get(id).unwrap().display_title(), "my title");
    assert!(app.alert.as_ref().unwrap().0.contains("build done"));
    assert!(!app.take_outer_output().is_empty());
    assert!(app.take_outer_output().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn focus_changes_reach_a_child_that_enabled_focus_reporting() {
    let dir = temp_dir("focus");
    let (spec, out) = recording_child(&dir, "printf '\\033[?1004h'");
    let (mut app, _receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.sessions.get(id).unwrap().host().focus_reporting() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    app.handle_event(Event::FocusGained);
    wait_for_bytes(&out, b"\x1b[I");
    app.handle_event(Event::FocusLost);
    wait_for_bytes(&out, b"\x1b[I\x1b[O");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shift_enter_is_csi_u_for_kitty_children_and_plain_otherwise() {
    let dir = temp_dir("shift-enter");
    let (spec, out) = recording_child(&dir, "printf '\\033[>5u'");
    let (mut app, _receiver) = app_with_wake();
    let id = launch_child(&mut app, &spec);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app
        .sessions
        .get(id)
        .unwrap()
        .host()
        .keyboard_flags()
        .is_empty()
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    wait_for_bytes(&out, b"\x1b[13;2u\r");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_pane_demoted_by_another_ccview_sends_nothing_until_it_takes_over() {
    use crate::terminal::session_host::daemon::{self, DaemonConfig};
    use crate::terminal::{HostClient, RuntimeDir};

    let dir = temp_dir("lease-ui");
    let runtime = RuntimeDir::at(dir.join("run"));
    let thread_dir = runtime.clone();
    let daemon = std::thread::spawn(move || {
        daemon::run(
            &thread_dir,
            &DaemonConfig {
                idle_exit: Duration::from_millis(500),
            },
        )
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime.socket().exists() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let client = HostClient::new(runtime);

    let mut first = app().with_session_host(client.clone());
    let (spec, out) = recording_child(&dir, "true");
    let first_id = launch_child(&mut first, &spec);
    let pid = first.sessions.get(first_id).unwrap().host().process_id();

    let mut second = app().with_session_host(client);
    let second_id = second.sessions.all()[0].id;
    second.show_embedded(second_id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !first.sessions.get(first_id).unwrap().host().is_standby() {
        assert!(Instant::now() < deadline, "first pane never went standby");
        std::thread::sleep(Duration::from_millis(10));
    }

    type_char(&mut first, 'x');
    press(&mut first, KeyCode::Enter);
    assert!(!first.sessions.get(first_id).unwrap().host().is_standby());
    type_char(&mut second, 'z');
    type_char(&mut first, 'y');
    wait_for_bytes(&out, b"y");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(read_file(&out), b"y");

    unsafe { libc::kill(pid.unwrap() as libc::pid_t, libc::SIGKILL) };
    drop((first, second));
    daemon.join().unwrap().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
