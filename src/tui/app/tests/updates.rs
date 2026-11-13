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
        modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
        ..DodCounts::default()
    };
    app.apply(Update::Dod(HashMap::from([
        ("c-owing".to_owned(), owed.clone()),
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

#[test]
fn an_idle_session_shows_the_highest_precedence_state_its_definition_of_done_supports() {
    use crate::{
        session::activity::{Activity, ActivityMap, ActivityState},
        sources::claude::dod::DodCounts,
        tui::widgets::tabs::TabKey,
    };
    use std::collections::HashMap;

    let counts = |owed, open, file_open, waits, label: &str| DodCounts {
        owed,
        open,
        file_open,
        waits,
        last_wait: (waits > 0).then(|| label.to_owned()),
        modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
    };
    type Case<'a> = (&'a str, bool, Option<DodCounts>, &'a str, &'a [&'a str]);
    let cases: Vec<Case> = vec![
        (
            "h-asks",
            true,
            Some(counts(2, 1, 4, 1, "vendor")),
            "asks you",
            &["waiting on", "stalled", "done"],
        ),
        (
            "g-foryou",
            false,
            Some(counts(2, 1, 4, 1, "vendor")),
            "2 for you",
            &["waiting on", "stalled", "done"],
        ),
        (
            "f-stalled",
            false,
            Some(counts(0, 3, 3, 0, "")),
            "stalled · 3 open",
            &["waiting on", "done"],
        ),
        ("d-idle", false, None, "idle", &["done", "stalled"]),
        (
            "e-idle-self",
            false,
            Some(counts(0, 0, 1, 0, "")),
            "idle",
            &["done", "stalled", "waiting on"],
        ),
        (
            "d-stale-idle",
            false,
            Some(counts(0, 0, 5, 0, "")),
            "idle",
            &["done", "stalled", "waiting on"],
        ),
        (
            "c-stale-wait",
            false,
            Some(counts(0, 0, 5, 1, "the data")),
            "waiting on the data",
            &["done", "stalled", "idle"],
        ),
        (
            "c-waiting",
            false,
            Some(counts(0, 1, 3, 2, "vendor reply")),
            "waiting on vendor reply · +1",
            &["stalled", "done"],
        ),
        (
            "b-done",
            false,
            Some(counts(0, 0, 0, 0, "")),
            "done",
            &["idle", "stalled", "waiting on"],
        ),
        (
            "a-held",
            true,
            Some(counts(2, 1, 4, 1, "vendor")),
            "on hold",
            &["asks you", "for you", "waiting on", "stalled"],
        ),
    ];
    let entries = cases
        .iter()
        .map(|(id, asks, ..)| {
            let mut entry = entry(id, "/w");
            entry.asks_user = *asks;
            entry
        })
        .collect();
    let mut app = app_with("idle-states", entries);
    let held = app
        .catalog
        .entries()
        .iter()
        .find(|entry| entry.session_id == "a-held")
        .unwrap()
        .clone();
    app.on_hold.toggle_entry(&held).unwrap();
    let activity: ActivityMap = cases
        .iter()
        .map(|(id, ..)| ((*id).to_owned(), Activity::new(ActivityState::Idle)))
        .collect();
    app.apply(Update::Activity(activity));
    app.apply(Update::Dod(
        cases
            .iter()
            .filter_map(|(id, _, dod, ..)| dod.clone().map(|dod| ((*id).to_owned(), dod)))
            .collect::<HashMap<_, _>>(),
    ));
    app.active = TabKey::Active;
    app.rebuild_view();

    let order: Vec<&str> = app
        .rows
        .iter()
        .map(|&row| app.catalog.entries()[row].session_id.as_str())
        .collect();
    assert_eq!(
        order,
        [
            "f-stalled",
            "g-foryou",
            "h-asks",
            "d-idle",
            "d-stale-idle",
            "e-idle-self",
            "c-stale-wait",
            "c-waiting",
            "b-done",
            "a-held",
        ]
    );
    let screen = screen_lines(&mut app, 200, 40);
    for (id, _, _, word, absent) in &cases {
        let title = screen
            .iter()
            .position(|line| line.contains(id) && line.contains('●'))
            .unwrap();
        let detail = &screen[title + 1];
        assert!(detail.contains(word), "{id}: {detail}");
        for other in *absent {
            assert!(!detail.contains(other), "{id}: {detail}");
        }
    }
}
