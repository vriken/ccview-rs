use std::fs::{self, File};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixListener;
use std::path::Path;

use super::daemon::{bind, record_holder, try_lock};
use super::runtime_dir::RuntimeDir;

type FileId = (u64, u64);

fn id_of_path(path: &Path) -> Option<FileId> {
    let metadata = fs::symlink_metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

fn id_of_file(file: &File) -> Option<FileId> {
    let metadata = file.metadata().ok()?;
    Some((metadata.dev(), metadata.ino()))
}

pub(super) struct HostFiles {
    lock: File,
    socket: Option<FileId>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Heal {
    Healthy,
    Repaired,
    Superseded,
}

impl HostFiles {
    pub(super) fn claim(dir: &RuntimeDir) -> io::Result<Option<(Self, UnixListener)>> {
        let Some(lock) = try_lock(dir)? else {
            return Ok(None);
        };
        record_holder(&lock)?;
        write_pid(dir)?;
        let listener = bind(dir)?;
        let socket = id_of_path(&dir.socket());
        Ok(Some((Self { lock, socket }, listener)))
    }

    pub(super) fn heal(&mut self, dir: &RuntimeDir, listener: &mut UnixListener) -> Heal {
        let mut repaired = false;
        if id_of_path(&dir.lock()) != id_of_file(&self.lock) {
            match try_lock(dir) {
                Ok(Some(lock)) => {
                    let _ = record_holder(&lock);
                    self.lock = lock;
                    eprintln!(
                        "ccview-rs host: host.lock was replaced or removed, took the one at the path"
                    );
                    repaired = true;
                }
                Ok(None) => {
                    eprintln!("ccview-rs host: another host holds host.lock in this directory");
                    return Heal::Superseded;
                }
                Err(error) => {
                    eprintln!("ccview-rs host: could not re-take host.lock: {error}");
                    return Heal::Healthy;
                }
            }
        }
        if id_of_path(&dir.socket()) != self.socket {
            match bind(dir) {
                Ok(fresh) => {
                    *listener = fresh;
                    self.socket = id_of_path(&dir.socket());
                    eprintln!("ccview-rs host: host.sock was missing or replaced, bound a new one");
                    repaired = true;
                }
                Err(error) => eprintln!("ccview-rs host: could not rebind host.sock: {error}"),
            }
        }
        if !pid_file_is_ours(dir) && write_pid(dir).is_ok() {
            eprintln!("ccview-rs host: host.pid was missing or replaced, rewrote it");
            repaired = true;
        }
        if repaired {
            Heal::Repaired
        } else {
            Heal::Healthy
        }
    }

    pub(super) fn release(&self, dir: &RuntimeDir) {
        if id_of_path(&dir.socket()) == self.socket {
            let _ = fs::remove_file(dir.socket());
        }
        if pid_file_is_ours(dir) {
            let _ = fs::remove_file(dir.pid_file());
        }
    }
}

fn write_pid(dir: &RuntimeDir) -> io::Result<()> {
    fs::write(dir.pid_file(), std::process::id().to_string())
}

fn pid_file_is_ours(dir: &RuntimeDir) -> bool {
    fs::read_to_string(dir.pid_file()).is_ok_and(|text| text == std::process::id().to_string())
}
