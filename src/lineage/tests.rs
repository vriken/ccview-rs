use super::*;
use std::time::{Duration, UNIX_EPOCH};

struct Sessions {
    paths: Vec<PathBuf>,
    prints: Vec<Fingerprint>,
}

impl Sessions {
    fn new(uuid_sets: Vec<Vec<u32>>) -> Self {
        Self {
            paths: (0..uuid_sets.len()).map(|n| PathBuf::from(format!("/p/{n}.jsonl"))).collect(),
            prints: uuid_sets.into_iter().map(fingerprint_of).collect(),
        }
    }

    fn groups(&self) -> Vec<Group> {
        let members: Vec<Member> = self
            .paths
            .iter()
            .zip(&self.prints)
            .enumerate()
            .map(|(n, (path, print))| Member {
                path,
                last_used: UNIX_EPOCH + Duration::from_secs(n as u64),
                fingerprint: Some(print),
            })
            .collect();
        group(&members)
    }

    fn collapsed(&self) -> Vec<(usize, Vec<usize>)> {
        self.groups().into_iter().map(|group| (group.visible, group.copies)).collect()
    }
}

fn run(range: std::ops::Range<u32>) -> Vec<u32> {
    range.collect()
}

fn joined(parts: &[std::ops::Range<u32>]) -> Vec<u32> {
    parts.iter().cloned().flatten().collect()
}

#[test]
fn a_continuation_chain_is_one_row_showing_the_newest_copy() {
    let sessions = Sessions::new(vec![run(0..800), run(0..1500), run(0..2400)]);
    assert_eq!(sessions.collapsed(), vec![(2, vec![1, 0])]);
}

#[test]
fn a_fork_keeps_both_branches_and_hides_only_the_shared_ancestor() {
    let sessions = Sessions::new(vec![run(0..800), joined(&[0..800, 1000..1800]), joined(&[0..800, 2000..2800])]);
    assert_eq!(relation(&sessions.prints[1], &sessions.prints[2]), Relation::Fork);
    assert_eq!(sessions.collapsed(), vec![(1, vec![]), (2, vec![0])]);
}

#[test]
fn unrelated_sessions_stay_separate() {
    let sessions = Sessions::new(vec![run(0..600), run(10_000..10_600), run(50_000..50_020)]);
    assert_eq!(sessions.collapsed(), vec![(0, vec![]), (1, vec![]), (2, vec![])]);
}

#[test]
fn an_ancestor_that_kept_growing_is_a_fork_unless_the_tail_is_small() {
    let grown_a_lot = Sessions::new(vec![run(0..1000), joined(&[0..800, 5000..5800])]);
    assert_eq!(grown_a_lot.collapsed(), vec![(0, vec![]), (1, vec![])]);

    let grown_a_little = Sessions::new(vec![run(0..1000), joined(&[0..970, 5000..5800])]);
    assert_eq!(grown_a_little.collapsed(), vec![(1, vec![0])]);
}

#[test]
fn identical_copies_collapse_under_the_most_recent() {
    let sessions = Sessions::new(vec![run(0..500), run(0..500)]);
    assert_eq!(sessions.collapsed(), vec![(1, vec![0])]);
}

#[test]
fn sessions_with_too_few_uuids_are_never_merged() {
    let sessions = Sessions::new(vec![run(0..4), run(0..4)]);
    assert_eq!(sessions.collapsed(), vec![(0, vec![]), (1, vec![])]);
}

#[test]
fn appending_lines_one_by_one_matches_building_from_all_of_them() {
    let all = fingerprint_of(0..3000);
    let mut incremental = fingerprint_of(0..1200);
    for uuid in 1200..3000u32 {
        let line = format!(r#"{{"parentUuid":"x","type":"user","uuid":"{uuid:08}-0000-4000-8000-000000000000"}}"#);
        incremental.add_line(line.as_bytes());
    }
    assert_eq!(incremental, all);
    assert_eq!(all.count, 3000);
}

#[test]
fn fingerprints_survive_a_json_round_trip_and_reject_garbage() {
    let print = fingerprint_of(0..500);
    let text = serde_json::to_string(&print).unwrap();
    assert_eq!(serde_json::from_str::<Fingerprint>(&text).unwrap(), print);
    assert!(serde_json::from_str::<Fingerprint>(r#"{"count":1,"sketch":"zz"}"#).is_err());
}

#[test]
fn only_message_records_count_and_a_nested_uuid_key_does_not_confuse_the_last_field() {
    let mut print = Fingerprint::default();
    print.add_line(br#"{"type":"ai-title","uuid":"not-a-message"}"#);
    print.add_line(br#"{"parentUuid":null,"message":{"content":[{"input":{"uuid":"nested"}}]},"uuid":"real-1","timestamp":"t"}"#);
    let mut expected = Fingerprint::default();
    expected.add_uuid(b"real-1");
    assert_eq!(print, expected);
}
