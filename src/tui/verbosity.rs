use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Chat,
    Tools,
    Everything,
}

impl Level {
    pub fn next(self) -> Self {
        match self {
            Level::Chat => Level::Tools,
            Level::Tools => Level::Everything,
            Level::Everything => Level::Chat,
        }
    }

    pub fn step(self, delta: isize) -> Self {
        (0..delta.rem_euclid(3)).fold(self, |level, _| level.next())
    }

    pub fn label(self) -> &'static str {
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
pub struct ViewSetting {
    pub level: Level,
    path: Option<PathBuf>,
}

impl ViewSetting {
    pub fn load_default() -> Self {
        Self::load(default_path())
    }

    pub fn load(path: PathBuf) -> Self {
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

    pub fn set(&mut self, level: Level) -> io::Result<()> {
        self.level = level;
        match &self.path {
            Some(path) => save(path, self.level),
            None => Ok(()),
        }
    }
}

fn save(path: &Path, level: Level) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let temp = path.with_file_name(name);
    fs::write(&temp, serde_json::to_vec_pretty(&Stored { level })?)?;
    fs::rename(&temp, path)
}

fn default_path() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
        .unwrap_or_default();
    config_home.join("ccview-rs/view.json")
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
    fn corrupt_or_unknown_file_falls_back_to_chat() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for content in ["{not json", r#"{"level":"loud"}"#, ""] {
            fs::write(&path, content).unwrap();
            assert_eq!(
                ViewSetting::load(path.clone()).level,
                Level::Chat,
                "{content}"
            );
        }
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
