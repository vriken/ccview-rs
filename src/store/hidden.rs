use crate::store::{config_file, write_atomically};
use std::{collections::HashSet, fs, io, path::PathBuf};

#[derive(Default)]
pub struct HiddenSessions {
    path: Option<PathBuf>,
    ids: HashSet<String>,
}

impl HiddenSessions {
    pub fn load_default() -> Self {
        Self::load(config_file("hidden.json"))
    }

    pub fn load(path: PathBuf) -> Self {
        let ids = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashSet<String>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            ids,
        }
    }

    pub fn contains(&self, session_id: &str) -> bool {
        self.ids.contains(session_id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.ids.iter().map(String::as_str)
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
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut sorted: Vec<&String> = self.ids.iter().collect();
        sorted.sort();
        write_atomically(path, &serde_json::to_vec_pretty(&sorted)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccview-hidden-{}-{name}/hidden.json",
            std::process::id()
        ))
    }

    #[test]
    fn corrupt_or_missing_file_loads_empty_and_is_replaced_on_toggle() {
        let path = temp_path("corrupt");
        assert_eq!(HiddenSessions::load(path.clone()).count(), 0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        let mut hidden = HiddenSessions::load(path.clone());
        assert_eq!(hidden.count(), 0);
        hidden.toggle("x").unwrap();
        assert!(HiddenSessions::load(path.clone()).contains("x"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
