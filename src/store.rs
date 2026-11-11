use std::{
    fs, io,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) mod dismissed;
pub(crate) mod favorite_dirs;
pub(crate) mod favorites;
pub(crate) mod hidden;
pub(crate) mod important;
pub(crate) mod on_hold;
pub(crate) mod order;
pub(crate) mod settings;

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let temp = temp_sibling(path);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn temp_sibling(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

pub(crate) fn config_file(name: &str) -> PathBuf {
    xdg_file("XDG_CONFIG_HOME", ".config", name)
}

pub(crate) fn cache_file(name: &str) -> PathBuf {
    xdg_file("XDG_CACHE_HOME", ".cache", name)
}

fn xdg_file(variable: &str, home_fallback: &str, name: &str) -> PathBuf {
    let base = std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(home_fallback)))
        .unwrap_or_default();
    base.join("ccview-rs").join(name)
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

    #[test]
    fn concurrent_writers_of_one_path_all_succeed_and_leave_no_temp_files() {
        let root = std::env::temp_dir().join(format!("ccview-store-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join("file.json");
        let results: Vec<io::Result<()>> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || (0..50).try_for_each(|_| write_atomically(&path, b"{}")))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
