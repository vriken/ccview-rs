use super::support::*;
use crate::harness::ClaudeCode;
use crate::session::harness_id::HarnessId;
use crate::tui::app::*;
use std::os::unix::fs::PermissionsExt;

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
