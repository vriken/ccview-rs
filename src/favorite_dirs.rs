use crate::favorites::{config_file, write_atomically};
use std::{collections::HashSet, fs, io, path::PathBuf};

#[derive(Default)]
pub struct FavoriteDirs {
    path: Option<PathBuf>,
    dirs: HashSet<String>,
}

impl FavoriteDirs {
    pub fn load_default() -> Self {
        Self::load(config_file("favorite_dirs.json"))
    }

    pub fn load(path: PathBuf) -> Self {
        let dirs = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashSet<String>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            dirs,
        }
    }

    pub fn contains(&self, cwd: &str) -> bool {
        self.dirs.contains(cwd)
    }

    pub fn toggle(&mut self, cwd: &str) -> io::Result<()> {
        if !self.dirs.remove(cwd) {
            self.dirs.insert(cwd.to_owned());
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut sorted: Vec<&String> = self.dirs.iter().collect();
        sorted.sort();
        write_atomically(path, &serde_json::to_vec_pretty(&sorted)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccview-favdirs-{}-{name}/favorite_dirs.json",
            std::process::id()
        ))
    }

    #[test]
    fn toggles_persist_across_loads_and_leave_favorites_json_alone() {
        let path = temp_path("roundtrip");
        let mut dirs = FavoriteDirs::load(path.clone());
        dirs.toggle("/w/one").unwrap();
        dirs.toggle("/w/two").unwrap();
        dirs.toggle("/w/one").unwrap();
        let reloaded = FavoriteDirs::load(path.clone());
        assert!(!reloaded.contains("/w/one") && reloaded.contains("/w/two"));
        assert!(!path.with_file_name("favorites.json").exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_or_missing_file_loads_empty_and_is_replaced_on_toggle() {
        let path = temp_path("corrupt");
        assert!(!FavoriteDirs::load(path.clone()).contains("/w/one"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        let mut dirs = FavoriteDirs::load(path.clone());
        assert!(!dirs.contains("/w/one"));
        dirs.toggle("/w/one").unwrap();
        assert!(FavoriteDirs::load(path.clone()).contains("/w/one"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
