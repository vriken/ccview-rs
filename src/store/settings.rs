use serde::{Deserialize, Deserializer, Serialize};
use std::{fs, io, path::PathBuf, time::Duration};

pub(crate) const DEFAULT_SPLIT_RATIO: f64 = 0.35;
pub(crate) const DEFAULT_MAX_AGE_DAYS: u32 = 30;
pub(crate) const NO_AGE_LIMIT: u32 = 0;
pub(crate) const MAX_AGE_CHOICES: [u32; 8] = [7, 14, 30, 60, 90, 180, 365, NO_AGE_LIMIT];
const SECONDS_PER_DAY: u64 = 86_400;

fn age_rank(days: u32) -> u32 {
    if days == NO_AGE_LIMIT { u32::MAX } else { days }
}

pub(crate) fn step_max_age(days: u32, delta: isize) -> u32 {
    let current = age_rank(days);
    let neighbour = if delta >= 0 {
        MAX_AGE_CHOICES
            .into_iter()
            .filter(|choice| age_rank(*choice) > current)
            .min_by_key(|choice| age_rank(*choice))
    } else {
        MAX_AGE_CHOICES
            .into_iter()
            .filter(|choice| age_rank(*choice) < current)
            .max_by_key(|choice| age_rank(*choice))
    };
    neighbour.unwrap_or(days)
}

pub(crate) fn max_age_label(days: u32) -> String {
    match days {
        NO_AGE_LIMIT => "all".to_owned(),
        1 => "1 day".to_owned(),
        other => format!("{other} days"),
    }
}

fn lenient_max_age_days<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|days| u32::try_from(days).ok())
        .unwrap_or(DEFAULT_MAX_AGE_DAYS))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TerminalApp {
    Terminal,
    ITerm,
    Ghostty,
    WezTerm,
    #[default]
    #[serde(other)]
    Auto,
}

impl TerminalApp {
    pub(crate) const ALL: [TerminalApp; 5] = [
        Self::Auto,
        Self::Terminal,
        Self::ITerm,
        Self::Ghostty,
        Self::WezTerm,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Terminal => "Terminal",
            Self::ITerm => "iTerm",
            Self::Ghostty => "Ghostty",
            Self::WezTerm => "WezTerm",
        }
    }

    pub(crate) fn app_name(self) -> Option<&'static str> {
        (self != Self::Auto).then(|| self.label())
    }

    pub(crate) fn step(self, delta: isize) -> Self {
        let index = Self::ALL.iter().position(|app| *app == self).unwrap_or(0) as isize;
        Self::ALL[(index + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UiSettings {
    pub(crate) split_ratio: f64,
    pub(crate) mouse: bool,
    pub(crate) identity_colors: bool,
    pub(crate) terminal_app: TerminalApp,
    pub(crate) disable_agent_view: bool,
    #[serde(deserialize_with = "lenient_max_age_days")]
    pub(crate) max_age_days: u32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            split_ratio: DEFAULT_SPLIT_RATIO,
            mouse: true,
            identity_colors: true,
            terminal_app: TerminalApp::Auto,
            disable_agent_view: true,
            max_age_days: DEFAULT_MAX_AGE_DAYS,
        }
    }
}

impl UiSettings {
    pub(crate) fn max_age(&self) -> Option<Duration> {
        (self.max_age_days != NO_AGE_LIMIT)
            .then(|| Duration::from_secs(u64::from(self.max_age_days) * SECONDS_PER_DAY))
    }

    fn sanitized(mut self) -> Self {
        if !self.split_ratio.is_finite() {
            self.split_ratio = DEFAULT_SPLIT_RATIO;
        }
        self.split_ratio = self.split_ratio.clamp(0.0, 1.0);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UiSettingsStore {
    path: Option<PathBuf>,
    settings: UiSettings,
}

impl UiSettingsStore {
    pub(crate) fn load_default() -> Self {
        Self::load(default_path())
    }

    pub(crate) fn load(path: PathBuf) -> Self {
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

    pub(crate) fn settings(&self) -> UiSettings {
        self.settings
    }

    pub(crate) fn set_split_ratio(&mut self, ratio: f64) -> io::Result<()> {
        self.update(|settings| settings.split_ratio = ratio)
    }

    pub(crate) fn update(&mut self, change: impl FnOnce(&mut UiSettings)) -> io::Result<()> {
        change(&mut self.settings);
        self.settings = self.settings.sanitized();
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::store::write_atomically(path, &serde_json::to_vec_pretty(&self.settings)?)
    }
}

fn default_path() -> PathBuf {
    crate::store::config_file("ui.json")
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

#[cfg(test)]
mod max_age_tests {
    use super::*;

    fn loaded(json: &str) -> UiSettings {
        let path = std::env::temp_dir().join(format!(
            "ccview-ui-age-{}-{}/ui.json",
            std::process::id(),
            json.len()
        ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, json).unwrap();
        let settings = UiSettingsStore::load(path.clone()).settings();
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        settings
    }

    #[test]
    fn the_limit_defaults_to_thirty_days_and_keeps_the_rest_of_the_file() {
        for json in [
            r#"{"mouse": false}"#,
            r#"{"max_age_days": "soon", "mouse": false}"#,
            r#"{"max_age_days": -3, "mouse": false}"#,
            r#"{"max_age_days": null, "mouse": false}"#,
        ] {
            let settings = loaded(json);
            assert_eq!(
                (settings.max_age_days, settings.mouse),
                (30, false),
                "{json}"
            );
        }
    }

    #[test]
    fn a_hand_edited_limit_is_honoured_and_zero_means_no_limit() {
        assert_eq!(loaded(r#"{"max_age_days": 45}"#).max_age_days, 45);
        assert_eq!(loaded(r#"{"max_age_days": 0}"#).max_age(), None);
    }

    #[test]
    fn stepping_snaps_a_custom_value_to_its_neighbour_choices() {
        assert_eq!(step_max_age(45, 1), 60);
        assert_eq!(step_max_age(45, -1), 30);
        assert_eq!(step_max_age(365, 1), NO_AGE_LIMIT);
        assert_eq!(step_max_age(NO_AGE_LIMIT, -1), 365);
        assert_eq!(step_max_age(NO_AGE_LIMIT, 1), NO_AGE_LIMIT);
        assert_eq!(step_max_age(7, -1), 7);
    }
}
