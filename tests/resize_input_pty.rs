mod common;

use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const HELP_TITLE: &str = "Help";
const ROUNDS: u16 = 30;
const KEY_DEADLINE: Duration = Duration::from_secs(1);
const TIMEOUT: Duration = Duration::from_secs(15);

fn start(home: &Path) -> PtyHost {
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", home.to_str().unwrap())
        .env("XDG_CONFIG_HOME", home.join("config").to_str().unwrap())
        .env("XDG_CACHE_HOME", home.join("cache").to_str().unwrap())
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

fn screen_within(host: &PtyHost, within: Duration, wanted: impl Fn(&str) -> bool) -> bool {
    let started = Instant::now();
    let mut seen = 0;
    loop {
        if wanted(&host.snapshot().text()) {
            return true;
        }
        if started.elapsed() >= within {
            return false;
        }
        seen = host.wait_for_change(seen, Duration::from_millis(20));
    }
}

#[test]
fn a_key_typed_while_the_window_is_resized_is_read_without_another_key() {
    let home = std::env::temp_dir().join(format!("ccview-resize-input-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).unwrap();
    let app = start(&home);
    assert!(screen_within(&app, TIMEOUT, |text| text
        .contains("No sessions.")
        && !text.contains(HELP_TITLE)));

    let mut stalled = 0;
    for round in 0..ROUNDS {
        let shrink = round % 2;
        app.resize(35 - shrink, 120 - shrink).unwrap();
        app.write(b"?").unwrap();
        if !screen_within(&app, KEY_DEADLINE, |text| text.contains(HELP_TITLE)) {
            stalled += 1;
        }
        app.write(b"\x1b").unwrap();
        assert!(screen_within(&app, TIMEOUT, |text| !text.contains(HELP_TITLE)));
    }

    app.write(b"q").unwrap();
    assert!(app.wait_for_exit(TIMEOUT).is_some());
    let _ = fs::remove_dir_all(&home);
    assert_eq!(
        stalled, 0,
        "keys typed during a resize waited for another key in {stalled} of {ROUNDS} rounds"
    );
}
