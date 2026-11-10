use super::palette;
use crate::favorites::{config_file, write_atomically};
use ratatui::style::Color;
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::PathBuf,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Session(String),
    Host(u64),
}

impl Key {
    fn seed(&self) -> String {
        match self {
            Key::Session(id) => id.clone(),
            Key::Host(id) => format!("host-{id}"),
        }
    }
}

pub struct Identity {
    path: PathBuf,
    overrides: HashMap<String, usize>,
    resolved: HashMap<Key, usize>,
    enabled: bool,
}

impl Identity {
    pub fn load_default() -> Self {
        Self::load(config_file("colors.json"))
    }

    pub fn load(path: PathBuf) -> Self {
        let overrides = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, usize>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, index)| *index < palette::SIZE)
            .collect();
        Self { path, overrides, resolved: HashMap::new(), enabled: true }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn index(&self, key: &Key) -> usize {
        self.assigned(key).unwrap_or_else(|| palette::index_for_seed(&key.seed()))
    }

    pub fn color(&self, key: &Key) -> Color {
        palette::color(self.index(key))
    }

    pub fn session_color(&self, session_id: &str) -> Color {
        self.color(&Key::Session(session_id.to_owned()))
    }

    pub fn track(&mut self, keys: impl IntoIterator<Item = Key>) {
        let keys: Vec<Key> = keys.into_iter().collect();
        let mut occupied: HashSet<usize> = keys.iter().filter_map(|key| self.assigned(key)).collect();
        for key in keys {
            if self.assigned(&key).is_some() {
                continue;
            }
            let start = palette::index_for_seed(&key.seed());
            let index = (0..palette::SIZE)
                .map(|step| (start + step) % palette::SIZE)
                .find(|candidate| !occupied.contains(candidate))
                .unwrap_or(start);
            occupied.insert(index);
            self.resolved.insert(key, index);
        }
    }

    pub fn adopt(&mut self, host: u64, session_id: &str) {
        let key = Key::Session(session_id.to_owned());
        if self.assigned(&key).is_none() {
            let index = self.index(&Key::Host(host));
            self.resolved.insert(key, index);
        }
    }

    pub fn cycle(&mut self, key: &Key) -> io::Result<()> {
        self.shift(key, 1)
    }

    pub fn shift(&mut self, key: &Key, delta: isize) -> io::Result<()> {
        let next = (self.index(key) as isize + delta).rem_euclid(palette::SIZE as isize) as usize;
        match key {
            Key::Session(id) => {
                self.overrides.insert(id.clone(), next);
                self.save()
            }
            Key::Host(_) => {
                self.resolved.insert(key.clone(), next);
                Ok(())
            }
        }
    }

    fn assigned(&self, key: &Key) -> Option<usize> {
        match key {
            Key::Session(id) => self.overrides.get(id).or_else(|| self.resolved.get(key)),
            Key::Host(_) => self.resolved.get(key),
        }
        .copied()
    }

    fn save(&self) -> io::Result<()> {
        let sorted: std::collections::BTreeMap<&String, &usize> = self.overrides.iter().collect();
        write_atomically(&self.path, &serde_json::to_vec_pretty(&sorted)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ccview-colors-{}-{name}/colors.json", std::process::id()))
    }

    fn session(id: &str) -> Key {
        Key::Session(id.to_owned())
    }

    fn ids(count: usize) -> Vec<Key> {
        (0..count).map(|n| session(&format!("session-{n:04}-{}", n * 7919))).collect()
    }

    #[test]
    fn tracked_sessions_never_share_a_colour_while_hues_are_free() {
        let mut identity = Identity::load(temp_path("collisions"));
        let keys = ids(palette::SIZE);
        identity.track(keys.clone());
        let used: HashSet<usize> = keys.iter().map(|key| identity.index(key)).collect();
        assert_eq!(used.len(), palette::SIZE);
    }

    #[test]
    fn tracking_more_sessions_keeps_the_colours_already_shown() {
        let mut identity = Identity::load(temp_path("stable"));
        let keys = ids(10);
        identity.track(keys[..6].to_vec());
        let before: Vec<usize> = keys[..6].iter().map(|key| identity.index(key)).collect();
        identity.track(keys.clone());
        identity.track(keys[3..].to_vec());
        let after: Vec<usize> = keys[..6].iter().map(|key| identity.index(key)).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn override_survives_a_reload_and_is_never_taken_by_a_new_session() {
        let path = temp_path("roundtrip");
        let mut identity = Identity::load(path.clone());
        let key = session("abc");
        identity.track([key.clone()]);
        let original = identity.index(&key);
        identity.cycle(&key).unwrap();
        let chosen = identity.index(&key);
        assert_eq!(chosen, (original + 1) % palette::SIZE);

        let mut reloaded = Identity::load(path.clone());
        assert_eq!(reloaded.index(&key), chosen);
        reloaded.track(ids(palette::SIZE - 1).into_iter().chain([key.clone()]));
        let others: HashSet<usize> = ids(palette::SIZE - 1).iter().map(|other| reloaded.index(other)).collect();
        assert!(!others.contains(&chosen));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_corrupt_or_out_of_range_file_loads_as_no_overrides() {
        let path = temp_path("corrupt");
        assert_eq!(Identity::load(path.clone()).overrides.len(), 0);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        assert_eq!(Identity::load(path.clone()).overrides.len(), 0);
        fs::write(&path, r#"{"a": 2, "b": 999}"#).unwrap();
        let identity = Identity::load(path.clone());
        assert_eq!(identity.index(&session("a")), 2);
        assert_eq!(identity.overrides.len(), 1);
        let mut identity = identity;
        identity.cycle(&session("c")).unwrap();
        assert_eq!(Identity::load(path.clone()).overrides.len(), 2);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
