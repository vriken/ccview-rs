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

const HIGHLIGHT_BACKGROUND: vt100::Color = vt100::Color::Idx(110);
const TRANSCRIPT_FIRST_COLUMN: u16 = 46;
const TRAFFIC: usize = 700;

fn write_long_session(home: &Path, project: &str, id: &str) -> std::path::PathBuf {
    let lines: Vec<(String, String)> = (0..TRAFFIC)
        .map(|index| {
            let text = match index {
                20 => "needle one quokka".to_owned(),
                300 => "needle two quokka quokka quokka".to_owned(),
                450 => "needle three quokka".to_owned(),
                _ => format!("filler {index} padding"),
            };
            ("user".to_owned(), text)
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = lines
        .iter()
        .map(|(role, text)| (role.as_str(), text.as_str()))
        .collect();
    write_session(home, project, "/work/long", id, &borrowed);
    home.join(".claude/projects")
        .join(project)
        .join(format!("{id}.jsonl"))
}

fn highlighted_cells(host: &PtyHost) -> usize {
    let snapshot = host.snapshot();
    let screen = snapshot.screen();
    let size = snapshot.size();
    let (rows, cols) = (size.rows, size.cols);
    (2..rows.saturating_sub(3))
        .flat_map(|row| (TRANSCRIPT_FIRST_COLUMN..cols).map(move |col| (row, col)))
        .filter(|&(row, col)| {
            screen
                .cell(row, col)
                .is_some_and(|cell| cell.bgcolor() == HIGHLIGHT_BACKGROUND)
        })
        .count()
}

fn transcript_text(host: &PtyHost) -> String {
    host.snapshot()
        .text()
        .lines()
        .map(|line| {
            line.chars()
                .skip(TRANSCRIPT_FIRST_COLUMN as usize)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for_transcript(host: &PtyHost, needle: &str) {
    let started = Instant::now();
    let mut seen = 0;
    while !transcript_text(host).contains(needle) {
        assert!(
            started.elapsed() < TIMEOUT,
            "timed out waiting for {needle:?} in the transcript; screen:\n{}",
            host.snapshot().text()
        );
        seen = host.wait_for_change(seen, Duration::from_millis(50));
    }
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
    wait_for(&app, "n/N match");
    wait_for(&app, "TRANSCRIPT · claude · alpha task");
    wait_for(&app, "quokka closing alpha");

    app.write(ESC).unwrap();
    wait_for(&app, "←/esc results");
    app.write(ESC).unwrap();
    wait_for(&app, "esc leave");
    app.write(ESC).unwrap();
    wait_until_absent(&app, "esc leave");
    wait_for(&app, "beta task");
    app.kill();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn enter_lands_on_the_best_match_highlighted_and_n_steps_through_the_others() {
    let root = std::env::temp_dir().join(format!("ccview-search-jump-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, config) = (root.join("home"), root.join("config"));
    write_long_session(&home, "-work-long", "33333333-cccc-4ccc-8ccc-cccccccccccc");
    let app = start(&home, &config);
    wait_for(&app, "filler 0 padding");

    app.write(b"/").unwrap();
    wait_for(&app, "esc leave");
    app.write(b"quokka").unwrap();
    wait_for(&app, "3 hits");
    app.write(ENTER).unwrap();

    wait_for_transcript(&app, "needle two");
    assert!(
        !app.snapshot()
            .text()
            .contains(&format!("filler {} padding", TRAFFIC - 1)),
        "landed on the tail instead of the match:\n{}",
        app.snapshot().text()
    );
    assert!(
        highlighted_cells(&app) >= 3,
        "matched words are not highlighted"
    );
    let landed = transcript_text(&app)
        .lines()
        .position(|line| line.contains("needle two"))
        .expect("the match is on screen");
    assert!(
        landed < 35 / 2,
        "match at row {landed} is not near the top of the pane"
    );

    app.write(b"n").unwrap();
    wait_for_transcript(&app, "needle three quokka");
    app.write(b"N").unwrap();
    wait_for_transcript(&app, "needle two");
    app.write(b"N").unwrap();
    wait_for_transcript(&app, "needle one quokka");
    assert!(highlighted_cells(&app) >= 1);
    app.write(b"N").unwrap();
    wait_for(&app, "no earlier match");

    app.write(ESC).unwrap();
    let started = Instant::now();
    while highlighted_cells(&app) > 0 {
        assert!(started.elapsed() < TIMEOUT, "highlight never cleared");
        std::thread::sleep(Duration::from_millis(20));
    }
    wait_for_transcript(&app, "needle one quokka");
    app.kill();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_transcript_changed_since_it_was_indexed_opens_at_the_tail_and_says_the_match_moved() {
    let root = std::env::temp_dir().join(format!("ccview-search-moved-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (home, config) = (root.join("home"), root.join("config"));
    let path = write_long_session(&home, "-work-long", "44444444-dddd-4ddd-8ddd-dddddddddddd");
    let app = start(&home, &config);
    wait_for(&app, "filler 0 padding");

    app.write(b"/").unwrap();
    wait_for(&app, "esc leave");
    app.write(b"quokka").unwrap();
    wait_for(&app, "3 hits");

    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let body = fs::read_to_string(&path).unwrap();
    let last = format!("filler {} padding", TRAFFIC - 1);
    assert!(body.contains(&last));
    fs::write(
        &path,
        body.replace(&last, &last.replace("filler", "fillex")),
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    app.write(ENTER).unwrap();

    wait_for(&app, "match moved");
    wait_for(&app, &format!("fillex {} padding", TRAFFIC - 1));
    assert!(!transcript_text(&app).contains("needle"));
    app.kill();
    fs::remove_dir_all(root).unwrap();
}
