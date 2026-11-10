use crate::sources::claude::scan::FileRecord;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

const FORMAT_VERSION: u32 = 2;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize)]
struct IndexFile {
    format_version: u32,
    app_version: String,
    sources: BTreeMap<String, BTreeMap<String, FileRecord>>,
}

#[derive(Default)]
pub struct IndexCache {
    path: Option<PathBuf>,
    sources: BTreeMap<String, BTreeMap<String, FileRecord>>,
    dirty: bool,
}

impl IndexCache {
    #[cfg(test)]
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn load_default() -> Self {
        Self::load(default_path())
    }

    pub fn load(path: PathBuf) -> Self {
        let sources = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<IndexFile>(&bytes).ok())
            .filter(|index| {
                index.format_version == FORMAT_VERSION && index.app_version == APP_VERSION
            })
            .map(|index| index.sources)
            .unwrap_or_default();
        Self {
            path: Some(path),
            sources,
            dirty: false,
        }
    }

    pub fn get(&self, source: &str, path: &Path) -> Option<&FileRecord> {
        self.sources.get(source)?.get(&key(path))
    }

    pub fn insert(&mut self, source: &str, path: &Path, record: FileRecord) {
        self.sources
            .entry(source.to_owned())
            .or_default()
            .insert(key(path), record);
        self.dirty = true;
    }

    pub fn records(&self, source: &str) -> impl Iterator<Item = (PathBuf, &FileRecord)> {
        self.sources
            .get(source)
            .into_iter()
            .flatten()
            .map(|(path, record)| (PathBuf::from(path), record))
    }

    pub fn remove_missing(&mut self, source: &str, present: &HashSet<PathBuf>) -> Vec<PathBuf> {
        let Some(records) = self.sources.get_mut(source) else {
            return Vec::new();
        };
        let missing: Vec<String> = records
            .keys()
            .filter(|path| !present.contains(Path::new(path.as_str())))
            .cloned()
            .collect();
        for path in &missing {
            records.remove(path);
        }
        self.dirty |= !missing.is_empty();
        missing.into_iter().map(PathBuf::from).collect()
    }

    pub fn save_if_dirty(&mut self) -> io::Result<()> {
        let Some(path) = self.path.as_ref().filter(|_| self.dirty) else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec(&IndexFile {
            format_version: FORMAT_VERSION,
            app_version: APP_VERSION.to_owned(),
            sources: self.sources.clone(),
        })?;
        let temp = temp_sibling(path);
        let written = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, path));
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        } else {
            self.dirty = false;
        }
        written
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

pub fn default_path() -> PathBuf {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))
        .unwrap_or_default();
    cache_home.join("ccview-rs/index-v1.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccview-index-{}-{name}/index-v1.json",
            std::process::id()
        ))
    }

    fn record(size: u64) -> FileRecord {
        FileRecord {
            size,
            ..FileRecord::default()
        }
    }

    #[test]
    fn saved_records_survive_a_reload_and_only_dirty_caches_write() {
        let path = temp_path("roundtrip");
        let mut cache = IndexCache::load(path.clone());
        cache.save_if_dirty().unwrap();
        assert!(!path.exists());
        cache.insert("claude", Path::new("/p/a.jsonl"), record(7));
        cache.save_if_dirty().unwrap();
        let reloaded = IndexCache::load(path.clone());
        assert_eq!(
            reloaded.get("claude", Path::new("/p/a.jsonl")),
            Some(&record(7))
        );
        assert_eq!(reloaded.get("codex", Path::new("/p/a.jsonl")), None);
        let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_truncated_and_foreign_version_files_load_empty() {
        let path = temp_path("bad");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut good = IndexCache::load(path.clone());
        good.insert("claude", Path::new("/p/a.jsonl"), record(1));
        good.save_if_dirty().unwrap();
        let valid = fs::read_to_string(&path).unwrap();

        let foreign_format = valid.replace("\"format_version\":2", "\"format_version\":99");
        let foreign_app = valid.replace(APP_VERSION, "0.0.0-other");
        let truncated = valid[..valid.len() / 2].to_owned();
        for bad in [
            foreign_format,
            foreign_app,
            truncated,
            "not json".to_owned(),
            String::new(),
        ] {
            fs::write(&path, bad).unwrap();
            assert!(
                IndexCache::load(path.clone())
                    .get("claude", Path::new("/p/a.jsonl"))
                    .is_none()
            );
        }
        fs::write(&path, &valid).unwrap();
        assert!(
            IndexCache::load(path.clone())
                .get("claude", Path::new("/p/a.jsonl"))
                .is_some()
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn remove_missing_drops_only_absent_paths() {
        let mut cache = IndexCache::in_memory();
        cache.insert("claude", Path::new("/p/a.jsonl"), record(1));
        cache.insert("claude", Path::new("/p/b.jsonl"), record(2));
        cache.insert("codex", Path::new("/p/c.jsonl"), record(3));
        let present: HashSet<PathBuf> = [PathBuf::from("/p/a.jsonl")].into();
        assert_eq!(
            cache.remove_missing("claude", &present),
            vec![PathBuf::from("/p/b.jsonl")]
        );
        assert!(cache.get("claude", Path::new("/p/a.jsonl")).is_some());
        assert!(cache.get("codex", Path::new("/p/c.jsonl")).is_some());
    }
}
