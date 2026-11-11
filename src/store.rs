use std::{
    fs, io,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub mod dismissed;
pub mod favorite_dirs;
pub mod favorites;
pub mod hidden;
pub mod important;
pub mod order;
pub mod settings;

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let temp = temp_sibling(path);
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?
        .write_all(bytes)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_written_file_is_private_to_the_owner_and_so_is_its_new_directory() {
        let root = std::env::temp_dir().join(format!("ccview-store-mode-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join("new/dir/file.json");
        write_atomically(&path, b"{}").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let _ = fs::remove_dir_all(&root);
    }
}
