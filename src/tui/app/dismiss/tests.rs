use crate::tui::embedded::manager::Launched;
use crate::tui::{app::tests::support::*, *};
use crate::{
    session::{
        activity::{Activity, ActivityMap, ActivityState},
        live::{LiveSession, Status},
    },
    terminal::{CommandSpec, Size},
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

fn idle_live_app(name: &str, file: &PathBuf) -> App {
    let mut app = app_with(
        name,
        vec![
            entry_in("a", "/w/one", HarnessId::Claude),
            entry_in("b", "/w/one", HarnessId::Claude),
        ],
    )
    .with_dismissed(Dismissed::load(file.clone()));
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
    press(&mut app, KeyCode::Char('d'));
    assert!(active_ids(&mut app).is_empty());
    assert_eq!(app.tabs[2].label, "Active");
    assert_eq!(bars(&mut app), 0);
    assert!(!shows_status_text(&mut app));
    press(&mut app, KeyCode::Char('D'));
    assert_eq!(active_ids(&mut app), ["a"]);
    assert!(bars(&mut app) > 0);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.focus, Focus::Conversation);
    press(&mut app, KeyCode::Char('d'));
    assert!(active_ids(&mut app).is_empty());
}

#[test]
fn a_dismissed_session_returns_when_it_becomes_busy_or_blocked() {
    for state in [ActivityState::Busy, ActivityState::Blocked] {
        let file = dismissed_file("returns");
        let mut app = idle_live_app("returns", &file);
        select_a(&mut app);
        press(&mut app, KeyCode::Char('d'));
        assert!(active_ids(&mut app).is_empty());
        app.apply(Update::Activity(activity(state)));
        assert_eq!(active_ids(&mut app), ["a"]);
        assert!(!file_ids(&file).contains("\"a\""));
    }
}

#[test]
fn a_new_process_for_the_session_brings_it_back() {
    let file = dismissed_file("newpid");
    let mut app = idle_live_app("newpid", &file);
    select_a(&mut app);
    press(&mut app, KeyCode::Char('d'));
    app.apply(Update::Live(vec![live("a", 11)]));
    assert_eq!(active_ids(&mut app), ["a"]);
}

#[test]
fn a_dismissal_whose_process_is_gone_is_removed_from_the_file() {
    let file = dismissed_file("gone");
    let mut app = idle_live_app("gone", &file);
    select_a(&mut app);
    press(&mut app, KeyCode::Char('d'));
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
    press(&mut app, KeyCode::Char('d'));
    drop(app);
    let mut restarted = idle_live_app("restart", &file);
    assert!(active_ids(&mut restarted).is_empty());
}

#[test]
fn d_on_a_row_running_in_an_embedded_pane_does_nothing() {
    let file = dismissed_file("pane");
    let mut app = idle_live_app("pane", &file);
    let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
    let launched = Launched {
        title: "t".to_owned(),
        session_id: Some("a".to_owned()),
        harness: HarnessId::Claude,
    };
    let id = app
        .sessions
        .launch(&spec, launched, Size { rows: 10, cols: 40 })
        .unwrap();
    select_a(&mut app);
    app.focus = Focus::List;
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.notice.as_deref(), Some("running in a pane"));
    assert_eq!(active_ids(&mut app), ["a"]);
    app.sessions.remove(id);
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
