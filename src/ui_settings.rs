use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const DEFAULT_SPLIT_RATIO: f64 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalApp {
    Terminal,
    ITerm,
    Ghostty,
    WezTerm,
    #[default]
    #[serde(other)]
    Auto,
}

impl TerminalApp {
    pub const ALL: [TerminalApp; 5] = [
        Self::Auto,
        Self::Terminal,
        Self::ITerm,
        Self::Ghostty,
        Self::WezTerm,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Terminal => "Terminal",
            Self::ITerm => "iTerm",
            Self::Ghostty => "Ghostty",
            Self::WezTerm => "WezTerm",
        }
    }

    pub fn app_name(self) -> Option<&'static str> {
        (self != Self::Auto).then(|| self.label())
    }

    pub fn step(self, delta: isize) -> Self {
        let index = Self::ALL.iter().position(|app| *app == self).unwrap_or(0) as isize;
        Self::ALL[(index + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub split_ratio: f64,
    pub mouse: bool,
    pub identity_colors: bool,
    pub terminal_app: TerminalApp,
    pub disable_agent_view: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            split_ratio: DEFAULT_SPLIT_RATIO,
            mouse: true,
            identity_colors: true,
            terminal_app: TerminalApp::Auto,
            disable_agent_view: true,
        }
    }
}

impl UiSettings {
    fn sanitized(mut self) -> Self {
        if !self.split_ratio.is_finite() {
            self.split_ratio = DEFAULT_SPLIT_RATIO;
        }
        self.split_ratio = self.split_ratio.clamp(0.0, 1.0);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct UiSettingsStore {
    path: Option<PathBuf>,
    settings: UiSettings,
}

impl UiSettingsStore {
    pub fn load_default() -> Self {
        Self::load(default_path())
    }

    pub fn load(path: PathBuf) -> Self {
        let settings = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<UiSettings>(&bytes).ok())
            .unwrap_or_default()
            .sanitized();
        Self {
            path: Some(path),
            settings,
        }
    }

    pub fn settings(&self) -> UiSettings {
        self.settings
    }

    pub fn set_split_ratio(&mut self, ratio: f64) -> io::Result<()> {
        self.update(|settings| settings.split_ratio = ratio)
    }

    pub fn update(&mut self, change: impl FnOnce(&mut UiSettings)) -> io::Result<()> {
        change(&mut self.settings);
        self.settings = self.settings.sanitized();
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temp = temp_sibling(path);
        fs::write(&temp, serde_json::to_vec_pretty(&self.settings)?)?;
        fs::rename(&temp, path)
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn default_path() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
        .unwrap_or_default();
    config_home.join("ccview-rs/ui.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ccview-ui-{}-{name}/ui.json", std::process::id()))
    }

    #[test]
    fn a_file_from_before_the_new_fields_loads_with_their_defaults() {
        let path = temp_path("old");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"split_ratio": 0.5, "mouse": false}"#).unwrap();
        let settings = UiSettingsStore::load(path.clone()).settings();
        assert_eq!((settings.split_ratio, settings.mouse), (0.5, false));
        assert!(settings.identity_colors && settings.disable_agent_view);
        assert_eq!(settings.terminal_app, TerminalApp::Auto);
        fs::write(&path, r#"{"terminal_app": "someday", "split_ratio": 0.4}"#).unwrap();
        let settings = UiSettingsStore::load(path.clone()).settings();
        assert_eq!(
            (settings.terminal_app, settings.split_ratio),
            (TerminalApp::Auto, 0.4)
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_or_out_of_range_file_falls_back_and_is_replaced_on_save() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        let mut store = UiSettingsStore::load(path.clone());
        assert_eq!(store.settings(), UiSettings::default());
        fs::write(&path, r#"{"split_ratio": 7.0}"#).unwrap();
        assert_eq!(
            UiSettingsStore::load(path.clone()).settings().split_ratio,
            1.0
        );
        store.set_split_ratio(f64::NAN).unwrap();
        assert_eq!(
            UiSettingsStore::load(path.clone()).settings().split_ratio,
            DEFAULT_SPLIT_RATIO
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
