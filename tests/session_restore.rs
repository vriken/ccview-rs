mod common;

use ccview_rs::terminal::{
    CommandSpec, HostClient, PtyHost, RuntimeDir, Size, SpawnRequest, TerminalOptions,
};
use common::{RuntimeGuard, alive, wait_until};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_ccview-rs");
const SECRET: &str = "sekrit-token-xyz";
const FAKE_HARNESS: &str = "#!/bin/sh\n\
[ \"$1\" = agents ] && exit 0\n\
dir=$(dirname \"$0\")\n\
echo \"$(pwd -P)|$*\" >> \"$dir/calls.log\"\n\
echo \"ARGV=$* AGENT_VIEW=$CLAUDE_CODE_DISABLE_AGENT_VIEW CLAUDECODE=$CLAUDECODE TOKEN=$CCVIEW_TEST_TOKEN PATHHEAD=${PATH%%:*}\" >> \"$dir/env.log\"\n\
case \"$1\" in --resume|resume) sid=\"re-$2\";; *) sid=\"sess-$$\";; esac\n\
mkdir -p \"$HOME/.claude/sessions\"\n\
printf '{\"pid\":%s,\"sessionId\":\"%s\"}' \"$$\" \"$sid\" > \"$HOME/.claude/sessions/$$.json\"\n\
while [ ! -e \"$dir/exit-$sid\" ] && [ ! -e \"$dir/exit-${2:-none}\" ]; do sleep 0.1; done\n";

struct Fixture {
    root: PathBuf,
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
        .join(format!("ccview-restore-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for dir in ["home", "work", "bin"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    let fake = root.join("bin/fake-claude");
    fs::write(&fake, FAKE_HARNESS).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    Fixture {
        root,
        runtime: RuntimeGuard::new(name),
    }
}

impl Fixture {
    fn fake(&self) -> PathBuf {
        self.root.join("bin/fake-claude")
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn sessions_file(&self) -> PathBuf {
        self.root.join("state/ccview-rs/sessions.json")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        common::lifeline(&mut command)
            .args(args)
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("CCVIEW_RUNTIME_DIR", self.runtime.path())
            .env("CCVIEW_CLAUDE_BIN", self.fake())
            .env("CCVIEW_TEST_TOKEN", SECRET)
            .env("CLAUDECODE", "1");
        command
    }

    fn host(&self, args: &[&str]) -> String {
        let output = self.command(&["host"]).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn write_state(&self, sessions: Vec<Value>) {
        fs::create_dir_all(self.sessions_file().parent().unwrap()).unwrap();
        fs::write(
            self.sessions_file(),
            json!({ "sessions": sessions }).to_string(),
        )
        .unwrap();
    }

    fn entry(&self, harness: &str, cwd: &Path, session_id: Option<&str>) -> Value {
        json!({
            "harness": harness,
            "cwd": cwd,
            "session_id": session_id,
            "program": self.fake(),
            "launch": "new",
            "title": "t",
            "started_ms": 1,
        })
    }

    fn state(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.sessions_file()).unwrap_or_default();
        serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value["sessions"].as_array().cloned())
            .unwrap_or_default()
    }

    fn state_ids(&self) -> Vec<String> {
        self.state()
            .iter()
            .filter_map(|entry| entry["session_id"].as_str().map(str::to_owned))
            .collect()
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("bin/calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn client(&self) -> HostClient {
        HostClient::new(RuntimeDir::at(self.runtime.path()))
    }

    fn spawn(&self, harness: &str, args: &[&str], session_id: Option<&str>) -> u32 {
        let spec = CommandSpec::new(self.fake().to_str().unwrap())
            .args(args.iter().copied())
            .cwd(self.work())
            .env("HOME", self.root.join("home").to_str().unwrap());
        let info = self
            .client()
            .spawn(SpawnRequest {
                spec,
                options: TerminalOptions::default(),
                size: Size { rows: 24, cols: 80 },
                title: "t".to_owned(),
                harness: harness.to_owned(),
                session_id: session_id.map(str::to_owned),
            })
            .unwrap();
        info.pid.unwrap()
    }

    fn stop_session(&self, key: &str) {
        fs::write(self.root.join("bin").join(format!("exit-{key}")), "").unwrap();
    }

    fn finish(&self) {
        self.runtime.stop();
        assert_eq!(processes_mentioning(&self.root), Vec::<String>::new());
    }
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
fn a_session_whose_harness_exits_by_itself_is_removed_from_the_list() {
    let fx = fixture("exit");
    let mut daemon = fx.command(&["host", "--restore"]);
    daemon.env("CCVIEW_HOST_EXIT_GRACE_MS", "100");
    assert!(daemon.output().unwrap().status.success());
    fx.spawn("claude", &["--resume", "a1"], Some("a1"));
    wait_until("the entry to be written", || {
        fx.state_ids().contains(&"re-a1".to_owned())
    });
    fx.stop_session("a1");
    wait_until("the entry to be removed", || fx.state().is_empty());
    fx.finish();
}

#[test]
fn sigterm_keeps_every_session_that_has_an_id_and_drops_the_ones_that_have_none() {
    let fx = fixture("sigterm");
    let mut daemon = fx.command(&["host", "--restore"]);
    daemon.env("CCVIEW_HOST_EXIT_GRACE_MS", "60000");
    assert!(daemon.output().unwrap().status.success());
    fx.spawn("claude", &["--resume", "a"], Some("a"));
    fx.spawn("claude", &["--resume", "b"], Some("b"));
    fx.spawn("codex", &[], None);
    wait_until("both claude ids to be bound", || {
        let ids = fx.state_ids();
        ids.contains(&"re-a".to_owned()) && ids.contains(&"re-b".to_owned())
    });
    fx.stop_session("a");
    wait_until("the exit to be seen", || {
        fx.host(&["--list"])
            .lines()
            .any(|line| line.contains("exited"))
    });
    std::thread::sleep(Duration::from_millis(1500));
    let daemon_pid = fx.runtime.daemon_pid().unwrap();
    unsafe { libc::kill(daemon_pid as i32, libc::SIGTERM) };
    wait_until("the daemon to exit", || !alive(daemon_pid));
    let mut ids = fx.state_ids();
    ids.sort();
    assert_eq!(ids, ["re-a", "re-b"]);
    assert_eq!(fx.state().len(), 2);
    fx.finish();
}

#[test]
fn restore_relaunches_with_the_resume_argv_in_the_entrys_cwd_and_keeps_the_environment_out_of_the_file()
 {
    let fx = fixture("argv");
    let missing = fx.root.join("gone");
    fx.write_state(vec![
        fx.entry("claude", &fx.work(), Some("s1")),
        fx.entry("claude", &missing, Some("s2")),
        fx.entry("claude", &fx.work(), None),
        fx.entry("codex", &fx.work(), Some("c1")),
    ]);
    let output = fx.host(&["--restore"]);
    assert!(output.contains("restored 2 session(s)"), "{output}");
    let work = fx.work().display().to_string();
    wait_until("both harnesses to start", || fx.calls().len() == 2);
    let log = fs::read_to_string(fx.runtime.path().join("host.log")).unwrap();
    assert!(
        log.contains("skipping s2") && log.contains("does not exist"),
        "{log}"
    );
    assert!(log.contains("no session id"), "{log}");
    let mut calls = fx.calls();
    calls.sort();
    assert_eq!(
        calls,
        [format!("{work}|--resume s1"), format!("{work}|resume c1")]
    );
    let environment = fs::read_to_string(fx.root.join("bin/env.log")).unwrap();
    let home_bin = format!("PATHHEAD={}", fx.root.join("home/.local/bin").display());
    let claude_line = environment
        .lines()
        .find(|line| line.starts_with("ARGV=--resume s1 "))
        .unwrap();
    assert!(
        claude_line.contains("AGENT_VIEW=1 CLAUDECODE= "),
        "{claude_line}"
    );
    assert!(claude_line.contains(&home_bin), "{claude_line}");

    wait_until("the resumed claude id to be rebound", || {
        fx.state_ids().contains(&"re-s1".to_owned())
    });
    let text = fs::read_to_string(fx.sessions_file()).unwrap();
    assert!(
        !text.contains(SECRET) && !text.contains("PATH") && !text.contains("env"),
        "{text}"
    );
    assert_eq!(fx.state().len(), 3);

    assert!(fx.host(&["--restore"]).contains("restored 0 session(s)"));
    assert_eq!(fx.calls().len(), 2);
    fx.finish();
}

#[test]
fn restore_starts_at_most_twelve_sessions() {
    let fx = fixture("cap");
    fx.write_state(
        (0..14)
            .map(|n| fx.entry("claude", &fx.work(), Some(&format!("s{n}"))))
            .collect(),
    );
    assert!(fx.host(&["--restore"]).contains("restored 12 session(s)"));
    wait_until("twelve launches", || fx.calls().len() == 12);
    assert_eq!(fx.state().len(), 14);
    assert!(fx.state_ids().contains(&"s13".to_owned()));
    fx.finish();
}

#[test]
fn two_concurrent_restores_start_each_session_once() {
    let fx = fixture("concurrent");
    fx.write_state(
        (0..4)
            .map(|n| fx.entry("claude", &fx.work(), Some(&format!("s{n}"))))
            .collect(),
    );
    let mut daemon = fx
        .command(&["host"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_until("the daemon socket", || {
        fx.runtime.path().join("host.sock").exists()
    });
    let restores: Vec<_> = (0..2)
        .map(|_| {
            let client = fx.client();
            std::thread::spawn(move || client.restore().unwrap())
        })
        .collect();
    let started: usize = restores.into_iter().map(|r| r.join().unwrap()).sum();
    wait_until("the launches", || fx.calls().len() >= 4);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!((started, fx.calls().len()), (4, 4));
    fx.finish();
    let _ = daemon.wait();
}

#[test]
fn a_session_that_is_already_running_is_not_started_a_second_time() {
    let fx = fixture("dup");
    fx.write_state(vec![fx.entry("codex", &fx.work(), Some("dup"))]);
    let mut daemon = fx
        .command(&["host"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_until("the daemon socket", || {
        fx.runtime.path().join("host.sock").exists()
    });
    fx.spawn("codex", &["resume", "dup"], Some("dup"));
    wait_until("the running copy", || fx.calls().len() == 1);
    assert!(fx.host(&["--restore"]).contains("restored 0 session(s)"));
    assert_eq!(fx.calls().len(), 1);
    fx.finish();
    let _ = daemon.wait();
}

const TIMEOUT: Duration = Duration::from_secs(15);

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
fn a_session_started_in_ccview_comes_back_after_the_host_is_terminated_and_restore_runs() {
    let fx = fixture("e2e");
    let spec = common::with_lifeline(CommandSpec::new(BIN))
        .cwd(&fx.root)
        .env("HOME", fx.root.join("home").to_str().unwrap())
        .env("XDG_STATE_HOME", fx.root.join("state").to_str().unwrap())
        .env("XDG_CONFIG_HOME", fx.root.join("config").to_str().unwrap())
        .env("XDG_CACHE_HOME", fx.root.join("cache").to_str().unwrap())
        .env("CCVIEW_RUNTIME_DIR", fx.runtime.path().to_str().unwrap())
        .env("CCVIEW_CLAUDE_BIN", fx.fake().to_str().unwrap())
        .env("TERM", "xterm-256color");
    let app = PtyHost::spawn(
        &spec,
        Size {
            rows: 35,
            cols: 120,
        },
    )
    .unwrap();
    wait_for(&app, "No sessions.");
    app.write(b"n").unwrap();
    wait_until("the new session to get its id", || {
        fx.state_ids().iter().any(|id| id.starts_with("sess-"))
    });
    let session_id = fx.state_ids().remove(0);
    let child_pids = fx.runtime.session_pids();
    let daemon = fx.runtime.daemon_pid().unwrap();

    unsafe { libc::kill(daemon as i32, libc::SIGTERM) };
    wait_until("the daemon to exit", || !alive(daemon));
    for pid in child_pids {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    drop(app);
    assert_eq!(fx.state_ids(), std::slice::from_ref(&session_id));

    assert!(fx.host(&["--restore"]).contains("restored 1 session(s)"));
    wait_until("the relaunch", || fx.calls().len() == 2);
    assert_eq!(
        fx.calls()[1],
        format!("{}|--resume {session_id}", fx.root.display())
    );
    fx.finish();
}

struct HostWithSession {
    daemon: std::process::Child,
    daemon_pid: u32,
    session_pid: u32,
    child_env: PathBuf,
}

fn start_host_with_session(fx: &Fixture, owner: Option<u32>) -> HostWithSession {
    let mut command = fx.command(&["host"]);
    match owner {
        Some(pid) => command.env("CCVIEW_HOST_OWNER_PID", pid.to_string()),
        None => command.env_remove("CCVIEW_HOST_OWNER_PID"),
    };
    let daemon = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_until("the daemon socket", || {
        fx.runtime.path().join("host.sock").exists()
    });
    let child_env = fx.root.join("child-env.txt");
    let script = format!("env > '{}'; exec sleep 300", child_env.display());
    let spec = CommandSpec::new("sh")
        .args(["-c", script.as_str()])
        .cwd(fx.work())
        .env("CCVIEW_HOST_OWNER_PID", "1");
    let info = fx
        .client()
        .spawn(SpawnRequest {
            spec,
            options: TerminalOptions::default(),
            size: Size { rows: 24, cols: 80 },
            title: "t".to_owned(),
            harness: "claude".to_owned(),
            session_id: None,
        })
        .unwrap();
    wait_until("the child to dump its environment", || {
        child_env.exists() && fs::metadata(&child_env).is_ok_and(|meta| meta.len() > 0)
    });
    HostWithSession {
        daemon_pid: daemon.id(),
        daemon,
        session_pid: info.pid.unwrap(),
        child_env,
    }
}

#[test]
fn a_host_whose_owner_process_is_gone_takes_its_sessions_down_with_it() {
    let fx = fixture("lifeline");
    let mut owner = Command::new("sleep").arg("2").spawn().unwrap();
    let mut host = start_host_with_session(&fx, Some(owner.id()));
    assert!(alive(host.daemon_pid) && alive(host.session_pid));
    assert!(
        !fs::read_to_string(&host.child_env)
            .unwrap()
            .contains("CCVIEW_HOST_OWNER_PID")
    );

    owner.wait().unwrap();
    wait_until("the host and its session to be gone", || {
        host.daemon.try_wait().unwrap().is_some() && !alive(host.session_pid)
    });
    assert!(!fx.runtime.path().join("host.sock").exists());
    assert!(!fx.runtime.path().join("host.pid").exists());
}

#[test]
fn a_host_started_without_an_owner_outlives_any_process_and_keeps_its_session() {
    let fx = fixture("no-lifeline");
    let mut helper = Command::new("sleep").arg("1").spawn().unwrap();
    let mut host = start_host_with_session(&fx, None);
    helper.wait().unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(alive(host.daemon_pid) && alive(host.session_pid));
    unsafe { libc::kill(host.session_pid as i32, libc::SIGKILL) };
    unsafe { libc::kill(host.daemon_pid as i32, libc::SIGTERM) };
    let _ = host.daemon.wait();
}
