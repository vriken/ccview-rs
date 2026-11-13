use super::support::*;
use crate::tui::app::*;
use crate::{catalog::feed::Update, store::favorites::Favorites, tui::style::identity::Identity};
use std::sync::mpsc;

#[test]
fn entries_queued_before_the_app_exists_are_on_the_first_frame() {
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Update::Scanned(vec![entry("cached-session", "/w/one")]))
        .unwrap();
    let mut app = App::new(
        Favorites::load(
            std::env::temp_dir().join(format!("ccview-first-{}/f.json", std::process::id())),
        ),
        Identity::load(
            std::env::temp_dir().join(format!("ccview-first-{}/c.json", std::process::id())),
        ),
        Some(receiver),
        mpsc::channel().0,
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("cached-session"));
}

#[test]
fn an_idle_session_with_handoffs_owed_to_the_owner_says_so_ranks_with_asks_you_and_yields_to_on_hold()
 {
    use crate::{
        session::activity::{Activity, ActivityMap, ActivityState},
        sources::claude::dod::DodCounts,
        tui::widgets::tabs::TabKey,
    };
    use std::collections::HashMap;

    let mut app = app_with(
        "owed",
        vec![
            entry("a-busy", "/w"),
            entry("b-plain", "/w"),
            entry("c-owing", "/w"),
            entry("d-held", "/w"),
        ],
    );
    let held = app.catalog.entries()[3].clone();
    app.on_hold.toggle_entry(&held).unwrap();
    let activity: ActivityMap = [
        ("a-busy", ActivityState::Busy),
        ("b-plain", ActivityState::Idle),
        ("c-owing", ActivityState::Idle),
        ("d-held", ActivityState::Idle),
    ]
    .into_iter()
    .map(|(id, state)| (id.to_owned(), Activity::new(state)))
    .collect();
    app.apply(Update::Activity(activity));
    let owed = DodCounts {
        owed: 2,
        open: 0,
        modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
    };
    app.apply(Update::Dod(HashMap::from([
        ("c-owing".to_owned(), owed),
        ("d-held".to_owned(), owed),
    ])));
    app.active = TabKey::Active;
    app.rebuild_view();

    let order: Vec<&str> = app
        .rows
        .iter()
        .map(|&row| app.catalog.entries()[row].session_id.as_str())
        .collect();
    assert_eq!(order, ["c-owing", "a-busy", "b-plain", "d-held"]);
    let screen = screen_lines(&mut app, 100, 20).join("\n");
    assert_eq!(screen.matches("2 for you").count(), 1);
    assert!(screen.contains("on hold"));
}
