use super::support::*;
use crate::{
    session::entry::TranscriptHandle, store::ui_state::UiState, tui::app::*,
    tui::widgets::tabs::TabKey,
};
use std::time::{Duration, Instant};

fn long_transcript_entry(dir: &std::path::Path) -> crate::session::entry::SessionEntry {
    let path = dir.join("b.jsonl");
    let lines: Vec<String> = (0..120)
        .map(|n| {
            format!(
                r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"line number {n}"}}]}}}}"#
            )
        })
        .collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let mut session = entry("b", "/w/two");
    session.path = path.clone();
    session.handle = TranscriptHandle::Rollout(path);
    session
}

fn settle(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(Instant::now() < deadline, "never settled");
        std::thread::sleep(Duration::from_millis(20));
        app.tick();
        screen_lines(app, 100, 30);
    }
}

#[test]
fn a_restarted_ui_comes_back_on_the_same_tab_session_and_scroll_position() {
    let dir = temp_dir("restart-roundtrip");
    let entries = || {
        vec![
            entry("a", "/w/one"),
            long_transcript_entry(&dir),
            entry("c", "/w/two"),
        ]
    };
    let mut before = app_with("restart-before", entries());
    before.active = TabKey::Dir("/w/two".to_owned());
    before.refresh_rows();
    let selected = before.selected_path.clone();
    before.select_path(Some(dir.join("b.jsonl")));
    assert_ne!(selected, before.selected_path);
    before.focus = Focus::Conversation;
    before.show_hidden = true;
    before.catalog.set_expanded(true);
    settle(&mut before, |app| app.max_scroll > 0);
    before.scroll_up(40);
    screen_lines(&mut before, 100, 30);
    let expected_scroll = before.scroll;
    assert!(expected_scroll > 0 && !before.follow_bottom);

    let state_path = dir.join("ui_state.json");
    before.capture_ui_state().save(&state_path).unwrap();
    let restored = UiState::take(&state_path, crate::store::ui_state::now_ms());
    assert!(restored.is_some() && !state_path.exists());

    let mut after = app_with("restart-after", entries()).with_restored_state(restored);
    settle(&mut after, |app| {
        app.pending_restore.is_none() && app.pending_scroll.is_none()
    });
    screen_lines(&mut after, 100, 30);
    assert_eq!(after.active, TabKey::Dir("/w/two".to_owned()));
    assert_eq!(after.selected_entry().unwrap().session_id, "b");
    assert_eq!(after.focus, Focus::Conversation);
    assert!(!after.follow_bottom);
    assert_eq!(after.scroll, expected_scroll);
    assert!(after.show_hidden && after.catalog.expanded());
}
