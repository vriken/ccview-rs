use std::{
    ffi::OsStr,
    fs, io,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::Path,
    process::Command,
    time::SystemTime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutableStamp {
    inode: u64,
    modified: Option<SystemTime>,
    len: u64,
    pub(crate) runnable: bool,
}

pub(crate) fn stamp_of_executable(path: &Path) -> Option<ExecutableStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(ExecutableStamp {
        inode: metadata.ino(),
        modified: metadata.modified().ok(),
        len: metadata.len(),
        runnable: metadata.is_file() && metadata.len() > 0 && metadata.mode() & 0o111 != 0,
    })
}

pub(crate) fn replace_process(
    program: &Path,
    arg0: &OsStr,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> io::Error {
    Command::new(program).arg0(arg0).args(args).exec()
}
