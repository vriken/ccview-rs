use super::tests::{Projects, scan_one};
use super::*;
use crate::catalog::scanner::Scanner;
use crate::session::{activity::Activity, lineage::fingerprint_of, live::Status};
use std::fs;
use std::{
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

fn entry(id: &str, age_secs: u64, uuids: std::ops::Range<u32>) -> SessionEntry {
    SessionEntry {
        path: PathBuf::from(format!("/p/{id}.jsonl")),
        session_id: id.to_owned(),
        last_used: UNIX_EPOCH + Duration::from_secs(1_000 + age_secs),
        fingerprint: Some(Arc::new(fingerprint_of(uuids))),
        ..SessionEntry::stub()
    }
}

fn live(session_id: &str, status: Status) -> LiveSession {
    LiveSession {
        pid: 1,
        pid_alive: true,
        session_id: Some(session_id.to_owned()),
        cwd: None,
        started_at_ms: None,
        updated_at_ms: Some(5_000),
        status_updated_at_ms: None,
        status,
        name: None,
        name_source: None,
        version: None,
        kind: None,
        entrypoint: None,
        transcript: None,
    }
}

fn chain() -> Catalog {
    let mut catalog = Catalog::default();
    catalog.merge(vec![
        entry("old", 1, 0..600),
        entry("new", 2, 0..1200),
        entry("other", 3, 5000..5600),
    ]);
    catalog
}

fn listed(catalog: &Catalog) -> Vec<&str> {
    catalog
        .entries()
        .iter()
        .filter(|entry| !catalog.is_collapsed_copy(entry))
        .map(|entry| entry.session_id.as_str())
        .collect()
}

#[test]
fn a_resumed_copy_collapses_under_the_newest_and_expanding_lists_both() {
    let mut catalog = chain();
    assert_eq!(listed(&catalog), ["other", "new"]);
    let newest = catalog
        .entries()
        .iter()
        .find(|entry| entry.session_id == "new")
        .unwrap();
    assert_eq!(newest.copies, ["old"]);

    catalog.set_expanded(true);
    assert_eq!(listed(&catalog), ["other", "new", "old"]);
    let old = catalog
        .entries()
        .iter()
        .find(|entry| entry.session_id == "old")
        .unwrap();
    assert_eq!(old.copy_of.as_deref(), Some("new"));
    assert!(
        catalog
            .entries()
            .iter()
            .all(|entry| entry.copies.is_empty())
    );
}

#[test]
fn activity_and_liveness_of_an_older_copy_show_on_the_visible_row() {
    let mut catalog = chain();
    let running = live("old", Status::Busy);
    catalog.set_activity(crate::sources::claude::activity::merge(
        std::slice::from_ref(&running),
        &[],
    ));
    catalog.set_live(vec![running]);

    assert!(
        catalog
            .activity()
            .get("new")
            .is_some_and(Activity::is_active)
    );
    assert!(!catalog.activity().contains_key("old"));
    let newest = catalog
        .entries()
        .iter()
        .find(|entry| entry.session_id == "new")
        .unwrap();
    assert!(newest.live.as_ref().is_some_and(|live| live.pid_alive));

    catalog.set_expanded(true);
    assert!(catalog.activity().contains_key("old") && !catalog.activity().contains_key("new"));
}

#[test]
fn a_star_on_an_older_copy_stars_the_collapsed_row_and_unstarring_clears_the_lineage() {
    let dir = std::env::temp_dir().join(format!("ccview-lineage-fav-{}", std::process::id()));
    let mut favorites = crate::store::favorites::Favorites::load(dir.join("favorites.json"));
    favorites.toggle("old").unwrap();
    let catalog = chain();
    let newest = catalog
        .entries()
        .iter()
        .find(|entry| entry.session_id == "new")
        .unwrap();
    assert!(favorites.contains_entry(newest));
    favorites.toggle_entry(newest).unwrap();
    assert!(!favorites.contains("old") && !favorites.contains("new"));
    favorites.toggle_entry(newest).unwrap();
    assert!(favorites.contains("new") && !favorites.contains("old"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn scanned_files_that_share_history_collapse_and_appends_keep_the_fingerprint_exact() {
    let projects = Projects::new();
    let line = |n: u32| {
        format!(
            r#"{{"parentUuid":null,"type":"user","message":{{"content":"hello"}},"uuid":"{n:08}-aaaa"}}"#
        )
    };
    let history: String = (0..400).map(|n| format!("{}\n", line(n))).collect();
    projects.append("first", &history);
    projects.append("second", &history);
    projects.append(
        "second",
        &(400..500)
            .map(|n| format!("{}\n", line(n)))
            .collect::<String>(),
    );
    let mut scanner = Scanner::default();
    let first = scan_one(&mut scanner, &projects, "first");
    let second = scan_one(&mut scanner, &projects, "second");
    let mut catalog = Catalog::default();
    catalog.merge(vec![first, second]);
    assert_eq!(listed(&catalog).len(), 1);

    projects.append(
        "second",
        &(500..600)
            .map(|n| format!("{}\n", line(n)))
            .collect::<String>(),
    );
    let incremental = scan_one(&mut scanner, &projects, "second");
    let fresh = scan_one(&mut Scanner::default(), &projects, "second");
    assert_eq!(incremental.fingerprint, fresh.fingerprint);
    assert_eq!(fresh.fingerprint.unwrap().count, 600);
}
