use ccview_rs::terminal::session_host::daemon::{self, DaemonConfig, Outcome};
use ccview_rs::terminal::session_host::protocol::{
    Frame, Request, read_frame, write_bytes, write_message,
};
use ccview_rs::terminal::{
    CommandSpec, ExitStatus, HostClient, KeyboardFlags, Output, PtyHost, RemoteHost, RuntimeDir,
    SessionInfo, Size, SpawnRequest, TerminalEvent, TerminalOptions, TerminalSession,
};
use std::{
    fs,
    num::NonZeroU16,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixStream,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);
const SIZE: Size = Size { rows: 12, cols: 40 };

struct TestDaemon {
    dir: RuntimeDir,
    thread: Option<JoinHandle<std::io::Result<Outcome>>>,
}

impl TestDaemon {
    fn start(name: &str, idle_exit: Duration) -> Self {
        let path = std::env::temp_dir().join(format!("ccview-hostd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        let dir = RuntimeDir::at(path);
        let thread_dir = dir.clone();
        let thread =
            std::thread::spawn(move || daemon::run(&thread_dir, &DaemonConfig { idle_exit }));
        wait_until(|| dir.socket().exists());
        Self {
            dir,
            thread: Some(thread),
        }
    }

    fn client(&self) -> HostClient {
        HostClient::new(self.dir.clone())
    }

    fn finished(&self) -> bool {
        self.thread.as_ref().is_some_and(JoinHandle::is_finished)
    }

    fn join(mut self) -> Outcome {
        self.thread.take().unwrap().join().unwrap().unwrap()
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.dir.path());
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "condition not met in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn request(script: &str) -> SpawnRequest {
    SpawnRequest {
        spec: CommandSpec::new("sh").args(["-c", script]),
        options: TerminalOptions::default(),
        size: SIZE,
        title: "t".to_owned(),
        harness: "claude".to_owned(),
        session_id: None,
    }
}

fn attach(client: &HostClient, info: &SessionInfo) -> RemoteHost {
    client.attach(info.id, None).unwrap().1
}

fn wait_for_text(host: &dyn TerminalSession, needle: &str) {
    let deadline = Instant::now() + TIMEOUT;
    while !host.snapshot().text().contains(needle) {
        assert!(
            Instant::now() < deadline,
            "never saw {needle:?}; screen:\n{}",
            host.snapshot().text()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn finish(host: &dyn TerminalSession) {
    host.write(b"\n").unwrap();
    wait_until(|| host.exit_status().is_some());
}

#[test]
fn input_output_and_exit_status_cross_the_socket_and_only_exited_sessions_can_be_dismissed() {
    let daemon = TestDaemon::start("roundtrip", Duration::from_secs(60));
    let client = daemon.client();
    let info = client
        .spawn(request("read x; echo got:$x; read y; exit 3"))
        .unwrap();
    assert_eq!(client.list().unwrap().len(), 1);

    let host = attach(&client, &info);
    host.write(b"hi\n").unwrap();
    wait_for_text(&host, "got:hi");
    assert!(host.exit_status().is_none());
    assert!(
        client.dismiss(info.id).is_err(),
        "a running session cannot be dismissed"
    );

    host.write(b"\n").unwrap();
    wait_until(|| host.exit_status().is_some());
    assert_eq!(
        host.exit_status(),
        Some(ExitStatus {
            code: 3,
            signal: None
        })
    );

    let late = attach(&client, &info);
    wait_until(|| late.exit_status().is_some());
    assert!(late.snapshot().text().contains("got:hi"));
    drop((host, late));
    client.dismiss(info.id).unwrap();
    assert!(client.list().unwrap().is_empty());
}

const FULL_SCREEN: &str = "printf '\\033[?1049h\\033[2J\\033]0;fancy title\\007';\
 printf '\\033[3;5H\\033[1;3;31;44mRED bold\\033[0m \\033[4mline\\033[0m';\
 printf '\\033[5;10H\\033[38;2;10;20;30;48;5;99mrgb\\033[0m \\033[7minv\\033[0m \\033[2mdim\\033[0m';\
 printf '\\033[7;3H日本語 wide';\
 printf '\\033[?2004h\\033[?1000h\\033[?1006h\\033[?1h\\033[?1004h\\033[>1u';\
 printf '\\033[9;12H\\033[?25lEND';\
 read x";

#[test]
fn a_reattached_full_screen_session_matches_the_original_cell_by_cell() {
    let daemon = TestDaemon::start("fidelity", Duration::from_secs(60));
    let client = daemon.client();
    let info = client.spawn(request(FULL_SCREEN)).unwrap();
    let original = attach(&client, &info);
    wait_for_text(&original, "END");
    let reattached = attach(&client, &info);

    {
        let (before, after) = (original.snapshot(), reattached.snapshot());
        let (before, after) = (before.screen(), after.screen());
        assert_eq!(before.size(), after.size());
        let (rows, cols) = before.size();
        for row in 0..rows.get() {
            for col in 0..cols.get() {
                let (a, b) = (
                    before.cell(row, col).unwrap(),
                    after.cell(row, col).unwrap(),
                );
                let describe = |cell: &vt100::Cell| {
                    (
                        cell.contents().to_owned(),
                        cell.fgcolor(),
                        cell.bgcolor(),
                        (
                            cell.bold(),
                            cell.dim(),
                            cell.italic(),
                            cell.underline(),
                            cell.inverse(),
                        ),
                        (cell.is_wide(), cell.is_wide_continuation()),
                    )
                };
                assert_eq!(describe(a), describe(b), "cell {row},{col}");
            }
        }
        assert_eq!(before.cursor_position(), after.cursor_position());
        let modes = |screen: &vt100::Screen| {
            (
                screen.hide_cursor(),
                screen.alternate_screen(),
                screen.application_cursor(),
                screen.bracketed_paste(),
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };
        assert_eq!(modes(before), modes(after));
        assert!(after.alternate_screen() && after.hide_cursor() && after.bracketed_paste());
    }

    assert_eq!(original.keyboard_flags(), KeyboardFlags::DISAMBIGUATE);
    assert_eq!(reattached.keyboard_flags(), KeyboardFlags::DISAMBIGUATE);
    assert!(original.focus_reporting() && reattached.focus_reporting());
    let title = TerminalEvent::Title("fancy title".to_owned());
    assert!(original.take_events().contains(&title));
    assert!(reattached.take_events().contains(&title));
    finish(&reattached);
}

#[test]
fn a_reattached_session_starts_at_the_bottom_and_scrolls_up_into_earlier_history() {
    let daemon = TestDaemon::start("bottom", Duration::from_secs(60));
    let client = daemon.client();
    let info = client
        .spawn(request(
            "i=1; while [ $i -le 300 ]; do echo row-$i; i=$((i+1)); done; read x",
        ))
        .unwrap();
    let original = attach(&client, &info);
    wait_for_text(&original, "row-300");

    let reattached = attach(&client, &info);
    wait_for_text(&reattached, "row-300");
    assert_eq!(reattached.snapshot().scrollback_offset(), 0);

    reattached.scroll_up(10_000);
    assert!(reattached.snapshot().text().starts_with("row-1\n"));
    reattached.reset_scroll();
    assert_eq!(reattached.snapshot().scrollback_offset(), 0);

    finish(&reattached);
}

#[test]
fn resize_from_an_attached_client_reaches_the_child() {
    let daemon = TestDaemon::start("resize", Duration::from_secs(60));
    let client = daemon.client();
    let info = client
        .spawn(request(
            "trap 'echo size:$(stty size)' WINCH; echo ready; read x",
        ))
        .unwrap();
    let host = attach(&client, &info);
    wait_for_text(&host, "ready");
    host.resize(20, 70).unwrap();
    wait_for_text(&host, "size:20 70");
    let reattached = attach(&client, &info);
    assert_eq!(reattached.snapshot().size(), Size { rows: 20, cols: 70 });
    finish(&reattached);
}

fn replay(bytes: &[u8]) -> vt100::Parser {
    let mut parser =
        vt100::Parser::new(NonZeroU16::new(5).unwrap(), NonZeroU16::new(20).unwrap(), 0);
    parser.process(bytes);
    parser
}

#[test]
fn snapshot_and_subscription_are_atomic_no_byte_is_lost_or_repeated() {
    const LINES: usize = 200_000;
    let script =
        format!("stty -onlcr; exec awk 'BEGIN{{for(i=1;i<={LINES};i++) printf \"%d\\r\\n\", i}}'");
    let host = PtyHost::spawn(
        &CommandSpec::new("sh").args(["-c", &script]),
        Size { rows: 5, cols: 20 },
    )
    .unwrap();
    let total: Vec<u8> = (1..=LINES)
        .flat_map(|i| format!("{i}\r\n").into_bytes())
        .collect();

    let mut drains = Vec::new();
    while host.exit_status().is_none() && drains.len() < 30 {
        let attachment = host.attach(None).unwrap();
        let screen = attachment.screen;
        let output = attachment.output;
        drains.push(std::thread::spawn(move || {
            let mut received = Vec::new();
            while let Ok(Output::Data(bytes)) = output.recv_timeout(TIMEOUT) {
                received.extend(bytes);
            }
            (screen, received)
        }));
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut results: Vec<(Vec<u8>, Vec<u8>)> = drains
        .into_iter()
        .map(|drain| drain.join().unwrap())
        .collect();
    results.sort_by_key(|(_, received)| std::cmp::Reverse(received.len()));
    let mut expected = replay(b"");
    let mut replayed = 0;
    let mut mid_stream = 0;
    for (screen, received) in results {
        let boundary = total.len() - received.len();
        assert_eq!(
            received,
            total[boundary..],
            "relayed bytes are a suffix of the child's output"
        );
        expected.process(&total[replayed..boundary]);
        replayed = boundary;
        let mut from_snapshot = replay(b"");
        from_snapshot.process(&screen);
        assert_eq!(
            from_snapshot.screen().contents_formatted(),
            expected.screen().contents_formatted(),
            "snapshot matches the output before the boundary at {boundary}"
        );
        if boundary > 0 && !received.is_empty() {
            mid_stream += 1;
        }
    }
    assert!(
        mid_stream > 0,
        "no attach landed mid-stream, the check proved nothing"
    );
}

#[test]
fn the_socket_and_directory_are_private_and_the_peer_is_the_current_user() {
    let daemon = TestDaemon::start("perms", Duration::from_secs(60));
    let mode = |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(daemon.dir.path()), 0o700);
    assert_eq!(mode(&daemon.dir.socket()), 0o600);
    let stream = UnixStream::connect(daemon.dir.socket()).unwrap();
    assert_eq!(
        daemon::peer_uid(&stream).unwrap(),
        ccview_rs::terminal::session_host::runtime_dir::effective_uid()
    );
}

#[test]
fn only_one_daemon_runs_per_directory() {
    let daemon = TestDaemon::start("single", Duration::from_secs(60));
    assert!(daemon::try_lock(&daemon.dir).unwrap().is_none());
    let outcome = daemon::run(&daemon.dir, &DaemonConfig::default()).unwrap();
    assert_eq!(outcome, Outcome::AlreadyRunning);
    assert!(
        daemon.dir.socket().exists(),
        "the second instance must not disturb the first"
    );
}

#[test]
fn the_daemon_exits_when_idle_but_never_while_a_session_is_alive_or_a_client_is_connected() {
    let idle = Duration::from_millis(400);
    let daemon = TestDaemon::start("idle", idle);
    let client = daemon.client();
    let info = client.spawn(request("read x")).unwrap();
    let host = attach(&client, &info);
    std::thread::sleep(idle * 4);
    assert!(
        !daemon.finished(),
        "alive session and connected client keep the daemon up"
    );

    drop(host);
    std::thread::sleep(idle * 4);
    assert!(
        !daemon.finished(),
        "a live session alone keeps the daemon up"
    );
    assert!(client.list().unwrap()[0].exit.is_none());

    let host = attach(&client, &info);
    finish(&host);
    drop(host);
    wait_until(|| daemon.finished());
    let dir = daemon.dir.clone();
    assert_eq!(daemon.join(), Outcome::Idle);
    assert!(!dir.socket().exists());
}

#[test]
fn a_child_that_never_reads_its_input_does_not_stall_other_clients_or_the_pasting_ui() {
    let daemon = TestDaemon::start("deaf-child", Duration::from_millis(500));
    let client = daemon.client();
    let flag = daemon.dir.path().join("ask");
    let script = format!(
        "stty raw -echo; printf ready; while [ ! -e {} ]; do sleep 0.05; done; printf '\\033[6n'; exec sleep 60",
        flag.display()
    );
    let info = client.spawn(request(&script)).unwrap();
    let pasting = attach(&client, &info);
    wait_for_text(&pasting, "ready");

    let started = Instant::now();
    pasting.write(&vec![b'a'; 1 << 20]).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(300));
    fs::write(&flag, "").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let started = Instant::now();
    assert_eq!(client.list().unwrap().len(), 1);
    assert!(started.elapsed() < Duration::from_secs(1));

    client.stop(info.id).unwrap();
    wait_until(|| pasting.exit_status().is_some());
    drop(pasting);
    wait_until(|| daemon.finished());
    daemon.join();
}

fn raw_attach(daemon: &TestDaemon, id: u64) -> UnixStream {
    let mut stream = UnixStream::connect(daemon.dir.socket()).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let request = Request::Attach { id, size: None };
    write_message(&mut stream, &request).unwrap();
    for _ in 0..2 {
        read_frame(&mut stream).unwrap();
    }
    stream
}

fn wait_for_demotion(stream: &mut UnixStream) {
    loop {
        if let Frame::Message(_) = read_frame(stream).unwrap() {
            return;
        }
    }
}

#[test]
fn only_the_newest_attach_drives_the_child_and_its_size_and_the_daemon_ignores_the_rest() {
    let daemon = TestDaemon::start("lease", Duration::from_secs(60));
    let client = daemon.client();
    let info = client
        .spawn(request(
            "while read x; do [ \"$x\" = quit ] && exit 0; echo got:$x; done",
        ))
        .unwrap();
    let mut first = raw_attach(&daemon, info.id);
    let newest_size = Size { rows: 20, cols: 60 };
    let second = client.attach(info.id, Some(newest_size)).unwrap().1;
    wait_for_demotion(&mut first);

    write_bytes(&mut first, b"stale\n").unwrap();
    write_message(&mut first, &Request::Resize { rows: 5, cols: 5 }).unwrap();
    second.write(b"fresh\n").unwrap();
    wait_for_text(&second, "got:fresh");
    assert!(!second.snapshot().text().contains("got:stale"));
    assert_eq!(client.list().unwrap()[0].size, newest_size);

    drop(second);
    let freed = client.attach(info.id, None).unwrap().1;
    freed.write(b"after\n").unwrap();
    wait_for_text(&freed, "got:after");
    assert!(!freed.snapshot().text().contains("got:stale"));
    drop(first);
    freed.write(b"quit\n").unwrap();
    wait_until(|| freed.exit_status().is_some());
}
