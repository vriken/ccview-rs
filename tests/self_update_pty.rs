mod common;

use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use common::RuntimeGuard;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(60);
const SETTLE: Duration = Duration::from_secs(7);
const SESSION_ID: &str = "55555555-eeee-4eee-8eee-eeeeeeeeeeee";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    installed: PathBuf,
    fake_claude: PathBuf,
    runtime: RuntimeGuard,
}

fn fixture() -> Fixture {
    let root = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("ccview-self-update-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
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
    let fake_claude = root.join("fake-claude");
    fs::write(&fake_claude, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
    let installed = root.join("bin/ccview-rs");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_ccview-rs"), &installed).unwrap();
    Fixture {
        root,
        home,
        installed,
        fake_claude,
        runtime: RuntimeGuard::new("self-update"),
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn install(fixture: &Fixture, content: &[u8], mode: u32) {
    let staged = fixture.installed.with_extension("staged");
    fs::write(&staged, content).unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(mode)).unwrap();
    fs::rename(&staged, &fixture.installed).unwrap();
}

fn start(fixture: &Fixture) -> PtyHost {
    let spec = CommandSpec::new(fixture.installed.to_str().unwrap())
        .env("HOME", fixture.home.to_str().unwrap())
        .env(
            "XDG_CONFIG_HOME",
            fixture.root.join("config").to_str().unwrap(),
        )
        .env(
            "XDG_CACHE_HOME",
            fixture.root.join("cache").to_str().unwrap(),
        )
        .env(
            "CCVIEW_RUNTIME_DIR",
            fixture.runtime.path().to_str().unwrap(),
        )
        .env("CCVIEW_HOST_IDLE_MS", "500")
        .env("CCVIEW_CLAUDE_BIN", fixture.fake_claude.to_str().unwrap())
        .env("TERM", "xterm-256color");
    let spec = common::isolate_harness_dirs(spec, &fixture.home);
    PtyHost::spawn(
        &spec,
        Size {
            rows: 30,
            cols: 100,
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

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn a_new_build_on_disk_restarts_the_ui_in_place_but_a_broken_one_does_not() {
    let fixture = fixture();
    let host = start(&fixture);
    wait_for(&host, "list the files");
    host.write(b"\r").unwrap();
    wait_for(&host, "found two files");

    install(&fixture, b"", 0o755);
    std::thread::sleep(SETTLE);
    assert!(
        host.snapshot().text().contains("found two files"),
        "an empty binary must not restart the UI"
    );

    let marker = fixture.root.join("marker");
    let state_copy = fixture.root.join("state-copy");
    let script = format!(
        "#!/bin/sh\nprintf '%s|%s' \"$#\" \"$0\" > {marker}\ncp \"$XDG_STATE_HOME/ccview-rs/ui_state.json\" {copy}\nprintf REEXECED\nsleep 2\n",
        marker = marker.display(),
        copy = state_copy.display()
    );
    install(&fixture, script.as_bytes(), 0o755);
    wait_for(&host, "REEXECED");

    assert_eq!(
        read(&marker),
        format!("0|{}", fixture.installed.display()),
        "same path and arguments"
    );
    let state = read(&state_copy);
    assert!(
        state.contains(SESSION_ID) && state.contains("transcript"),
        "state written before the restart: {state}"
    );
    assert!(
        !host.snapshot().text().contains("found two files"),
        "terminal left the UI screen before the exec"
    );
}
