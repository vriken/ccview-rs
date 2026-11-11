use crate::session::{entry::SessionEntry, live::LiveSession};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub(crate) type CheckoutKeys = HashMap<String, PathBuf>;

pub(crate) fn checkout_of(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

#[derive(Default)]
pub(crate) struct CheckoutResolver {
    by_cwd: HashMap<String, PathBuf>,
}

impl CheckoutResolver {
    pub(crate) fn resolve(&mut self, live: &[LiveSession]) -> CheckoutKeys {
        let running: Vec<(&String, &String)> = live
            .iter()
            .filter(|session| session.pid_alive)
            .filter_map(|session| Some((session.session_id.as_ref()?, session.cwd.as_ref()?)))
            .collect();
        self.by_cwd
            .retain(|cwd, _| running.iter().any(|(_, running)| *running == cwd));
        running
            .into_iter()
            .map(|(session_id, cwd)| {
                let key = self
                    .by_cwd
                    .entry(cwd.clone())
                    .or_insert_with(|| checkout_of(Path::new(cwd)));
                (session_id.clone(), key.clone())
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedCheckout {
    pub(crate) first_other: String,
    pub(crate) more: usize,
}

#[derive(Default)]
pub(crate) struct SharedCheckouts(HashMap<String, SharedCheckout>);

impl SharedCheckouts {
    pub(crate) fn for_entry(&self, entry: &SessionEntry) -> Option<&SharedCheckout> {
        std::iter::once(&entry.session_id)
            .chain(&entry.copies)
            .find_map(|id| self.0.get(id))
    }

    pub(crate) fn compute(
        entries: &[SessionEntry],
        keys: &CheckoutKeys,
        is_running: impl Fn(&str) -> bool,
    ) -> Self {
        struct Candidate<'a> {
            entry: &'a SessionEntry,
            lineage: &'a str,
            key: &'a PathBuf,
        }
        let candidates: Vec<Candidate> = entries
            .iter()
            .filter(|entry| is_running(&entry.session_id))
            .filter_map(|entry| {
                Some(Candidate {
                    entry,
                    lineage: entry.copy_of.as_deref().unwrap_or(&entry.session_id),
                    key: keys.get(&entry.session_id)?,
                })
            })
            .collect();
        let mut shared = HashMap::new();
        for candidate in &candidates {
            let others: Vec<&Candidate> = candidates
                .iter()
                .filter(|other| other.key == candidate.key && other.lineage != candidate.lineage)
                .collect();
            if let Some(first) = others.first() {
                shared.insert(
                    candidate.entry.session_id.clone(),
                    SharedCheckout {
                        first_other: first.entry.title.clone(),
                        more: others.len() - 1,
                    },
                );
            }
        }
        Self(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::Catalog,
        session::{lineage::fingerprint_of, live::Status},
    };
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, UNIX_EPOCH},
    };

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "ccview-checkout-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn dir(&self, relative: &str) -> String {
            let dir = self.0.join(relative);
            fs::create_dir_all(&dir).unwrap();
            dir.to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(id: &str, age: u64, uuids: std::ops::Range<u32>) -> SessionEntry {
        SessionEntry {
            path: PathBuf::from(format!("/p/{id}.jsonl")),
            session_id: id.to_owned(),
            title: format!("title {id}"),
            last_used: UNIX_EPOCH + Duration::from_secs(1_000 + age),
            fingerprint: Some(Arc::new(fingerprint_of(uuids))),
            ..SessionEntry::stub()
        }
    }

    fn live_in(id: &str, cwd: &str) -> LiveSession {
        LiveSession {
            pid: 1,
            pid_alive: true,
            session_id: Some(id.to_owned()),
            cwd: Some(cwd.to_owned()),
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

    #[test]
    fn only_live_sessions_of_different_conversations_in_one_checkout_are_flagged() {
        let scratch = Scratch::new();
        let main = scratch.dir("main");
        fs::create_dir(scratch.0.join("main/.git")).unwrap();
        let worktree = scratch.dir("main/trees/wt");
        fs::write(
            scratch.0.join("main/trees/wt/.git"),
            "gitdir: ../main/.git/worktrees/wt",
        )
        .unwrap();
        let solo = scratch.dir("solo");
        fs::create_dir(scratch.0.join("solo/.git")).unwrap();
        let quiet = scratch.dir("quiet");
        fs::create_dir(scratch.0.join("quiet/.git")).unwrap();
        let mut ended = live_in("gone", &quiet);
        ended.pid_alive = false;
        let mut catalog = Catalog::default();
        catalog.merge(vec![
            entry("lone", 7, 40_000..40_600),
            entry("idle", 8, 50_000..50_600),
            entry("a", 1, 0..600),
            entry("b", 2, 5000..5600),
            entry("wt", 3, 9000..9600),
            entry("gone", 4, 20_000..20_600),
            entry("old", 5, 30_000..30_600),
            entry("new", 6, 30_000..31_200),
        ]);
        let sessions = [
            live_in("a", &format!("{main}/src")),
            live_in("b", &scratch.dir("main/docs")),
            live_in("wt", &worktree),
            live_in("old", &solo),
            live_in("new", &solo),
            live_in("lone", &quiet),
            live_in("idle", &format!("{main}/src")),
            ended,
        ];
        catalog.set_live(sessions.to_vec());
        catalog.set_activity(
            ["a", "b", "wt", "lone", "old", "new"]
                .into_iter()
                .map(|id| {
                    (
                        id.to_owned(),
                        crate::session::activity::Activity::new(
                            crate::session::activity::ActivityState::Busy,
                        ),
                    )
                })
                .collect(),
        );
        catalog.set_checkouts(CheckoutResolver::default().resolve(&sessions));
        let shared = |id: &str| {
            let entry = catalog
                .entries()
                .iter()
                .find(|entry| entry.session_id == id)
                .unwrap();
            catalog.shared_checkouts().for_entry(entry).cloned()
        };
        assert_eq!(
            shared("a"),
            Some(SharedCheckout {
                first_other: "title b".to_owned(),
                more: 0
            })
        );
        assert!(shared("b").is_some());
        assert_eq!(shared("wt"), None);
        assert_eq!(shared("lone"), None);
        assert_eq!(shared("idle"), None);
        assert_eq!(shared("new"), None);
    }
}
