mod common;

use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(20);
const ESC: &[u8] = b"\x1b";
const ENTER: &[u8] = b"\r";

fn write_session(home: &Path, project: &str, cwd: &str, id: &str, lines: &[(&str, &str)]) {
    let dir = home.join(".claude/projects").join(project);
    fs::create_dir_all(&dir).unwrap();
    let body: Vec<String> = lines
        .iter()
        .map(|(role, text)| match *role {
            "user" => format!(
                r#"{{"type":"user","cwd":"{cwd}","message":{{"content":"{text}"}}}}"#
            ),
            _ => format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            ),
        })
        .collect();
    fs::write(dir.join(format!("{id}.jsonl")), body.join("\n") + "\n").unwrap();
}

fn start(home: &Path, config: &Path) -> PtyHost {
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", config.to_str().unwrap())
        .env("XDG_CACHE_HOME", config.join("cache").to_str().unwrap())
        .env("CCVIEW_CLAUDE_BIN", "/usr/bin/false")
        .env("CCVIEW_RUNTIME_DIR", home.join("run").to_str().unwrap())
        .env("CCVIEW_NO_SELF_UPDATE", "1")
        .env("TERM", "xterm-256color");
    let spec = common::isolate_harness_dirs(spec, home);
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

fn wait_until_absent(host: &PtyHost, needle: &str) {
    let started = Instant::now();
    let mut seen = 0;
    while host.snapshot().text().contains(needle) {
        assert!(
            started.elapsed() < TIMEOUT,
            "{needle:?} never left the screen:\n{}",
            host.snapshot().text()
        );
        seen = host.wait_for_change(seen, Duration::from_millis(50));
    }
}

#[test]
fn slash_a_word_and_enter_open_the_session_that_contains_it() {
    let root = std::env::temp_dir().join(format!("ccview-search-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, config) = (root.join("home"), root.join("config"));
    write_session(
        &home,
        "-work-alpha",
        "/work/alpha",
        "11111111-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        &[
            ("user", "alpha task"),
            ("assistant", "the pangolin burrows at night"),
            ("user", "alpha follow up"),
            ("assistant", "quokka closing alpha"),
        ],
    );
    write_session(
        &home,
        "-work-beta",
        "/work/beta",
        "22222222-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        &[("user", "beta task"), ("assistant", "done")],
    );
    let app = start(&home, &config);
    wait_for(&app, "alpha task");
    wait_for(&app, "beta task");

    app.write(b"/").unwrap();
    wait_for(&app, "esc leave");
    app.write(b"pangolin").unwrap();
    wait_for(&app, "1 hit");
    wait_for(&app, "pangolin burrows");
    wait_until_absent(&app, "beta task");

    app.write(ENTER).unwrap();
    wait_for(&app, "←/esc list");
    wait_for(&app, "TRANSCRIPT · claude · alpha task");
    wait_for(&app, "quokka closing alpha");

    app.write(ESC).unwrap();
    wait_for(&app, "esc leave");
    app.write(ESC).unwrap();
    wait_until_absent(&app, "esc leave");
    wait_for(&app, "beta task");
    app.kill();
    fs::remove_dir_all(root).unwrap();
}
