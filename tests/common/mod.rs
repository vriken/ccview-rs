#![allow(dead_code)]

use ccview_rs::terminal::CommandSpec;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub struct RuntimeGuard {
    dir: PathBuf,
}

pub fn isolate_harness_dirs(spec: CommandSpec, home: &Path) -> CommandSpec {
    spec.env("CLAUDE_CONFIG_DIR", home.join(".claude").to_str().unwrap())
        .env("CODEX_HOME", home.join(".codex").to_str().unwrap())
        .env("XDG_DATA_HOME", home.join(".local/share").to_str().unwrap())
        .env(
            "XDG_STATE_HOME",
            home.join(".local/state").to_str().unwrap(),
        )
}

pub fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl RuntimeGuard {
    pub fn new(name: &str) -> Self {
        let dir = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("ccview-run-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn daemon_pid(&self) -> Option<u32> {
        fs::read_to_string(self.dir.join("host.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    pub fn session_pids(&self) -> Vec<u32> {
        let listing = Command::new(env!("CARGO_BIN_EXE_ccview-rs"))
            .args(["host", "--list"])
            .env("CCVIEW_RUNTIME_DIR", &self.dir)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        listing
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                words.find(|word| *word == "pid")?;
                words.next()?.parse().ok()
            })
            .collect()
    }

    pub fn stop(&self) {
        let sessions = self.session_pids();
        let daemon = self.daemon_pid();
        for pid in sessions.into_iter().filter(|pid| *pid > 1) {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        if let Some(pid) = daemon.filter(|pid| *pid > 1) {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        self.stop();
    }
}
