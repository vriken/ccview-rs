use ccview_rs::terminal::{CommandSpec, PtyHost, Size};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const CODEX_ID: &str = "019aaaaa-0000-7000-8000-000000000001";
const OPENCODE_ID: &str = "ses_fixture0001";

struct Fixture {
    home: PathBuf,
    codex_cwd: PathBuf,
    opencode_cwd: PathBuf,
    open_capture: PathBuf,
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture(name: &str) -> Fixture {
    let home = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("ccview-launch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    let codex_cwd = home.join("work/codex-app");
    let opencode_cwd = home.join("work/opencode-app");
    fs::create_dir_all(&codex_cwd).unwrap();
    fs::create_dir_all(&opencode_cwd).unwrap();

    let day = home.join(".codex/sessions/2026/01/01");
    fs::create_dir_all(&day).unwrap();
    let rollout = day.join(format!("rollout-2026-01-01T10-00-00-{CODEX_ID}.jsonl"));
    fs::write(
        &rollout,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{CODEX_ID}\",\"cwd\":\"{}\"}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"codex fixture prompt\"}}]}}}}\n",
            codex_cwd.display()
        ),
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(&rollout)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(600))
        .unwrap();

    let data = home.join(".local/share/opencode");
    fs::create_dir_all(&data).unwrap();
    let db = rusqlite::Connection::open(data.join("opencode.db")).unwrap();
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    db.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, parent_id TEXT, title TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
    )
    .unwrap();
    db.execute(
        "INSERT INTO session VALUES (?1, ?2, NULL, 'opencode fixture title', ?3, NULL)",
        rusqlite::params![OPENCODE_ID, opencode_cwd.to_str().unwrap(), now_ms],
    )
    .unwrap();

    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for fake in ["fake-codex", "fake-opencode"] {
        executable(
            &bin.join(fake),
            "#!/bin/sh\necho \"ARGV[$*] PWD[$(pwd -P)]\" >> \"$HOME/launch.log\"\nexec sleep 30\n",
        );
    }
    let open_capture = home.join("open-capture.command");
    executable(
        &bin.join("open"),
        &format!("#!/bin/sh\ncp \"$3\" {}\n", open_capture.display()),
    );
    Fixture { home, codex_cwd, opencode_cwd, open_capture }
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

fn start(fixture: &Fixture) -> PtyHost {
    let bin = fixture.home.join("bin");
    let spec = CommandSpec::new(env!("CARGO_BIN_EXE_ccview-rs"))
        .env("HOME", fixture.home.to_str().unwrap())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("CCVIEW_CODEX_BIN", bin.join("fake-codex").to_str().unwrap())
        .env("CCVIEW_OPENCODE_BIN", bin.join("fake-opencode").to_str().unwrap())
        .env("CCVIEW_CLAUDE_BIN", bin.join("fake-claude-unused").to_str().unwrap())
        .env("TERM", "xterm-256color");
    let host = PtyHost::spawn(&spec, Size { rows: 35, cols: 120 }).unwrap();
    wait_for(&host, "opencode fixture title");
    wait_for(&host, "codex fixture prompt");
    host.write(b"kk").unwrap();
    host
}

fn embedded_argv(fixture: &Fixture, row_down: bool, key: u8, expected: &str, cwd: &Path) {
    let log = fixture.home.join("launch.log");
    let _ = fs::remove_file(&log);
    let host = start(fixture);
    if row_down {
        host.write(b"j").unwrap();
    }
    host.write(&[key]).unwrap();
    let started = Instant::now();
    let wanted = format!("ARGV[{expected}] PWD[{}]\n", cwd.display());
    while fs::read_to_string(&log).unwrap_or_default() != wanted {
        assert!(
            started.elapsed() < TIMEOUT,
            "key {} wanted {wanted:?} got {:?}",
            key as char,
            fs::read_to_string(&log)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    host.kill();
}

#[test]
fn every_launch_key_runs_the_selected_harness_binary_with_the_documented_argv_in_the_entry_cwd() {
    let fx = fixture("keys");
    let cases = [
        ("opencode", false, &fx.opencode_cwd, [
            (b'r', format!("--session {OPENCODE_ID}")),
            (b'f', format!("--session {OPENCODE_ID} --fork")),
            (b'n', String::new()),
            (b'c', "--continue".to_owned()),
        ]),
        ("codex", true, &fx.codex_cwd, [
            (b'r', format!("resume {CODEX_ID}")),
            (b'f', format!("fork {CODEX_ID}")),
            (b'n', String::new()),
            (b'c', "resume --last".to_owned()),
        ]),
    ];
    for (_, row_down, cwd, keys) in &cases {
        for (key, argv) in keys {
            embedded_argv(&fx, *row_down, *key, argv, cwd);
        }
    }
    fs::remove_dir_all(&fx.home).unwrap();
}

#[test]
fn o_and_capital_o_hand_the_selected_harness_command_to_the_external_terminal() {
    let fx = fixture("external");
    let cases = [
        (false, b'o', format!("exec '{}/bin/fake-opencode' '--session' '{OPENCODE_ID}'", fx.home.display()), &fx.opencode_cwd),
        (false, b'O', format!("exec '{}/bin/fake-opencode'\n", fx.home.display()), &fx.opencode_cwd),
        (true, b'o', format!("exec '{}/bin/fake-codex' 'resume' '{CODEX_ID}'", fx.home.display()), &fx.codex_cwd),
        (true, b'O', format!("exec '{}/bin/fake-codex'\n", fx.home.display()), &fx.codex_cwd),
    ];
    for (row_down, key, exec_line, cwd) in cases {
        let _ = fs::remove_file(&fx.open_capture);
        let host = start(&fx);
        if row_down {
            host.write(b"j").unwrap();
        }
        host.write(&[key]).unwrap();
        let started = Instant::now();
        while !fx.open_capture.exists() {
            assert!(started.elapsed() < TIMEOUT, "open was never called for {}", key as char);
            std::thread::sleep(Duration::from_millis(20));
        }
        let script = fs::read_to_string(&fx.open_capture).unwrap();
        assert!(script.contains(&format!("cd -- '{}'", cwd.display())), "{script}");
        assert!(script.contains(&exec_line), "key {} row_down {row_down}: {script}", key as char);
        host.kill();
    }
    fs::remove_dir_all(&fx.home).unwrap();
}

#[test]
fn attach_is_refused_with_a_notice_on_a_codex_row() {
    let fx = fixture("attach");
    let host = start(&fx);
    host.write(b"ja").unwrap();
    wait_for(&host, "Codex cannot do that");
    host.kill();
    fs::remove_dir_all(&fx.home).unwrap();
}
