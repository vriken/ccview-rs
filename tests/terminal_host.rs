use std::time::{Duration, Instant};

use ccview_rs::terminal::{CommandSpec, PtyHost, Size, TerminalWidget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;

const TIMEOUT: Duration = Duration::from_secs(10);

fn sh(script: &str) -> CommandSpec {
    CommandSpec::new("sh").args(["-c", script])
}

fn spawn(script: &str, rows: u16, cols: u16) -> PtyHost {
    PtyHost::spawn(&sh(script), Size { rows, cols }).unwrap()
}

fn wait_for_text(host: &PtyHost, needle: &str) {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = 0;
    loop {
        if host.snapshot().text().contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; screen:\n{}",
            host.snapshot().text()
        );
        seen = host.wait_for_change(seen, Duration::from_millis(200));
    }
}

fn render(host: &PtyHost, area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(area);
    TerminalWidget::new(host).render(area, &mut buffer);
    buffer
}

#[test]
fn output_lands_on_screen_and_exit_status_is_reported() {
    let host = spawn("printf 'hello\\r\\nworld'; exit 3", 5, 20);
    let status = host.wait_for_exit(TIMEOUT).expect("child exits");
    assert_eq!(status.code, 3);
    let text = host.snapshot().text();
    assert_eq!(text, "hello\nworld");
}

#[test]
fn exit_status_is_none_while_running_and_kill_terminates() {
    let host = spawn("sleep 60", 5, 20);
    assert!(host.exit_status().is_none());
    host.kill();
    let status = host.wait_for_exit(TIMEOUT).expect("killed");
    assert!(!status.success());
}

#[test]
fn resize_reaches_the_child_and_the_parser() {
    let host = spawn("stty size; read _; stty size; read _", 10, 40);
    wait_for_text(&host, "10 40");
    host.resize(7, 33).unwrap();
    host.write(b"\n").unwrap();
    wait_for_text(&host, "7 33");
    assert_eq!(host.snapshot().size(), Size { rows: 7, cols: 33 });
}

#[test]
fn input_reaches_the_child() {
    let host = spawn("read line; printf 'got:%s' \"$line\"", 5, 30);
    host.write(b"abc\r").unwrap();
    wait_for_text(&host, "got:abc");
}

#[test]
fn scrollback_can_be_scrolled_and_reset() {
    let host = spawn("seq 1 50; sleep 60", 5, 10);
    wait_for_text(&host, "50");
    assert!(host.snapshot().scrollback_len() >= 40);

    host.scroll_up(10);
    let scrolled = host.snapshot();
    assert_eq!(scrolled.scrollback_offset(), 10);
    assert_eq!(scrolled.cursor(), None);
    assert!(scrolled.text().starts_with("37"), "{}", scrolled.text());
    drop(scrolled);

    host.scroll_down(4);
    assert_eq!(host.snapshot().scrollback_offset(), 6);
    host.reset_scroll();
    assert_eq!(host.snapshot().scrollback_offset(), 0);
    assert!(host.snapshot().text().contains("50"));
}

#[test]
fn widget_draws_colors_attributes_wide_chars_and_clips() {
    let host = spawn(
        "printf '\\033[1;31mRED\\033[0m \\033[38;2;1;2;3mRGB\\033[0m \\033[48;5;99;7mIX\\033[0m 日本\\r\\nline2'; sleep 60",
        4,
        20,
    );
    wait_for_text(&host, "line2");

    let buffer = render(&host, Rect::new(2, 1, 20, 4));
    let red = &buffer[(2, 1)];
    assert_eq!(red.symbol(), "R");
    assert_eq!(red.fg, Color::Indexed(1));
    assert!(red.modifier.contains(Modifier::BOLD));
    assert_eq!(buffer[(6, 1)].fg, Color::Rgb(1, 2, 3));
    let indexed = &buffer[(10, 1)];
    assert_eq!(indexed.bg, Color::Indexed(99));
    assert!(indexed.modifier.contains(Modifier::REVERSED));
    assert_eq!(buffer[(13, 1)].symbol(), "日");
    assert_eq!(buffer[(15, 1)].symbol(), "本");
    assert_eq!(buffer[(2, 2)].symbol(), "l");

    let narrow = render(&host, Rect::new(0, 0, 12, 1));
    assert_eq!(narrow[(11, 0)].symbol(), " ");
    assert_eq!(narrow[(0, 0)].symbol(), "R");

    let tall = render(&host, Rect::new(0, 0, 30, 10));
    assert_eq!(tall[(25, 8)].symbol(), " ");
}

#[test]
fn a_group_member_that_ignores_hangup_is_killed_when_the_host_is_dropped() {
    let dir = std::env::temp_dir().join(format!("ccview-hup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("member.pid");
    let script = format!(
        "sh -c 'trap \"\" HUP; echo $$ > {}; exec sleep 60' </dev/null >/dev/null 2>&1 & sleep 60",
        pid_file.display()
    );
    let host = spawn(&script, 5, 20);
    let deadline = Instant::now() + TIMEOUT;
    let member: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "member never started");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(unsafe { libc::kill(member, 0) }, 0);
    drop(host);
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(member, 0) } == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survived = unsafe { libc::kill(member, 0) } == 0;
    if survived {
        unsafe { libc::kill(member, libc::SIGKILL) };
    }
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!survived, "hangup-ignoring member outlived the host");
}
