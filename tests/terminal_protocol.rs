use std::time::{Duration, Instant};

use ccview_rs::terminal::input::send_key;
use ccview_rs::terminal::{
    CommandSpec, KeyboardFlags, PtyHost, Rgb, Size, TerminalEvent, TerminalOptions,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const TIMEOUT: Duration = Duration::from_secs(10);

fn spawn(script: &str) -> PtyHost {
    spawn_with(script, TerminalOptions::default())
}

fn spawn_with(script: &str, options: TerminalOptions) -> PtyHost {
    let spec = CommandSpec::new("sh").args(["-c", script]);
    PtyHost::spawn_with_options(&spec, Size { rows: 10, cols: 200 }, options).unwrap()
}

fn wait_for_text(host: &PtyHost, needle: &str) -> String {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = 0;
    loop {
        let text = host.snapshot().text().split_whitespace().collect::<Vec<_>>().join(" ");
        if text.contains(needle) {
            return text;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {needle:?}; screen:\n{text}");
        seen = host.wait_for_change(seen, Duration::from_millis(200));
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ")
}

fn assert_reply_with(options: TerminalOptions, setup: &str, query: &str, expected: &[u8]) {
    let script = format!(
        "stty -icanon -echo -icrnl; printf '{setup}{query}'; printf 'R:'; head -c {} | od -An -v -tx1 | tr -d '\\n'; printf ':E'",
        expected.len()
    );
    let host = spawn_with(&script, options);
    let text = wait_for_text(&host, ":E");
    let got = text.split("R:").nth(1).unwrap().split(":E").next().unwrap().trim();
    assert_eq!(got, hex(expected), "query {query:?}");
}

fn assert_reply(query: &str, expected: &[u8]) {
    assert_reply_with(TerminalOptions::default(), "", query, expected);
}

fn no_reply_within_a_second(query: &str) {
    let script = format!("stty -icanon -echo -icrnl; printf '{query}'; read -t 1 -n 1 got; printf 'got=[%s]' \"$got\"");
    let host = spawn(&script);
    wait_for_text(&host, "got=[]");
}

fn after_marker(host: &PtyHost) {
    wait_for_text(host, "MARK");
}

#[test]
fn kitty_keyboard_stack_push_pop_and_set() {
    let flags = |script: &str| {
        let host = spawn(&format!("printf '{script}MARK'; sleep 60"));
        after_marker(&host);
        host.keyboard_flags().bits()
    };
    assert_eq!(flags(""), 0);
    assert_eq!(flags("\\033[>1u\\033[>5u"), 5);
    assert_eq!(flags("\\033[>1u\\033[>5u\\033[<u"), 1);
    assert_eq!(flags("\\033[>1u\\033[>5u\\033[<2u"), 0);
    assert_eq!(flags("\\033[<u\\033[<1u"), 0);
    assert_eq!(flags("\\033[=8u"), 8);
    assert_eq!(flags("\\033[>1u\\033[=8;2u"), 9);
    assert_eq!(flags("\\033[>9u\\033[=8;3u"), 1);
    assert_eq!(flags("\\033[>1u\\033[=4;1u"), 4);
}

#[test]
fn kitty_keyboard_flags_are_kept_per_screen() {
    let host = spawn("printf '\\033[>5u\\033[?1049h\\033[>1uALT'; sleep 60");
    wait_for_text(&host, "ALT");
    assert_eq!(host.keyboard_flags(), KeyboardFlags::DISAMBIGUATE);
    let host = spawn("printf '\\033[>5u\\033[?1049hALT\\033[?1049lMARK'; sleep 60");
    after_marker(&host);
    assert_eq!(host.keyboard_flags().bits(), 5);
}

#[test]
fn kitty_keyboard_query_reports_current_flags() {
    assert_reply_with(TerminalOptions::default(), "\\033[>5u", "\\033[?u", b"\x1b[?5u");
    assert_reply("\\033[?u", b"\x1b[?0u");
}

#[test]
fn shift_enter_reaches_a_child_that_enabled_kitty_disambiguation() {
    let host = spawn("stty -icanon -echo -icrnl; printf '\\033[>1uREADY'; head -c 7 | od -An -v -tx1 | tr -d '\\n'; printf ':E'");
    wait_for_text(&host, "READY");
    send_key(&host, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
    let text = wait_for_text(&host, ":E");
    assert!(text.contains(&hex(b"\x1b[13;2u")), "{text}");
}

#[test]
fn shift_enter_is_plain_enter_for_a_child_without_kitty() {
    let host = spawn("stty -icanon -echo -icrnl; printf 'READY'; head -c 1 | od -An -v -tx1 | tr -d '\\n'; printf ':E'");
    wait_for_text(&host, "READY");
    send_key(&host, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
    let text = wait_for_text(&host, ":E");
    assert!(text.contains("0d"), "{text}");
}

#[test]
fn focus_events_are_sent_only_while_the_child_asked_for_them() {
    let host = spawn("stty -icanon -echo -icrnl; printf '\\033[?1004hREADY'; head -c 3 | od -An -v -tx1 | tr -d '\\n'; printf ':E'");
    wait_for_text(&host, "READY");
    assert!(host.focus_reporting());
    host.send_focus(true).unwrap();
    let text = wait_for_text(&host, ":E");
    assert!(text.contains(&hex(b"\x1b[I")), "{text}");

    let host = spawn("printf '\\033[?1004h\\033[?1004lMARK'; stty -icanon -echo -icrnl; read -t 1 -n 1 got; printf 'got=[%s]' \"$got\"");
    after_marker(&host);
    assert!(!host.focus_reporting());
    host.send_focus(false).unwrap();
    wait_for_text(&host, "got=[]");
}

#[test]
fn focus_out_event_is_escape_bracket_o() {
    let host = spawn("stty -icanon -echo -icrnl; printf '\\033[?1004hREADY'; head -c 3 | od -An -v -tx1 | tr -d '\\n'; printf ':E'");
    wait_for_text(&host, "READY");
    host.send_focus(false).unwrap();
    let text = wait_for_text(&host, ":E");
    assert!(text.contains(&hex(b"\x1b[O")), "{text}");
}

#[test]
fn title_clipboard_notification_and_bell_become_events_without_touching_the_screen() {
    let host = spawn(
        "printf 'x\\033]0;first\\007\\033]2;my title\\007\\033]52;c;aGVsbG8=\\007\\033]9;done\\007\\033]777;notify;T;B;C\\007\\033]9;4;1;50\\007\\007MARK'; sleep 60",
    );
    after_marker(&host);
    assert_eq!(
        host.take_events(),
        vec![
            TerminalEvent::Title("my title".into()),
            TerminalEvent::Clipboard { selection: "c".into(), data: b"hello".to_vec() },
            TerminalEvent::Notification { title: None, body: "done".into() },
            TerminalEvent::Notification { title: Some("T".into()), body: "B;C".into() },
            TerminalEvent::Bell,
        ]
    );
    assert!(host.take_events().is_empty());
    assert_eq!(host.snapshot().text(), "xMARK");
}

#[test]
fn osc_8_hyperlinks_leave_only_their_text() {
    let host = spawn("printf 'a\\033]8;id=1;https://example.com\\007LINK\\033]8;;\\007b MARK'; sleep 60");
    after_marker(&host);
    assert_eq!(host.snapshot().text(), "aLINKb MARK");
}

#[test]
fn synchronized_output_is_tracked_and_does_not_disturb_the_screen() {
    let host = spawn("printf '\\033[?2026hab'; sleep 60");
    wait_for_text(&host, "ab");
    assert!(host.synchronized_output());
    let host = spawn("printf '\\033[?2026hab\\033[?2026lcd'; sleep 60");
    wait_for_text(&host, "abcd");
    assert!(!host.synchronized_output());
}

#[test]
fn default_colors_are_reported_to_osc_10_and_11_queries() {
    assert_reply("\\033]10;?\\033\\\\", b"\x1b]10;rgb:dddd/dddd/dddd\x1b\\");
    assert_reply("\\033]11;?\\007", b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\");
}

#[test]
fn configured_colors_replace_the_defaults() {
    let options = TerminalOptions {
        foreground: Rgb(0, 0, 0),
        background: Rgb(255, 255, 255),
        ansi_palette: vec![Rgb(1, 2, 3)],
        ..TerminalOptions::default()
    };
    assert_reply_with(options.clone(), "", "\\033]11;?\\007", b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    assert_reply_with(options, "", "\\033]4;0;?\\007", b"\x1b]4;0;rgb:0101/0202/0303\x1b\\");
}

#[test]
fn palette_queries_cover_ansi_cube_and_grayscale() {
    assert_reply(
        "\\033]4;196;?\\007\\033]4;244;?\\007",
        b"\x1b]4;196;rgb:ffff/0000/0000\x1b\\\x1b]4;244;rgb:8080/8080/8080\x1b\\",
    );
}

#[test]
fn mode_queries_report_set_reset_and_unknown() {
    assert_reply("\\033[?2004$p", b"\x1b[?2004;2$y");
    assert_reply_with(TerminalOptions::default(), "\\033[?2004h", "\\033[?2004$p", b"\x1b[?2004;1$y");
    assert_reply_with(TerminalOptions::default(), "\\033[?1004h", "\\033[?1004$p", b"\x1b[?1004;1$y");
    assert_reply_with(TerminalOptions::default(), "\\033[?2026h", "\\033[?2026$p", b"\x1b[?2026;1$y");
    assert_reply("\\033[?2026$p", b"\x1b[?2026;2$y");
    assert_reply("\\033[?2027$p", b"\x1b[?2027;0$y");
}

#[test]
fn version_and_window_size_queries_are_answered() {
    let version = format!("\x1bP>|ccview-rs {}\x1b\\", env!("CARGO_PKG_VERSION"));
    assert_reply("\\033[>0q", version.as_bytes());
    assert_reply("\\033[18t", b"\x1b[8;10;200t");
    assert_reply("\\033[14t", b"\x1b[4;160;1600t");
    assert_reply("\\033[16t", b"\x1b[6;16;8t");
}

#[test]
fn device_attributes_and_cursor_reports_still_work() {
    assert_reply("\\033[c", b"\x1b[?62;22c");
    assert_reply("ab\\033[6n", b"\x1b[1;3R");
}

#[test]
fn queries_nobody_supports_get_no_answer() {
    no_reply_within_a_second("\\033_Gi=31337,s=1,v=1,a=q,t=d,f=24;AAAA\\033\\\\\\033P+q4d73\\033\\\\");
}
