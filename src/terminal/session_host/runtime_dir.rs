use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const RUNTIME_DIR_ENV: &str = "CCVIEW_RUNTIME_DIR";
const XDG_RUNTIME_DIR_ENV: &str = "XDG_RUNTIME_DIR";
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const GROUP_AND_OTHER: u32 = 0o077;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var_os(RUNTIME_DIR_ENV),
            std::env::var_os(XDG_RUNTIME_DIR_ENV),
            std::env::temp_dir(),
            effective_uid(),
        )
    }

    fn resolve(
        explicit: Option<std::ffi::OsString>,
        xdg: Option<std::ffi::OsString>,
        temp: PathBuf,
        uid: u32,
    ) -> Self {
        let non_empty =
            |value: std::ffi::OsString| (!value.is_empty()).then_some(PathBuf::from(value));
        let path = match (explicit.and_then(non_empty), xdg.and_then(non_empty)) {
            (Some(explicit), _) => explicit,
            (None, Some(xdg)) => xdg.join("ccview-rs"),
            (None, None) => temp.join(format!("ccview-{uid}")),
        };
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn socket(&self) -> PathBuf {
        self.path.join("host.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.path.join("host.lock")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.path.join("host.pid")
    }

    pub fn log(&self) -> PathBuf {
        self.path.join("host.log")
    }

    pub fn ensure(&self) -> io::Result<()> {
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(&self.path)?;
        let metadata = fs::metadata(&self.path)?;
        if metadata.uid() != effective_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not owned by this user", self.path.display()),
            ));
        }
        if metadata.permissions().mode() & GROUP_AND_OTHER != 0 {
            fs::set_permissions(&self.path, fs::Permissions::from_mode(DIR_MODE))?;
        }
        Ok(())
    }

    pub fn open_private(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(path)
    }
}

pub fn effective_uid() -> u32 {
    crate::sys::process::effective_uid()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn resolved(explicit: Option<&str>, xdg: Option<&str>) -> PathBuf {
        RuntimeDir::resolve(
            explicit.map(OsString::from),
            xdg.map(OsString::from),
            PathBuf::from("/var/tmp-x"),
            501,
        )
        .path
    }

    #[test]
    fn the_override_wins_then_xdg_then_the_per_user_temp_dir() {
        assert_eq!(
            resolved(Some("/t/own"), Some("/run/user/1")),
            PathBuf::from("/t/own")
        );
        assert_eq!(
            resolved(None, Some("/run/user/1")),
            PathBuf::from("/run/user/1/ccview-rs")
        );
        assert_eq!(resolved(None, None), PathBuf::from("/var/tmp-x/ccview-501"));
        assert_eq!(
            resolved(Some(""), Some("")),
            PathBuf::from("/var/tmp-x/ccview-501")
        );
    }

    #[test]
    fn the_directory_is_private_even_when_it_pre_existed_with_wider_permissions() {
        let path = std::env::temp_dir().join(format!("ccview-rtdir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        RuntimeDir::at(&path).ensure().unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(&path).unwrap();
    }
}
