use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

pub(crate) fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

pub(crate) fn is_held(path: &Path) -> io::Result<bool> {
    match File::open(path) {
        Ok(file) => Ok(!try_lock_exclusive(&file)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
