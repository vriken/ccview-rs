use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) fn claude_config_dir() -> Option<PathBuf> {
    if let Some(config_dir) =
        std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(config_dir));
    }
    std::env::var_os("HOME").map(|home| Path::new(&home).join(".claude"))
}

const RELOADED_CONFIG_FILES: [&str; 3] = [
    "CLAUDE.md",
    "settings.json",
    "plugins/installed_plugins.json",
];

pub(crate) fn config_changed_ms(config_dir: &Path) -> Option<u64> {
    RELOADED_CONFIG_FILES
        .iter()
        .filter_map(|name| fs::metadata(config_dir.join(name)).ok()?.modified().ok())
        .filter_map(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_millis() as u64)
        .max()
}

pub(crate) fn projects_dir() -> Option<PathBuf> {
    claude_config_dir().map(|dir| dir.join("projects"))
}

pub(crate) fn find_sessions(projects_dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    for project in projects.flatten().filter(|p| p.path().is_dir()) {
        let Ok(files) = fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                let modified = file
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                found.push((modified, path));
            }
        }
    }
    found.sort_by_key(|file| std::cmp::Reverse(file.0));
    found.into_iter().map(|(_, path)| path).collect()
}
