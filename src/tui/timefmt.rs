unsafe extern "C" {
    fn tzset();
}

pub struct Stamp {
    pub day: String,
    pub clock: String,
}

pub fn local_stamp(iso: &str) -> Option<Stamp> {
    let epoch = parse_epoch(iso)?;
    let (year, month, day, hour, minute) = local_fields(epoch)?;
    Some(Stamp {
        day: format!("{year:04}-{month:02}-{day:02}"),
        clock: format!("{hour:02}:{minute:02}"),
    })
}

fn parse_epoch(iso: &str) -> Option<i64> {
    let bytes = iso.as_bytes();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        std::str::from_utf8(bytes.get(range)?).ok()?.parse().ok()
    };
    if bytes.get(4) != Some(&b'-') || bytes.get(10) != Some(&b'T') || bytes.get(13) != Some(&b':') {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19).unwrap_or(0));
    let mut rest = &iso[19.min(iso.len())..];
    if let Some(fraction) = rest.strip_prefix('.') {
        rest = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset_seconds = match rest.as_bytes() {
        [sign @ (b'+' | b'-'), digits @ ..] => {
            let digits: Vec<u8> = digits.iter().copied().filter(|b| *b != b':').collect();
            let text = std::str::from_utf8(&digits).ok()?;
            let hours: i64 = text.get(0..2)?.parse().ok()?;
            let minutes: i64 = text.get(2..4).unwrap_or("00").parse().ok()?;
            let total = hours * 3600 + minutes * 60;
            if *sign == b'-' { -total } else { total }
        }
        _ => 0,
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn local_fields(epoch: i64) -> Option<(i32, u32, u32, u32, u32)> {
    let time = epoch as libc::time_t;
    let mut broken_down: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        tzset();
        if libc::localtime_r(&time, &mut broken_down).is_null() {
            return None;
        }
    }
    Some((
        broken_down.tm_year + 1900,
        (broken_down.tm_mon + 1) as u32,
        broken_down.tm_mday as u32,
        broken_down.tm_hour as u32,
        broken_down.tm_min as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const CHILD_FLAG: &str = "CCVIEW_TZ_CHILD";

    fn rendered(iso: &str) -> String {
        let stamp = local_stamp(iso).unwrap();
        format!("{} {}", stamp.day, stamp.clock)
    }

    #[test]
    fn utc_and_offset_timestamps_denote_the_same_instant() {
        assert_eq!(parse_epoch("2026-11-10T10:35:31.582Z"), parse_epoch("2026-11-10T11:35:31+01:00"));
        assert_eq!(parse_epoch("1970-01-02T00:00:00Z"), Some(86_400));
    }

    #[test]
    fn garbage_is_not_a_timestamp() {
        assert!(local_stamp("yesterday").is_none() && local_stamp("").is_none());
    }

    #[test]
    fn converts_to_the_local_zone_including_half_hour_and_day_rollover() {
        for (zone, expected) in [
            ("UTC", "2026-11-10 23:50"),
            ("Asia/Kolkata", "2026-11-11 05:20"),
            ("America/New_York", "2026-11-10 18:50"),
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tui::timefmt::tests::child_prints_stamp", "--nocapture"])
                .env(CHILD_FLAG, "1")
                .env("TZ", zone)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains(&format!("STAMP[{expected}]")), "{zone}: {stdout}");
        }
    }

    #[test]
    fn child_prints_stamp() {
        if std::env::var_os(CHILD_FLAG).is_some() {
            println!("STAMP[{}]", rendered("2026-11-10T23:50:15.100Z"));
        }
    }
}
