mod common;

use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use common::{RuntimeGuard, alive, wait_until};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const PREFIX: &[u8] = b"\0";
const FAKE_HARNESS: &str = "#!/bin/sh\n\
[ \"$1\" = agents ] && exit 0\n\
trap 'echo \"SIZE $(stty size)\"' WINCH\n\
echo \"FAKE-PID $$\"\n\
while IFS= read -r line; do\n\
  echo \"echo: $line\"\n\
  [ \"$line\" = bye ] && exit 0\n\
done\n";

struct Fixture {
    root: std::path::PathBuf,
    runtime: RuntimeGuard,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture(name: &str) -> Fixture {
    let root = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("ccview-hostpty-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("home")).unwrap();
    fs::create_dir_all(root.join("config/ccview-rs")).unwrap();
    fs::write(
        root.join("config/ccview-rs/ui.json"),
        r#"{"sessions_strip": true}"#,
    )
    .unwrap();
    let fake = root.join("fake-claude");
    fs::write(&fake, FAKE_HARNESS).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    Fixture {
        root,
        runtime: RuntimeGuard::new(name),
    }
}

fn start(fixture: &Fixture) -> PtyHost {
    let root = &fixture.root;
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .cwd(root)
        .env("HOME", root.join("home").to_str().unwrap())
        .env("XDG_CONFIG_HOME", root.join("config").to_str().unwrap())
        .env("XDG_CACHE_HOME", root.join("cache").to_str().unwrap())
        .env(
            "CCVIEW_RUNTIME_DIR",
            fixture.runtime.path().to_str().unwrap(),
        )
        .env("CCVIEW_HOST_IDLE_MS", "1500")
        .env(
            "CCVIEW_CLAUDE_BIN",
            root.join("fake-claude").to_str().unwrap(),
        )
        .env("CCVIEW_NO_SELF_UPDATE", "1")
        .env("TERM", "xterm-256color");
    PtyHost::spawn(
        &spec,
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

fn fake_pid(host: &PtyHost) -> u32 {
    let text = host.snapshot().text();
    let after = text.split("FAKE-PID ").nth(1).expect("pid line on screen");
    after
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn processes_mentioning(marker: &Path) -> Vec<String> {
    let listing = Command::new("ps")
        .args(["-axo", "pid,command"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|line| line.contains(marker.to_str().unwrap()) && !line.contains("ps -axo"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_session_started_in_one_ccview_survives_quitting_and_is_picked_up_by_the_next() {
    let fx = fixture("outlive");
    let first = start(&fx);
    wait_for(&first, "No sessions.");
    first.write(b"n").unwrap();
    wait_for(&first, "FAKE-PID");
    let pid = fake_pid(&first);
    first.write(b"hello\r").unwrap();
    wait_for(&first, "echo: hello");

    first.write(PREFIX).unwrap();
    first.write(b"l").unwrap();
    first.write(b"q").unwrap();
    let status = first
        .wait_for_exit(TIMEOUT)
        .expect("ccview quits without asking anything");
    assert!(status.success(), "{status:?}");
    assert!(alive(pid), "the session must outlive ccview");
    assert_eq!(fx.runtime.session_pids(), vec![pid]);
    let daemon = fx.runtime.daemon_pid().expect("daemon pid file");
    assert!(alive(daemon));

    let second = start(&fx);
    wait_for(&second, "claude new");
    second.write(b"1").unwrap();
    wait_for(&second, "echo: hello");
    assert_eq!(fake_pid(&second), pid);

    second.write(b"again\r").unwrap();
    wait_for(&second, "echo: again");
    second.resize(28, 100).unwrap();
    wait_for(&second, "SIZE ");

    second.write(b"bye\r").unwrap();
    wait_until("the harness to exit", || !alive(pid));
    wait_for(&second, "session ended");
    second.write(b"q").unwrap();
    assert!(
        second
            .wait_for_exit(TIMEOUT)
            .expect("second ccview quits")
            .success()
    );

    wait_until("the idle daemon to exit", || !alive(daemon));
    assert!(!fx.runtime.path().join("host.sock").exists());
    assert_eq!(processes_mentioning(&fx.root), Vec::<String>::new());
}

#[test]
fn ctrl_c_on_the_list_quits_at_once_even_with_a_running_session() {
    let fx = fixture("ctrlc");
    let app = start(&fx);
    wait_for(&app, "No sessions.");
    app.write(b"n").unwrap();
    wait_for(&app, "FAKE-PID");
    let pid = fake_pid(&app);
    app.write(PREFIX).unwrap();
    app.write(b"l").unwrap();
    app.write(b"\x03").unwrap();
    assert!(app.wait_for_exit(TIMEOUT).expect("ccview quits").success());
    assert!(alive(pid));
}

#[test]
fn detaching_keeps_the_session_running() {
    let fx = fixture("detach");
    let app = start(&fx);
    wait_for(&app, "No sessions.");
    app.write(b"n").unwrap();
    wait_for(&app, "FAKE-PID");
    let pid = fake_pid(&app);
    app.write(PREFIX).unwrap();
    app.write(b"x").unwrap();
    wait_for(&app, "detached");
    assert!(alive(pid));
    app.write(b"q").unwrap();
    assert!(app.wait_for_exit(TIMEOUT).expect("ccview quits").success());
    assert!(alive(pid));
}
