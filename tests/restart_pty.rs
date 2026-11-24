mod common;

use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use common::{RuntimeGuard, alive, wait_until};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const PREFIX: &[u8] = b"\0";
const SESSION_ID: &str = "44444444-dddd-4ddd-8ddd-dddddddddddd";

struct Fixture {
    home: PathBuf,
    work: PathBuf,
    log: PathBuf,
    runtime: RuntimeGuard,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.stop();
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn fixture(name: &str, status: &str) -> Fixture {
    let home = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("ccview-restart-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    let work = home.join("work/app");
    let project = home.join(".claude/projects/-work-app");
    let sessions = home.join(".claude/sessions");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&sessions).unwrap();
    fs::write(
        project.join(format!("{SESSION_ID}.jsonl")),
        format!(
            "{{\"type\":\"user\",\"cwd\":\"{}\",\"message\":{{\"content\":\"restart fixture prompt\"}}}}\n",
            work.display()
        ),
    )
    .unwrap();
    let log = home.join("launch.log");
    let fake = home.join("fake-claude");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\n\
[ \"$1\" = agents ] && exit 0\n\
echo \"START $$ $PWD $*\" >> '{log}'\n\
printf '{{\"pid\":%s,\"sessionId\":\"{SESSION_ID}\",\"status\":\"{status}\",\"kind\":\"interactive\"}}' $$ > '{sessions}/'$$.json\n\
trap 'echo \"TERM $$\" >> \"{log}\"; exit 143' TERM\n\
echo \"FAKE-PID $$\"\n\
while :; do sleep 1 & wait $!; done\n",
            log = log.display(),
            sessions = sessions.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    Fixture {
        home,
        work,
        log,
        runtime: RuntimeGuard::new(name),
    }
}

fn start(fixture: &Fixture) -> PtyHost {
    let home = &fixture.home;
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .cwd(home)
        .env("HOME", home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", home.join("config").to_str().unwrap())
        .env("XDG_CACHE_HOME", home.join("cache").to_str().unwrap())
        .env(
            "CCVIEW_RUNTIME_DIR",
            fixture.runtime.path().to_str().unwrap(),
        )
        .env(
            "CCVIEW_CLAUDE_BIN",
            home.join("fake-claude").to_str().unwrap(),
        )
        .env("CCVIEW_NO_SELF_UPDATE", "1")
        .env("TERM", "xterm-256color");
    PtyHost::spawn(
        &common::isolate_harness_dirs(spec, home),
        Size {
            rows: 35,
            cols: 120,
        },
    )
    .unwrap()
}

fn wait_for(host: &PtyHost, needle: &str) {
    let started = Instant::now();
    let mut seen = 0;
    while !host.snapshot().text().contains(needle) {
        assert!(
            started.elapsed() < TIMEOUT,
            "timed out waiting for {needle:?}; screen:\n{}",
            host.snapshot().text()
        );
        seen = host.wait_for_change(seen, Duration::from_millis(50));
    }
}

fn log_lines(log: &Path, kind: &str) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with(kind))
        .map(str::to_owned)
        .collect()
}

fn pid_of(line: &str) -> u32 {
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

fn resume_running_session(fixture: &Fixture) -> (PtyHost, u32) {
    let app = start(fixture);
    wait_for(&app, "restart fixture prompt");
    app.write(b"r").unwrap();
    wait_for(&app, "FAKE-PID");
    wait_until("the fake to register its session", || {
        log_lines(&fixture.log, "START").len() == 1
    });
    let first = log_lines(&fixture.log, "START").remove(0);
    (app, pid_of(&first))
}

#[test]
fn restarting_an_idle_session_stops_its_process_and_resumes_the_same_session_in_its_directory() {
    let fx = fixture("idle", "idle");
    let (app, first_pid) = resume_running_session(&fx);

    app.write(PREFIX).unwrap();
    app.write(b"l").unwrap();
    app.write(b"\x12").unwrap();
    wait_for(&app, "? y/n");
    app.write(b"y").unwrap();

    wait_until("a second launch", || log_lines(&fx.log, "START").len() == 2);
    assert_eq!(
        log_lines(&fx.log, "TERM"),
        vec![format!("TERM {first_pid}")]
    );
    assert!(!alive(first_pid), "the old process must be gone");
    let second = log_lines(&fx.log, "START").remove(1);
    assert_ne!(pid_of(&second), first_pid);
    assert!(
        second.ends_with(&format!("{} --resume {SESSION_ID}", fx.work.display())),
        "{second}"
    );
    app.kill();
}

#[test]
fn a_busy_session_is_not_restarted() {
    let fx = fixture("busy", "busy");
    let (app, first_pid) = resume_running_session(&fx);
    wait_for(&app, "busy");

    app.write(PREFIX).unwrap();
    app.write(b"r").unwrap();
    wait_for(&app, "busy, restart when idle");
    app.write(b"y").unwrap();
    std::thread::sleep(Duration::from_millis(1500));

    assert!(alive(first_pid));
    assert_eq!(log_lines(&fx.log, "TERM"), Vec::<String>::new());
    assert_eq!(log_lines(&fx.log, "START").len(), 1);
    app.kill();
}
