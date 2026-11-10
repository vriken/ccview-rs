use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_ID: &str = "33333333-cccc-4ccc-8ccc-cccccccccccc";

fn write_session(home: &Path) {
    let project = home.join(".claude/projects/-work-app");
    fs::create_dir_all(&project).unwrap();
    let lines = [
        r#"{"type":"user","cwd":"/work/app","message":{"content":"list the files"}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"alpha\nbeta"}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"found two files"}]}}"#,
    ];
    fs::write(
        project.join(format!("{SESSION_ID}.jsonl")),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

fn start(home: &Path, config: &Path) -> PtyHost {
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", config.to_str().unwrap())
        .env("CCVIEW_CLAUDE_BIN", "/usr/bin/false")
        .env("CCVIEW_RUNTIME_DIR", home.join("run").to_str().unwrap())
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

#[test]
fn v_cycles_the_view_level_and_the_choice_survives_a_restart() {
    let root = std::env::temp_dir().join(format!("ccview-view-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, config) = (root.join("home"), root.join("config"));
    write_session(&home);

    let first = start(&home, &config);
    wait_for(&first, "found two files");
    wait_for(&first, "view: chat");
    assert!(!first.snapshot().text().contains("Bash(ls -la)"));
    first.write(b"v").unwrap();
    wait_for(&first, "view: tools");
    wait_for(&first, "Bash(ls -la)");
    wait_for(&first, "⎿  alpha");
    first.write(b"v").unwrap();
    wait_for(&first, "view: everything");
    first.write(b"v").unwrap();
    wait_for(&first, "view: chat");
    first.write(b"v").unwrap();
    wait_for(&first, "view: tools");
    first.kill();

    let second = start(&home, &config);
    wait_for(&second, "view: tools");
    wait_for(&second, "Bash(ls -la)");
    second.kill();
    fs::remove_dir_all(root).unwrap();
}
