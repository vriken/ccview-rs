use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub mod dismissed;
pub mod favorite_dirs;
pub mod favorites;
pub mod hidden;
pub mod important;
pub mod settings;

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = temp_sibling(path);
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

pub fn config_file(name: &str) -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
        .unwrap_or_default();
    config_home.join("ccview-rs").join(name)
}
