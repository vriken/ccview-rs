use crate::library::SessionEntry;
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

pub struct Favorites {
    path: PathBuf,
    ids: HashSet<String>,
}

impl Favorites {
    pub fn load_default() -> Self {
        Self::load(default_path())
    }

    pub fn load(path: PathBuf) -> Self {
        let ids = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashSet<String>>(&bytes).ok())
            .unwrap_or_default();
        Self { path, ids }
    }

    pub fn contains(&self, session_id: &str) -> bool {
        self.ids.contains(session_id)
    }

    pub fn contains_entry(&self, entry: &SessionEntry) -> bool {
        self.contains(&entry.session_id) || entry.copies.iter().any(|id| self.contains(id))
    }

    pub fn toggle_entry(&mut self, entry: &SessionEntry) -> io::Result<()> {
        let starred = self.contains_entry(entry);
        for id in std::iter::once(&entry.session_id).chain(&entry.copies) {
            if starred {
                self.ids.remove(id);
            } else if *id == entry.session_id {
                self.ids.insert(id.clone());
            }
        }
        self.save()
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    pub fn toggle(&mut self, session_id: &str) -> io::Result<()> {
        if !self.ids.remove(session_id) {
            self.ids.insert(session_id.to_owned());
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let mut sorted: Vec<&String> = self.ids.iter().collect();
        sorted.sort();
        write_atomically(&self.path, &serde_json::to_vec_pretty(&sorted)?)
    }
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = temp_sibling(path);
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn default_path() -> PathBuf {
    config_file("favorites.json")
}

pub fn config_file(name: &str) -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
        .unwrap_or_default();
    config_home.join("ccview-rs").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ccview-fav-{}-{name}/favorites.json", std::process::id()))
    }

    #[test]
    fn toggles_persist_across_loads() {
        let path = temp_path("roundtrip");
        let mut favorites = Favorites::load(path.clone());
        favorites.toggle("a").unwrap();
        favorites.toggle("b").unwrap();
        favorites.toggle("a").unwrap();
        let reloaded = Favorites::load(path.clone());
        assert!(!reloaded.contains("a") && reloaded.contains("b"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_or_missing_file_loads_empty_and_is_replaced_on_toggle() {
        let path = temp_path("corrupt");
        assert_eq!(Favorites::load(path.clone()).count(), 0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        let mut favorites = Favorites::load(path.clone());
        assert_eq!(favorites.count(), 0);
        favorites.toggle("x").unwrap();
        assert!(Favorites::load(path.clone()).contains("x"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
