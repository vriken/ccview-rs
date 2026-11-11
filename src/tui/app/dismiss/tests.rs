use crate::terminal::session_host::{
    client::HostClient,
    daemon::{self, DaemonConfig},
    runtime_dir::RuntimeDir,
};
use crate::tui::embedded::manager::{Launched, SessionManager};
use crate::tui::{app::tests::support::*, app::*};
use crate::{catalog::feed::Update, session::harness_id::HarnessId, store::dismissed::Dismissed};
use crate::{
    session::{
        activity::{Activity, ActivityMap, ActivityState},
        live::{LiveSession, Status},
    },
    terminal::{CommandSpec, Size},
};
use crossterm::event::KeyCode;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const BAR: &str = "▌";

fn live(session_id: &str, pid: u32) -> LiveSession {
    LiveSession {
        pid,
        pid_alive: true,
        session_id: Some(session_id.to_owned()),
        cwd: None,
        started_at_ms: None,
        updated_at_ms: None,
        status_updated_at_ms: None,
        status: Status::Idle,
        name: None,
        name_source: None,
        version: None,
        kind: None,
        entrypoint: None,
        transcript: None,
    }
}

fn activity(state: ActivityState) -> ActivityMap {
    ActivityMap::from([(
        "a".to_owned(),
        Activity {
            state,
            background: false,
            attach_id: None,
            agents: Vec::new(),
            last_activity: None,
        },
    )])
}

fn dismissed_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ccview-dismiss-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("dismissed.json")
}

fn idle_live_app(name: &str, file: &Path) -> App {
    let mut app = app_with(
        name,
        vec![
            entry_in("a", "/w/one", HarnessId::Claude),
            entry_in("b", "/w/one", HarnessId::Claude),
        ],
    )
    .with_dismissed(Dismissed::load(file.to_path_buf()));
    app.apply(Update::Live(vec![live("a", 10)]));
    app.apply(Update::Activity(activity(ActivityState::Idle)));
    app
}

fn active_ids(app: &mut App) -> Vec<String> {
    app.switch_tab(2);
    let ids = app
        .rows
        .iter()
        .map(|&i| app.catalog.entries()[i].session_id.clone())
        .collect();
    app.switch_tab(-2);
    ids
}

fn bars(app: &mut App) -> usize {
    screen_lines(app, 100, 20).join("").matches(BAR).count()
}

fn file_ids(file: &PathBuf) -> String {
    std::fs::read_to_string(file).unwrap_or_default()
}

fn shows_status_text(app: &mut App) -> bool {
    screen_lines(app, 100, 20).join("").contains("idle")
}

fn dismiss(app: &mut App) {
    press(app, KeyCode::Char('d'));
    press(app, KeyCode::Char('y'));
}

fn select_a(app: &mut App) {
    app.select_path(Some(PathBuf::from("/p/a.jsonl")));
}

#[test]
fn dismissing_an_idle_live_session_removes_it_from_active_and_the_colour_bar() {
    let file = dismissed_file("idle");
    let mut app = idle_live_app("idle", &file);
    assert_eq!(active_ids(&mut app), ["a"]);
    assert!(bars(&mut app) > 0);
    assert!(shows_status_text(&mut app));
    select_a(&mut app);
    dismiss(&mut app);
    assert!(active_ids(&mut app).is_empty());
    assert_eq!(app.tabs[2].label, "Active");
    assert_eq!(bars(&mut app), 0);
    assert!(!shows_status_text(&mut app));
    press(&mut app, KeyCode::Char('D'));
    assert_eq!(active_ids(&mut app), ["a"]);
    assert!(bars(&mut app) > 0);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.focus, Focus::Conversation);
    dismiss(&mut app);
    assert!(active_ids(&mut app).is_empty());
}

#[test]
fn a_dismissed_session_stays_dismissed_when_it_becomes_busy_or_blocked() {
    for state in [ActivityState::Busy, ActivityState::Blocked] {
        let file = dismissed_file("stays");
        let mut app = idle_live_app("stays", &file);
        select_a(&mut app);
        dismiss(&mut app);
        app.apply(Update::Activity(activity(state)));
        assert!(active_ids(&mut app).is_empty());
        assert!(file_ids(&file).contains("\"a\""));
    }
}

#[test]
fn a_new_process_for_the_session_brings_it_back() {
    let file = dismissed_file("newpid");
    let mut app = idle_live_app("newpid", &file);
    select_a(&mut app);
    dismiss(&mut app);
    app.apply(Update::Live(vec![live("a", 11)]));
    assert_eq!(active_ids(&mut app), ["a"]);
}

#[test]
fn a_dismissal_whose_process_is_gone_is_removed_from_the_file() {
    let file = dismissed_file("gone");
    let mut app = idle_live_app("gone", &file);
    select_a(&mut app);
    dismiss(&mut app);
    assert!(file_ids(&file).contains("\"a\": 10"));
    app.apply(Update::Live(Vec::new()));
    app.apply(Update::Activity(ActivityMap::new()));
    assert!(!file_ids(&file).contains("\"a\""));
}

#[test]
fn a_restart_keeps_the_dismissal_while_the_same_process_lives() {
    let file = dismissed_file("restart");
    let mut app = idle_live_app("restart", &file);
    select_a(&mut app);
    dismiss(&mut app);
    drop(app);
    let mut restarted = idle_live_app("restart", &file);
    assert!(active_ids(&mut restarted).is_empty());
}

#[test]
fn the_tab_bar_ignores_d() {
    let file = dismissed_file("tabbar");
    let mut app = idle_live_app("tabbar", &file);
    select_a(&mut app);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.list_focus, ListFocus::Tabs);
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(active_ids(&mut app), ["a"]);
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "condition not met in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn launch_sleeper(app: &mut App, session_id: &str, seconds: u32) -> u64 {
    let spec = CommandSpec::new("sh").args(["-c", &format!("exec sleep {seconds}")]);
    let launched = Launched {
        title: "t".to_owned(),
        session_id: Some(session_id.to_owned()),
        harness: HarnessId::Claude,
    };
    app.sessions
        .launch(&spec, launched, Size { rows: 10, cols: 40 })
        .unwrap()
}

#[test]
fn d_on_a_hosted_session_stops_only_that_child_after_y_and_any_other_key_cancels() {
    let file = dismissed_file("hosted");
    let mut app = idle_live_app("hosted", &file);
    let run_dir = RuntimeDir::at(
        std::env::temp_dir().join(format!("ccview-stop-{}-hosted", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(run_dir.path());
    let daemon_dir = run_dir.clone();
    let daemon = std::thread::spawn(move || {
        daemon::run(
            &daemon_dir,
            &DaemonConfig {
                idle_exit: Duration::from_millis(300),
            },
        )
    });
    wait_until(|| run_dir.socket().exists());
    let client = HostClient::new(run_dir.clone());
    app.sessions = SessionManager::new(std::sync::mpsc::channel().0).with_host(client.clone());
    let stopped = launch_sleeper(&mut app, "a", 60);
    let bystander = launch_sleeper(&mut app, "b", 61);
    select_a(&mut app);
    app.focus = Focus::List;

    press(&mut app, KeyCode::Char('d'));
    assert!(app.confirm.is_some());
    press(&mut app, KeyCode::Char('n'));
    std::thread::sleep(Duration::from_millis(300));
    assert!(app.sessions.get(stopped).unwrap().is_running());
    assert_eq!(active_ids(&mut app), ["a"]);

    dismiss(&mut app);
    wait_until(|| !app.sessions.get(stopped).unwrap().is_running());
    assert!(app.sessions.get(bystander).unwrap().is_running());
    assert!(active_ids(&mut app).is_empty());
    assert!(client.list().unwrap().len() == 2);

    app.sessions.stop(bystander).unwrap();
    wait_until(|| !app.sessions.get(bystander).unwrap().is_running());
    app.sessions.remove(stopped);
    app.sessions.remove(bystander);
    drop(app);
    daemon.join().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(run_dir.path());
}

fn own_sleeper() -> Child {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}

fn register(dir: &Path, pid: u32, session_id: &str, proc_start: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let json = format!(
        r#"{{"pid":{pid},"sessionId":"{session_id}","procStart":"{proc_start}","status":"idle"}}"#
    );
    std::fs::write(dir.join(format!("{pid}.json")), json).unwrap();
}

#[test]
fn d_on_a_session_in_another_terminal_stops_its_pid_only_while_the_registry_still_names_it() {
    let mut children = [own_sleeper(), own_sleeper()];
    let file = dismissed_file("registry");
    let sessions_dir = file.parent().unwrap().join("sessions");
    let starts = children
        .each_ref()
        .map(|child| crate::sys::process::process_start(child.id()).unwrap());
    let other_start = "Mon Jan  1 00:00:00 2001".to_owned();
    for (index, registered_as, started, expect_stopped) in [
        (0, "someone-else", starts[0].clone(), false),
        (1, "a", other_start, false),
        (1, "a", starts[1].clone(), true),
    ] {
        let child = &mut children[index];
        let mut app = idle_live_app("registry", &file);
        app.claude_sessions_dir = Some(sessions_dir.clone());
        app.apply(Update::Live(vec![live("a", child.id())]));
        register(&sessions_dir, child.id(), registered_as, &started);
        select_a(&mut app);
        dismiss(&mut app);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(child.try_wait().unwrap().is_some(), expect_stopped);
        app.catalog.undismiss("a").unwrap();
    }
    for child in &mut children {
        let _ = child.kill();
        child.wait().unwrap();
    }
}
