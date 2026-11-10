use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const FAST: Duration = Duration::from_millis(1500);
const INTERACTIVE_ID: &str = "44444444-dddd-4ddd-8ddd-dddddddddddd";
const BACKGROUND_ID: &str = "55555555-eeee-4eee-8eee-eeeeeeeeeeee";

fn write_transcript(home: &Path, session_id: &str, title: &str) {
    let project = home.join(".claude/projects/-work-app");
    fs::create_dir_all(&project).unwrap();
    let name =
        format!(r#"{{"type":"custom-title","customTitle":"{title}","sessionId":"{session_id}"}}"#);
    let user = r#"{"type":"user","cwd":"/work/app","message":{"content":"hello"}}"#;
    fs::write(
        project.join(format!("{session_id}.jsonl")),
        format!("{user}\n{name}\n"),
    )
    .unwrap();
}

fn write_registry(home: &Path, status: &str) {
    let sessions = home.join(".claude/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let pid = std::process::id();
    let json = format!(
        r#"{{"pid":{pid},"sessionId":"{INTERACTIVE_ID}","status":"{status}","kind":"interactive","cwd":"/work/app","updatedAt":1}}"#
    );
    let staged = sessions.join("staged.tmp");
    fs::write(&staged, json).unwrap();
    fs::rename(staged, sessions.join(format!("{pid}.json"))).unwrap();
}

fn write_background_job(home: &Path, state: &str) {
    let cli = format!(
        r#"[{{"id":"b0b0b0b0","sessionId":"{BACKGROUND_ID}","kind":"background","state":"{state}","status":"idle"}}]"#
    );
    fs::write(home.join("cli.json"), cli).unwrap();
    let job = home.join(".claude/jobs/b0b0b0b0");
    fs::create_dir_all(&job).unwrap();
    fs::write(
        job.join("state.json"),
        format!(r#"{{"state":"working","tempo":"{state}"}}"#),
    )
    .unwrap();
}

fn wait_for(host: &PtyHost, needle: &str) -> Duration {
    let started = Instant::now();
    let mut seen = 0;
    while !host.snapshot().text().contains(needle) {
        assert!(
            started.elapsed() < TIMEOUT,
            "timed out waiting for {needle:?}; screen:\n{}",
            host.snapshot().text()
        );
        seen = host.wait_for_change(seen, Duration::from_millis(5));
    }
    started.elapsed()
}

fn wait_until_gone(host: &PtyHost, needle: &str) {
    let started = Instant::now();
    let mut seen = 0;
    while host.snapshot().text().contains(needle) {
        assert!(started.elapsed() < TIMEOUT, "{needle:?} never went away");
        seen = host.wait_for_change(seen, Duration::from_millis(5));
    }
}

#[test]
fn needs_you_reaches_the_screen_within_a_second_for_a_waiting_interactive_session_and_a_blocked_background_job()
 {
    let home = std::env::temp_dir().join(format!("ccview-live-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    write_transcript(&home, INTERACTIVE_ID, "interactive one");
    write_transcript(&home, BACKGROUND_ID, "background one");
    write_registry(&home, "busy");
    write_background_job(&home, "working");
    let shim = home.join("fake-claude");
    fs::write(
        &shim,
        format!("#!/bin/sh\ncat '{}'\n", home.join("cli.json").display()),
    )
    .unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&shim)
        .status()
        .unwrap();

    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", home.to_str().unwrap())
        .env("CCVIEW_CLAUDE_BIN", shim.to_str().unwrap())
        .env("CCVIEW_RUNTIME_DIR", home.join("run").to_str().unwrap())
        .env("TERM", "xterm-256color");
    let host = PtyHost::spawn(
        &spec,
        Size {
            rows: 35,
            cols: 120,
        },
    )
    .unwrap();
    wait_for(&host, "1 busy");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!host.snapshot().text().contains("needs you"));

    write_registry(&home, "waiting");
    let waiting = wait_for(&host, "1 needs you");
    write_registry(&home, "idle");
    wait_until_gone(&host, "needs you");
    write_background_job(&home, "blocked");
    let blocked = wait_for(&host, "1 needs you");
    eprintln!("waiting shown after {waiting:?}; blocked background shown after {blocked:?}");
    assert!(waiting < FAST, "waiting took {waiting:?}");
    assert!(blocked < FAST, "blocked took {blocked:?}");

    host.kill();
    fs::remove_dir_all(home).unwrap();
}
