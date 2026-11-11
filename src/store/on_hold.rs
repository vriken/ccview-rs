use crate::{
    session::entry::SessionEntry,
    store::{config_file, write_atomically},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize)]
struct Hold {
    since_ms: u64,
}

#[derive(Default)]
pub(crate) struct OnHoldSessions {
    path: Option<PathBuf>,
    holds: BTreeMap<String, Hold>,
}

impl OnHoldSessions {
    pub(crate) fn load_default() -> Self {
        Self::load(config_file("on_hold.json"))
    }

    pub(crate) fn load(path: PathBuf) -> Self {
        let holds = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            holds,
        }
    }

    pub(crate) fn contains(&self, session_id: &str) -> bool {
        self.holds.contains_key(session_id)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = &String> {
        self.holds.keys()
    }

    pub(crate) fn contains_entry(&self, entry: &SessionEntry) -> bool {
        self.contains(&entry.session_id) || entry.copies.iter().any(|id| self.contains(id))
    }

    pub(crate) fn toggle_entry(&mut self, entry: &SessionEntry) -> io::Result<()> {
        if self.contains_entry(entry) {
            for id in std::iter::once(&entry.session_id).chain(&entry.copies) {
                self.holds.remove(id);
            }
        } else {
            self.holds
                .insert(entry.session_id.clone(), Hold { since_ms: now_ms() });
        }
        self.save()
    }

    pub(crate) fn reconcile(&mut self, entries: &[SessionEntry]) -> io::Result<bool> {
        if self.holds.is_empty() {
            return Ok(false);
        }
        let before = self.holds.len();
        for entry in entries {
            let last_used_ms = epoch_ms(entry.last_used);
            for id in std::iter::once(&entry.session_id).chain(&entry.copies) {
                if self
                    .holds
                    .get(id)
                    .is_some_and(|hold| last_used_ms > hold.since_ms)
                {
                    self.holds.remove(id);
                }
            }
        }
        let changed = self.holds.len() != before;
        if changed {
            self.save()?;
        }
        Ok(changed)
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        write_atomically(path, &serde_json::to_vec_pretty(&self.holds)?)
    }
}

fn now_ms() -> u64 {
    epoch_ms(SystemTime::now())
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ccview-on-hold-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("on_hold.json")
    }

    fn entry(id: &str, copies: &[&str], last_used_ms: u64) -> SessionEntry {
        SessionEntry {
            session_id: id.to_owned(),
            copies: copies.iter().map(|id| (*id).to_owned()).collect(),
            last_used: UNIX_EPOCH + Duration::from_millis(last_used_ms),
            ..SessionEntry::stub()
        }
    }

    fn held(since_ms: u64, ids: &[&str]) -> OnHoldSessions {
        let mut on_hold = OnHoldSessions::default();
        for id in ids {
            on_hold.holds.insert((*id).to_owned(), Hold { since_ms });
        }
        on_hold
    }

    #[test]
    fn a_corrupt_file_loads_empty() {
        let path = scratch("corrupt");
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(OnHoldSessions::load(path).ids().count(), 0);
    }

    #[test]
    fn replying_to_a_held_session_releases_it() {
        let cases = [
            (
                "a later message on the flagged id",
                entry("a", &[], 2_000),
                false,
            ),
            (
                "a later message on a lineage member",
                entry("b", &["a"], 2_000),
                false,
            ),
            (
                "a message exactly at the flag time",
                entry("a", &[], 1_000),
                true,
            ),
            ("a message before the flag time", entry("a", &[], 500), true),
            (
                "an id missing from the catalog",
                entry("other", &[], 9_000),
                true,
            ),
        ];
        for (name, catalog_entry, kept) in cases {
            let mut on_hold = held(1_000, &["a"]);
            on_hold.reconcile(&[catalog_entry]).unwrap();
            assert_eq!(on_hold.contains("a"), kept, "{name}");
        }
    }
}
