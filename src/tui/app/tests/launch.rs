use super::support::*;
use crate::catalog::feed::Update;
use crate::harness::ClaudeCode;
use crate::session::entry::SessionEntry;
use crate::session::harness_id::HarnessId;
use crate::session::live::{LiveSession, Status};
use crate::terminal::{CommandSpec, Size};
use crate::tui::app::*;
use crate::tui::embedded::manager::Launched;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn pressing_continue_twice_in_one_directory_attaches_the_running_session_instead_of_starting_another()
 {
    let dir = std::env::temp_dir().join(format!("ccview-continue-twice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut app = app_with(
        "continue-twice",
        vec![entry_in("a", dir.to_str().unwrap(), HarnessId::Claude)],
    );
    app.harness = Box::new(ClaudeCode::with_binary(fake.to_str().unwrap()));
    app.select_path(Some(PathBuf::from("/p/a.jsonl")));

    press(&mut app, KeyCode::Char('c'));
    assert_eq!(app.sessions.all().len(), 1);
    let first = app.sessions.all()[0].id;

    app.focus = Focus::List;
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(app.sessions.all().len(), 1);
    assert_eq!(app.shown, Some(first));

    app.sessions.remove(first);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resume_in_a_directory_that_does_not_exist_starts_nothing_and_says_so() {
    let missing = std::env::temp_dir().join(format!("ccview-no-such-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let marker =
        std::env::temp_dir().join(format!("ccview-no-such-dir-ran-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let fake = std::env::temp_dir().join(format!("ccview-no-such-dir-bin-{}", std::process::id()));
    std::fs::write(&fake, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut app = app_with(
        "resume-missing-dir",
        vec![entry_in("a", missing.to_str().unwrap(), HarnessId::Claude)],
    );
    app.harness = Box::new(ClaudeCode::with_binary(fake.to_str().unwrap()));
    app.select_path(Some(PathBuf::from("/p/a.jsonl")));

    press(&mut app, KeyCode::Char('r'));

    assert_eq!(
        app.notice.as_deref(),
        Some(format!("directory not found: {}", missing.display()).as_str())
    );
    assert!(app.sessions.all().is_empty());
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!marker.exists());
    let _ = std::fs::remove_file(&fake);
}

fn lineage_entry(id: &str, cwd: &str, age_secs: u64, uuids: std::ops::Range<u32>) -> SessionEntry {
    SessionEntry {
        last_used: std::time::UNIX_EPOCH + Duration::from_secs(1_000 + age_secs),
        fingerprint: Some(std::sync::Arc::new(
            crate::session::lineage::fingerprint_of(uuids),
        )),
        ..entry_in(id, cwd, HarnessId::Claude)
    }
}

fn lineage_app(name: &str) -> (App, PathBuf) {
    let dir = temp_dir(name);
    let fake = dir.join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cwd = dir.to_str().unwrap();
    let mut app = app_with(
        name,
        vec![
            lineage_entry("old", cwd, 0, 0..10),
            lineage_entry("new", cwd, 10, 0..20),
        ],
    );
    app.harness = Box::new(ClaudeCode::with_binary(fake.to_str().unwrap()));
    (app, dir)
}

fn live_for_pid(pid: u32, session_id: &str) -> LiveSession {
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

fn sleeper_bound_to(app: &mut App, session_id: &str) -> u64 {
    let spec = CommandSpec::new("sh").args(["-c", "exec sleep 30"]);
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
fn r_on_the_visible_row_reattaches_a_pane_bound_to_an_older_copy_of_the_conversation() {
    let (mut app, dir) = lineage_app("dup-collapsed");
    let pane = sleeper_bound_to(&mut app, "old");
    app.select_path(Some(PathBuf::from("/p/new.jsonl")));
    assert_eq!(app.selected_entry().unwrap().copies, ["old"]);

    press(&mut app, KeyCode::Char('r'));

    assert_eq!(app.sessions.all().len(), 1);
    assert_eq!(app.shown, Some(pane));
    app.sessions.remove(pane);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn r_twice_on_an_expanded_older_row_ends_with_one_pane_even_after_the_pane_rebinds_to_the_new_id() {
    let (mut app, dir) = lineage_app("dup-expanded");
    app.catalog.set_expanded(true);
    app.select_path(Some(PathBuf::from("/p/old.jsonl")));

    press(&mut app, KeyCode::Char('r'));
    let pane = app.sessions.all()[0].id;
    let pid = app.sessions.get(pane).unwrap().host().process_id().unwrap();
    app.apply(Update::Live(vec![live_for_pid(pid, "new")]));
    assert_eq!(
        app.sessions.get(pane).unwrap().session_id.as_deref(),
        Some("new")
    );

    app.focus = Focus::List;
    press(&mut app, KeyCode::Char('r'));

    assert_eq!(app.sessions.all().len(), 1);
    assert_eq!(app.shown, Some(pane));
    app.sessions.remove(pane);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn r_attaches_a_host_session_bound_to_another_member_of_the_lineage() {
    use crate::terminal::session_host::{
        client::HostClient,
        daemon::{self, DaemonConfig},
        runtime_dir::RuntimeDir,
    };
    use crate::tui::embedded::manager::SessionManager;
    let (mut app, dir) = lineage_app("dup-host");
    let run_dir = RuntimeDir::at(dir.join("run"));
    let daemon_dir = run_dir.clone();
    let daemon = std::thread::spawn(move || {
        daemon::run(
            &daemon_dir,
            &DaemonConfig {
                idle_exit: Duration::from_millis(300),
                ..DaemonConfig::default()
            },
        )
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !run_dir.socket().exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let client = HostClient::new(run_dir.clone());
    app.sessions = SessionManager::new(std::sync::mpsc::channel().0).with_host(client.clone());
    let pane = sleeper_bound_to(&mut app, "old");
    let daemon_id = client.list().unwrap()[0].id;
    app.sessions.remove(pane);
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .any(|info| info.id == daemon_id && info.exit.is_none())
    );
    app.select_path(Some(PathBuf::from("/p/new.jsonl")));
    app.focus = Focus::List;

    press(&mut app, KeyCode::Char('r'));

    assert_eq!(app.sessions.all().len(), 1);
    assert_eq!(client.list().unwrap().len(), 1);
    client.stop(daemon_id).unwrap();
    app.sessions.remove(app.sessions.all()[0].id);
    drop(app);
    daemon.join().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
