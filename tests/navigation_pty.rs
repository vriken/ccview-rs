use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";

fn write_session(home: &Path, project: &str, cwd: &str, id: &str, prompt: &str) {
    let dir = home.join(".claude/projects").join(project);
    fs::create_dir_all(&dir).unwrap();
    let lines = [
        format!(r#"{{"type":"user","cwd":"{cwd}","message":{{"content":"{prompt}"}}}}"#),
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#.to_owned(),
    ];
    fs::write(dir.join(format!("{id}.jsonl")), lines.join("\n") + "\n").unwrap();
}

fn start(home: &Path, config: &Path) -> PtyHost {
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", config.to_str().unwrap())
        .env("XDG_CACHE_HOME", config.join("cache").to_str().unwrap())
        .env("CCVIEW_CLAUDE_BIN", "/usr/bin/false")
        .env("TERM", "xterm-256color");
    PtyHost::spawn(&spec, Size { rows: 35, cols: 120 }).unwrap()
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

fn fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("ccview-nav-pty-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, config) = (root.join("home"), root.join("config"));
    write_session(&home, "-work-alpha", "/work/alpha", "11111111-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "alpha task");
    write_session(&home, "-work-beta", "/work/beta", "22222222-bbbb-4bbb-8bbb-bbbbbbbbbbbb", "beta task");
    (root, home, config)
}

#[test]
fn up_reaches_the_tab_bar_and_right_opens_the_session_from_the_rows() {
    let (root, home, config) = fixture("walk");
    let app = start(&home, &config);
    wait_for(&app, "alpha task");
    wait_for(&app, "↑/↓ move");
    app.write(UP).unwrap();
    wait_for(&app, "↓ rows");
    wait_for(&app, "▶All");
    for _ in 0..3 {
        app.write(RIGHT).unwrap();
    }
    wait_for(&app, "▶beta");
    app.write(DOWN).unwrap();
    wait_for(&app, "→ open");
    app.write(RIGHT).unwrap();
    wait_for(&app, "TRANSCRIPT · claude · beta task");
    wait_for(&app, "done");
    app.write(LEFT).unwrap();
    wait_for(&app, "→ open");
    app.kill();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_hidden_session_stays_hidden_after_a_restart_and_h_shows_it_again() {
    let (root, home, config) = fixture("hide");
    let first = start(&home, &config);
    wait_for(&first, "alpha task");
    wait_for(&first, "beta task");
    first.write(b"x").unwrap();
    wait_for(&first, "1 hidden · H show");
    wait_for(&first, "TRANSCRIPT · claude · alpha task");
    assert!(!first.snapshot().text().contains("beta task"));
    first.kill();

    let second = start(&home, &config);
    wait_for(&second, "1 hidden · H show");
    wait_for(&second, "TRANSCRIPT · claude · alpha task");
    assert!(!second.snapshot().text().contains("beta task"));
    second.write(b"H").unwrap();
    wait_for(&second, "1 hidden · H hide");
    wait_for(&second, "beta task");
    wait_for(&second, "hidden · beta");
    second.kill();
    fs::remove_dir_all(root).unwrap();
}
