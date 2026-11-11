use crate::sources::claude::scan::FileRecord;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

const FORMAT_VERSION: u32 = 4;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize)]
struct IndexFile {
    format_version: u32,
    app_version: String,
    sources: BTreeMap<String, BTreeMap<String, FileRecord>>,
}

#[derive(Default)]
pub(crate) struct IndexCache {
    path: Option<PathBuf>,
    sources: BTreeMap<String, BTreeMap<String, FileRecord>>,
    dirty: bool,
}

impl IndexCache {
    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self::default()
    }

    pub(crate) fn load_default() -> Self {
        Self::load(default_path())
    }

    pub(crate) fn load(path: PathBuf) -> Self {
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

    pub(crate) fn get(&self, source: &str, path: &Path) -> Option<&FileRecord> {
        self.sources.get(source)?.get(&key(path))
    }

    pub(crate) fn insert(&mut self, source: &str, path: &Path, record: FileRecord) {
        self.sources
            .entry(source.to_owned())
            .or_default()
            .insert(key(path), record);
        self.dirty = true;
    }

    pub(crate) fn records(&self, source: &str) -> impl Iterator<Item = (PathBuf, &FileRecord)> {
        self.sources
            .get(source)
            .into_iter()
            .flatten()
            .map(|(path, record)| (PathBuf::from(path), record))
    }

    pub(crate) fn remove_missing(
        &mut self,
        source: &str,
        present: &HashSet<PathBuf>,
    ) -> Vec<PathBuf> {
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

    pub(crate) fn save_if_dirty(&mut self) -> io::Result<()> {
        let Some(path) = self.path.as_ref().filter(|_| self.dirty) else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(&IndexFile {
            format_version: FORMAT_VERSION,
            app_version: APP_VERSION.to_owned(),
            sources: self.sources.clone(),
        })?;
        crate::store::write_atomically(path, &bytes)?;
        self.dirty = false;
        Ok(())
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn default_path() -> PathBuf {
    crate::store::cache_file("index-v1.json")
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

        let foreign_format = valid.replace(
            &format!("\"format_version\":{FORMAT_VERSION}"),
            "\"format_version\":99",
        );
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
    fn an_index_written_before_message_time_ordering_is_not_trusted() {
        let path = temp_path("v2");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut cache = IndexCache::load(path.clone());
        cache.insert("claude", Path::new("/p/a.jsonl"), record(1));
        cache.save_if_dirty().unwrap();
        let old = fs::read_to_string(&path).unwrap().replace(
            &format!("\"format_version\":{FORMAT_VERSION}"),
            "\"format_version\":2",
        );
        fs::write(&path, old).unwrap();
        assert!(
            IndexCache::load(path.clone())
                .get("claude", Path::new("/p/a.jsonl"))
                .is_none()
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
