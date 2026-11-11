use crate::store::{config_file, write_atomically};
use std::{collections::BTreeMap, fs, io, path::PathBuf};

#[derive(Default)]
pub struct ManualOrder {
    path: Option<PathBuf>,
    tabs: BTreeMap<String, Vec<String>>,
}

impl ManualOrder {
    pub fn load_default() -> Self {
        Self::load(config_file("order.json"))
    }

    pub fn load(path: PathBuf) -> Self {
        let tabs = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            tabs,
        }
    }

    pub fn ids(&self, tab: &str) -> &[String] {
        self.tabs.get(tab).map_or(&[], Vec::as_slice)
    }

    pub fn set(&mut self, tab: &str, ids: Vec<String>) -> io::Result<()> {
        self.tabs.insert(tab.to_owned(), ids);
        self.save()
    }

    pub fn clear(&mut self, tab: &str) -> io::Result<()> {
        if self.tabs.remove(tab).is_none() {
            return Ok(());
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        write_atomically(path, &serde_json::to_vec_pretty(&self.tabs)?)
    }
}
