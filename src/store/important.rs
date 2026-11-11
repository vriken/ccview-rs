use crate::{
    session::entry::SessionEntry,
    store::{config_file, write_atomically},
};
use std::{collections::HashSet, fs, io, path::PathBuf};

#[derive(Default)]
pub(crate) struct ImportantSessions {
    path: Option<PathBuf>,
    ids: HashSet<String>,
}

impl ImportantSessions {
    pub(crate) fn load_default() -> Self {
        Self::load(config_file("important.json"))
    }

    pub(crate) fn load(path: PathBuf) -> Self {
        let ids = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashSet<String>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            ids,
        }
    }

    pub(crate) fn contains(&self, session_id: &str) -> bool {
        self.ids.contains(session_id)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = &String> {
        self.ids.iter()
    }

    pub(crate) fn contains_entry(&self, entry: &SessionEntry) -> bool {
        self.contains(&entry.session_id) || entry.copies.iter().any(|id| self.contains(id))
    }

    pub(crate) fn toggle_entry(&mut self, entry: &SessionEntry) -> io::Result<()> {
        let important = self.contains_entry(entry);
        for id in std::iter::once(&entry.session_id).chain(&entry.copies) {
            if important {
                self.ids.remove(id);
            } else if *id == entry.session_id {
                self.ids.insert(id.clone());
            }
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut sorted: Vec<&String> = self.ids.iter().collect();
        sorted.sort();
        write_atomically(path, &serde_json::to_vec_pretty(&sorted)?)
    }
}
