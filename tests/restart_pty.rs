mod common;

use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use common::{RuntimeGuard, alive, wait_until};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
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
        .env("CCVIEW_AUTO_RESTART_DELAY_MS", "1500")
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

#[test]
fn a_session_whose_directory_is_gone_is_not_stopped() {
    let fx = fixture("gone", "idle");
    let (app, first_pid) = resume_running_session(&fx);

    app.write(PREFIX).unwrap();
    app.write(b"l").unwrap();
    app.write(b"\x12").unwrap();
    wait_for(&app, "? y/n");
    fs::remove_dir_all(&fx.work).unwrap();
    app.write(b"y").unwrap();

    wait_for(&app, "directory not found");
    assert!(alive(first_pid));
    assert_eq!(log_lines(&fx.log, "TERM"), Vec::<String>::new());
    app.kill();
}

fn back_to_list(app: &PtyHost) {
    app.write(PREFIX).unwrap();
    app.write(b"l").unwrap();
}

fn change_claude_config(fixture: &Fixture) {
    std::thread::sleep(Duration::from_millis(1600));
    write_claude_config(fixture, SystemTime::now());
}

fn write_claude_config(fixture: &Fixture, modified: SystemTime) {
    let path = fixture.home.join(".claude/CLAUDE.md");
    fs::write(&path, "changed\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

#[test]
fn a_config_written_while_the_session_starts_or_dated_in_the_future_is_no_reason_to_restart() {
    let fx = fixture("early", "idle");
    let (app, first_pid) = resume_running_session(&fx);
    back_to_list(&app);
    write_claude_config(&fx, SystemTime::now());
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(
        log_lines(&fx.log, "TERM"),
        Vec::<String>::new(),
        "Claude rewrites its plugin list as it starts"
    );

    write_claude_config(&fx, SystemTime::now() + Duration::from_secs(86_400));
    std::thread::sleep(Duration::from_secs(4));
    assert!(alive(first_pid));
    assert_eq!(log_lines(&fx.log, "TERM"), Vec::<String>::new());
    app.kill();
}

fn append_to_transcript(fixture: &Fixture, line: &str) {
    let path = fixture
        .home
        .join(format!(".claude/projects/-work-app/{SESSION_ID}.jsonl"));
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(line);
    text.push('\n');
    fs::write(path, text).unwrap();
}

#[test]
fn an_idle_session_restarts_by_itself_after_a_config_change_once_the_list_is_focused() {
    let fx = fixture("auto", "idle");
    change_claude_config(&fx);
    let (app, first_pid) = resume_running_session(&fx);
    back_to_list(&app);
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(
        log_lines(&fx.log, "START").len(),
        1,
        "a config older than the session is not a reason to restart"
    );

    app.write(b"r").unwrap();
    wait_for(&app, "FAKE-PID");
    change_claude_config(&fx);
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(
        log_lines(&fx.log, "TERM"),
        Vec::<String>::new(),
        "never while the owner is inside the session"
    );

    back_to_list(&app);
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        log_lines(&fx.log, "TERM"),
        Vec::<String>::new(),
        "a session that has just gone quiet is left alone for the whole delay"
    );
    wait_until("an automatic second launch", || {
        log_lines(&fx.log, "START").len() == 2
    });
    assert_eq!(
        log_lines(&fx.log, "TERM"),
        vec![format!("TERM {first_pid}")]
    );
    let second = log_lines(&fx.log, "START").remove(1);
    assert!(
        second.ends_with(&format!("--resume {SESSION_ID}")),
        "{second}"
    );
    app.kill();
}

#[test]
fn a_session_with_an_unfinished_background_task_waits_until_the_task_ends() {
    let fx = fixture("pending", "idle");
    let (app, first_pid) = resume_running_session(&fx);
    append_to_transcript(
        &fx,
        r#"{"type":"assistant","timestamp":"2099-01-01T00:00:00.000Z","message":{"content":[{"type":"tool_use","id":"toolu_bg1","name":"Bash","input":{"command":"sleep 600","run_in_background":true}}]}}"#,
    );
    back_to_list(&app);
    change_claude_config(&fx);
    std::thread::sleep(Duration::from_secs(5));
    assert!(alive(first_pid));
    assert_eq!(log_lines(&fx.log, "TERM"), Vec::<String>::new());

    append_to_transcript(
        &fx,
        r#"{"type":"queue-operation","operation":"enqueue","content":"<task-notification><task-id>b1</task-id><tool-use-id>toolu_bg1</tool-use-id><status>completed</status></task-notification>"}"#,
    );
    wait_until("the restart once the task has ended", || {
        log_lines(&fx.log, "START").len() == 2
    });
    assert_eq!(
        log_lines(&fx.log, "TERM"),
        vec![format!("TERM {first_pid}")]
    );
    app.kill();
}
