pub mod codex;
pub mod follow;
pub mod opencode;

use crate::library::TitleSource;
use std::{path::PathBuf, time::SystemTime};

const PROMPT_MAX_CHARS: usize = 160;
const SHORT_ID_CHARS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HarnessId {
    Claude,
    Codex,
    OpenCode,
}

impl HarnessId {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessId::Claude => "claude",
            HarnessId::Codex => "codex",
            HarnessId::OpenCode => "opencode",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [HarnessId::Claude, HarnessId::Codex, HarnessId::OpenCode]
            .into_iter()
            .find(|harness| harness.as_str() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptHandle {
    Rollout(PathBuf),
    Database {
        path: PathBuf,
        session_id: String,
    },
}

impl TranscriptHandle {
    pub fn entry_key(&self) -> PathBuf {
        match self {
            TranscriptHandle::Rollout(path) => path.clone(),
            TranscriptHandle::Database { path, session_id } => path.join(session_id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub harness: HarnessId,
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub title_source: TitleSource,
    pub last_used: SystemTime,
    pub handle: TranscriptHandle,
}

pub(crate) fn resolve_binary(env_key: &str, default: &str) -> String {
    std::env::var(env_key)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub(crate) fn prompt_title(text: &str) -> Option<String> {
    let first_line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(first_line.chars().take(PROMPT_MAX_CHARS).collect())
}

pub(crate) fn short_id(session_id: &str) -> String {
    session_id.chars().take(SHORT_ID_CHARS).collect()
}

pub(crate) fn iso8601_from_millis(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let sub_millis = millis.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{sub_millis:03}Z",
        day_seconds / 3600,
        day_seconds % 3600 / 60,
        day_seconds % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_millis_as_iso8601() {
        assert_eq!(iso8601_from_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_from_millis(1_787_650_590_999), "2026-08-25T09:36:30.999Z");
    }
}
