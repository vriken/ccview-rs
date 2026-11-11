use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Level {
    Chat,
    #[default]
    Tools,
    Everything,
}

impl Level {
    pub(crate) fn next(self) -> Self {
        match self {
            Level::Chat => Level::Tools,
            Level::Tools => Level::Everything,
            Level::Everything => Level::Chat,
        }
    }

    pub(crate) fn step(self, delta: isize) -> Self {
        (0..delta.rem_euclid(3)).fold(self, |level, _| level.next())
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Level::Chat => "chat",
            Level::Tools => "tools",
            Level::Everything => "everything",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    level: Level,
}

#[derive(Default)]
pub(crate) struct ViewSetting {
    pub(crate) level: Level,
    path: Option<PathBuf>,
}

impl ViewSetting {
    pub(crate) fn load_default() -> Self {
        Self::load(default_path())
    }

    pub(crate) fn load(path: PathBuf) -> Self {
        let level = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .map(|stored| stored.level)
            .unwrap_or_default();
        Self {
            level,
            path: Some(path),
        }
    }

    pub(crate) fn set(&mut self, level: Level) -> io::Result<()> {
        self.level = level;
        match &self.path {
            Some(path) => save(path, self.level),
            None => Ok(()),
        }
    }
}

fn save(path: &Path, level: Level) -> io::Result<()> {
    crate::store::write_atomically(path, &serde_json::to_vec_pretty(&Stored { level })?)
}

fn default_path() -> PathBuf {
    crate::store::config_file("view.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccview-view-{}-{name}/view.json",
            std::process::id()
        ))
    }

    #[test]
    fn corrupt_or_unknown_file_falls_back_to_tools() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for content in ["{not json", r#"{"level":"loud"}"#, ""] {
            fs::write(&path, content).unwrap();
            assert_eq!(
                ViewSetting::load(path.clone()).level,
                Level::Tools,
                "{content}"
            );
        }
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
