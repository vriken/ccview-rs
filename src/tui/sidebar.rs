use crate::{discover, parse};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct SessionEntry {
    pub path: PathBuf,
    pub preview: String,
    pub modified: SystemTime,
}

pub struct Project {
    pub name: String,
    pub sessions: Vec<SessionEntry>,
}

pub fn load_projects(projects_dir: &Path) -> Vec<Project> {
    let mut projects: Vec<Project> = Vec::new();
    for path in discover::find_sessions(projects_dir) {
        let Some(dir_name) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
        else {
            continue;
        };
        let name = decode_project_name(dir_name);
        let entry = SessionEntry {
            preview: preview_of(&path),
            modified: path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH),
            path,
        };
        match projects.iter_mut().find(|p| p.name == name) {
            Some(project) => project.sessions.push(entry),
            None => projects.push(Project {
                name,
                sessions: vec![entry],
            }),
        }
    }
    projects
}

fn decode_project_name(dir_name: &str) -> String {
    let decoded = dir_name.replace('-', "/");
    let home = std::env::var("HOME").unwrap_or_default();
    match decoded.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => decoded,
    }
}

fn preview_of(path: &Path) -> String {
    let text = parse::first_user_text(path).unwrap_or_else(|| "(no user text)".to_owned());
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn format_time(modified: SystemTime, now: SystemTime) -> String {
    let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
    let seconds = age.as_secs();
    match seconds {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        86_400..604_800 => format!("{}d ago", seconds / 86_400),
        _ => absolute_date(modified),
    }
}

fn absolute_date(time: SystemTime) -> String {
    let days = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
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
    format!("{year}-{month:02}-{day:02}")
}
