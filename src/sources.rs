pub mod claude;
pub mod codex;
pub mod follow;
pub mod opencode;

use crate::session::{
    entry::{TitleSource, TranscriptHandle},
    harness_id::HarnessId,
};
use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, SystemTime},
};

const PROMPT_MAX_CHARS: usize = 160;
const SHORT_ID_CHARS: usize = 8;

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

#[derive(Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub len: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgeFilter {
    max_age: Option<Duration>,
    keep: HashSet<String>,
}

impl AgeFilter {
    pub fn new(max_age: Option<Duration>, keep: HashSet<String>) -> Self {
        Self { max_age, keep }
    }

    pub fn cutoff(&self) -> Option<SystemTime> {
        SystemTime::now().checked_sub(self.max_age?)
    }

    pub fn keep_ids(&self) -> &HashSet<String> {
        &self.keep
    }

    pub fn admits_id(&self, modified: SystemTime, session_id: &str) -> bool {
        self.is_recent(modified) || self.keep.contains(session_id)
    }

    pub fn admits_file_stem(&self, modified: SystemTime, stem: &str) -> bool {
        self.is_recent(modified) || self.keep.iter().any(|id| stem.ends_with(id.as_str()))
    }

    fn is_recent(&self, modified: SystemTime) -> bool {
        self.cutoff().is_none_or(|cutoff| modified >= cutoff)
    }
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
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

pub(crate) fn millis_from_rfc3339(text: &str) -> Option<u64> {
    let (date, rest) = text.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let zone_start = rest.find(['Z', '+', '-'])?;
    let (clock, zone) = rest.split_at(zone_start);
    let (whole_seconds, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut clock_parts = whole_seconds.splitn(3, ':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    let offset_seconds = match zone {
        "Z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (offset_hours, offset_minutes) = zone[1..].split_once(':')?;
            sign * (offset_hours.parse::<i64>().ok()? * 3600
                + offset_minutes.parse::<i64>().ok()? * 60)
        }
    };
    let in_range = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60
        && second < 61;
    if !in_range {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
        - offset_seconds;
    u64::try_from(seconds * 1000 + millis).ok()
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted_year = year - i64::from(month <= 2);
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_forms_into_epoch_millis() {
        assert_eq!(
            millis_from_rfc3339("2026-08-25T09:36:30.999Z"),
            Some(1_787_650_590_999)
        );
        assert_eq!(
            millis_from_rfc3339("2026-08-25T09:36:30Z"),
            Some(1_787_650_590_000)
        );
        assert_eq!(
            millis_from_rfc3339("2026-08-25T11:36:30.5+02:00"),
            Some(1_787_650_590_500)
        );
        assert_eq!(
            millis_from_rfc3339("2026-08-25T05:06:30.123456-04:30"),
            Some(1_787_650_590_123)
        );
        assert_eq!(millis_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn rejects_malformed_timestamps() {
        for bad in [
            "",
            "garbage",
            "2026-08-25",
            "2026-08-25T09:36:30",
            "2026-13-25T09:36:30Z",
            "2026-08-25T25:36:30Z",
            "2026-08-25T09:36:30.x9Z",
            "2026-08-25T09:36:30+0200",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(millis_from_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn formats_epoch_millis_as_iso8601() {
        assert_eq!(iso8601_from_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso8601_from_millis(1_787_650_590_999),
            "2026-08-25T09:36:30.999Z"
        );
    }
}
