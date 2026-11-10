use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const BLOCKED_ID: &str = "11111111-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const AGENTS_ID: &str = "22222222-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

fn write_transcript(home: &Path, session_id: &str, title: &str) {
    let project = home.join(".claude/projects/-work-app");
    fs::create_dir_all(&project).unwrap();
    let line =
        format!(r#"{{"type":"custom-title","customTitle":"{title}","sessionId":"{session_id}"}}"#);
    let user = format!(r#"{{"type":"user","cwd":"/work/app","message":{{"content":"hello"}}}}"#);
    fs::write(
        project.join(format!("{session_id}.jsonl")),
        format!("{user}\n{line}\n"),
    )
    .unwrap();
}

fn write_registry(home: &Path, session_id: &str, status: &str, kind: &str) {
    let sessions = home.join(".claude/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let pid = std::process::id();
    let json = format!(
        r#"{{"pid":{pid},"sessionId":"{session_id}","status":"{status}","kind":"{kind}","cwd":"/work/app","updatedAt":1}}"#
    );
    fs::write(sessions.join(format!("{pid}-{session_id}.json")), json).unwrap();
}

fn write_running_agent(home: &Path, session_id: &str, agent_id: &str) {
    let dir = home.join(format!(".claude/projects/-work-app/{session_id}/subagents"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("agent-{agent_id}.meta.json")),
        r#"{"agentType":"explore","description":"scan"}"#,
    )
    .unwrap();
    fs::write(
        dir.join(format!("agent-{agent_id}.jsonl")),
        "{\"type\":\"user\"}\n",
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
        seen = host.wait_for_change(seen, Duration::from_millis(50));
    }
    started.elapsed()
}

#[test]
fn blocked_and_agent_badges_reach_the_screen_through_a_fake_claude_agents_json() {
    let home = std::env::temp_dir().join(format!("ccview-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    write_transcript(&home, BLOCKED_ID, "needs a decision");
    write_transcript(&home, AGENTS_ID, "fans out work");
    write_registry(&home, BLOCKED_ID, "idle", "bg");
    write_registry(&home, AGENTS_ID, "busy", "interactive");
    write_running_agent(&home, AGENTS_ID, "a1");
    write_running_agent(&home, AGENTS_ID, "a2");
    let shim = home.join("fake-claude");
    fs::write(
        &shim,
        format!(
            "#!/bin/sh\ncat <<'JSON'\n[{{\"id\":\"11111111\",\"sessionId\":\"{BLOCKED_ID}\",\"kind\":\"background\",\"state\":\"blocked\",\"status\":\"idle\"}},{{\"sessionId\":\"{AGENTS_ID}\",\"kind\":\"interactive\",\"status\":\"busy\"}}]\nJSON\n"
        ),
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

    let first_badges = wait_for(&host, "needs you");
    wait_for(&host, "⛭ 2");
    wait_for(&host, "1 busy · 1 needs you · 2 agents running");
    wait_for(&host, "Active 2");
    host.write(b"\x1b[A").unwrap();
    host.write(b"\x1b[C").unwrap();
    host.write(b"\x1b[C").unwrap();
    let switched = wait_for(&host, "Active 2 · 2");
    eprintln!("first badges after {first_badges:?}; Active tab after keys {switched:?}");
    let screen = host.snapshot().text();
    assert!(screen.find("needs a decision").unwrap() < screen.find("fans out work").unwrap());

    host.kill();
    fs::remove_dir_all(home).unwrap();
}
