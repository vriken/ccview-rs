use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const ESC_GAP: Duration = Duration::from_millis(400);
const SESSION_ID: &str = "44444444-dddd-4ddd-8ddd-dddddddddddd";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    child_stdin: PathBuf,
    fake_claude: PathBuf,
}

fn fixture(name: &str) -> Fixture {
    let root = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("ccview-settings-pty-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, config) = (root.join("home"), root.join("config"));
    let work = home.join("work/app");
    fs::create_dir_all(&work).unwrap();
    let project = home.join(".claude/projects/-work-app");
    fs::create_dir_all(&project).unwrap();
    let lines = [
        format!(
            r#"{{"type":"user","cwd":"{}","message":{{"content":"list the files"}}}}"#,
            work.display()
        ),
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"found two files"}]}}"#
            .to_owned(),
    ];
    fs::write(
        project.join(format!("{SESSION_ID}.jsonl")),
        lines.join("\n") + "\n",
    )
    .unwrap();
    let child_stdin = root.join("child-stdin.bin");
    let fake_claude = root.join("fake-claude");
    fs::write(
        &fake_claude,
        format!(
            "#!/bin/sh\necho $$ > {}\nstty raw -echo\nprintf 'fake child ready'\nexec cat > {}\n",
            root.join("child.pid").display(),
            child_stdin.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
    Fixture {
        root,
        home,
        config,
        child_stdin,
        fake_claude,
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        stop_child(self);
    }
}

fn stop_child(fixture: &Fixture) {
    if let Ok(pid) = fs::read_to_string(fixture.root.join("child.pid")) {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid.trim()])
            .status();
    }
}

fn start(fixture: &Fixture, rows: u16, cols: u16) -> PtyHost {
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", fixture.home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", fixture.config.to_str().unwrap())
        .env(
            "XDG_CACHE_HOME",
            fixture.root.join("cache").to_str().unwrap(),
        )
        .env(
            "CCVIEW_RUNTIME_DIR",
            fixture.root.join("run").to_str().unwrap(),
        )
        .env("CCVIEW_HOST_IDLE_MS", "500")
        .env("CCVIEW_CLAUDE_BIN", fixture.fake_claude.to_str().unwrap())
        .env("TERM", "xterm-256color");
    PtyHost::spawn(&spec, Size { rows, cols }).unwrap()
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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn settings_survive_a_restart() {
    let fixture = fixture("persist");
    let first = start(&fixture, 35, 120);
    wait_for(&first, "found two files");
    first.write(b",").unwrap();
    wait_for(&first, "Identity colours");
    first.write(b"\x1b[B\x1b[B\x1b[C").unwrap();
    wait_for(&first, "everything");
    first.write(b"\x1b[B\r").unwrap();
    wait_for(&first, &format!("{:<28}off", "Mouse capture"));
    first.write(b"\x1b").unwrap();
    wait_for(&first, "view: everything");
    first.kill();
    assert!(read(&fixture.config.join("ccview-rs/view.json")).contains("everything"));
    assert!(read(&fixture.config.join("ccview-rs/ui.json")).contains("\"mouse\": false"));

    let second = start(&fixture, 35, 120);
    wait_for(&second, "view: everything");
    second.write(b",").unwrap();
    wait_for(&second, &format!("{:<28}off", "Mouse capture"));
    second.kill();
    stop_child(&fixture);
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn overlay_keys_never_reach_an_embedded_child() {
    let fixture = fixture("child");
    let host = start(&fixture, 35, 120);
    wait_for(&host, "found two files");
    host.write(b"r").unwrap();
    wait_for(&host, "fake child ready");
    host.write(b"\0,").unwrap();
    wait_for(&host, "Identity colours");
    host.write(b"qxzabc\r").unwrap();
    host.write(b"\x1b").unwrap();
    std::thread::sleep(ESC_GAP);
    host.write(b"Y").unwrap();
    let started = Instant::now();
    while read(&fixture.child_stdin) != "Y" {
        assert!(
            started.elapsed() < TIMEOUT,
            "child saw {:?}",
            read(&fixture.child_stdin)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    host.write(b"\0?").unwrap();
    wait_for(&host, "List rows");
    host.write(b"\x1b").unwrap();
    std::thread::sleep(ESC_GAP);
    host.write(b"W").unwrap();
    let started = Instant::now();
    while read(&fixture.child_stdin) != "YW" {
        assert!(
            started.elapsed() < TIMEOUT,
            "child saw {:?}",
            read(&fixture.child_stdin)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    host.kill();
    stop_child(&fixture);
    fs::remove_dir_all(&fixture.root).unwrap();
}
