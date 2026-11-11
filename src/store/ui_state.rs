use crate::store::{state_file, write_atomically};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_AGE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FocusZone {
    List,
    Transcript,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiState {
    pub(crate) tab: String,
    pub(crate) session_id: Option<String>,
    pub(crate) focus: FocusZone,
    pub(crate) scroll: usize,
    pub(crate) follow_bottom: bool,
    pub(crate) show_hidden: bool,
    pub(crate) expanded: bool,
    pub(crate) saved_ms: u64,
}

pub(crate) fn default_path() -> PathBuf {
    state_file("ui_state.json")
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl UiState {
    pub(crate) fn save(&self, path: &Path) -> io::Result<()> {
        write_atomically(path, &serde_json::to_vec(self)?)
    }

    pub(crate) fn take(path: &Path, now_ms: u64) -> Option<Self> {
        let bytes = fs::read(path).ok();
        let _ = fs::remove_file(path);
        let state: Self = serde_json::from_slice(&bytes?).ok()?;
        let age_ms = now_ms.saturating_sub(state.saved_ms);
        (age_ms <= MAX_AGE.as_millis() as u64).then_some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_state_file_is_ignored_and_still_removed() {
        let dir = std::env::temp_dir().join(format!("ccview-ui-state-{}", std::process::id()));
        let path = dir.join("ui_state.json");
        let state = UiState {
            tab: "all".to_owned(),
            session_id: None,
            focus: FocusZone::List,
            scroll: 0,
            follow_bottom: true,
            show_hidden: false,
            expanded: false,
            saved_ms: 1_000,
        };
        state.save(&path).unwrap();
        assert_eq!(UiState::take(&path, 1_000 + 5_000), Some(state.clone()));
        state.save(&path).unwrap();
        assert_eq!(UiState::take(&path, 1_000 + 10 * 60_000), None);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
