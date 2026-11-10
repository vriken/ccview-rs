use std::io;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use super::host::PtyHost;

const ESC: u8 = 0x1b;
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const LEGACY_MOUSE_MAX_COORDINATE: u16 = 223;

pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    if key.kind == KeyEventKind::Release {
        return Vec::new();
    }
    let modifiers = key.modifiers;
    let alt = modifiers.contains(KeyModifiers::ALT);
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let xterm_modifier = xterm_modifier(modifiers);

    let mut bytes = match key.code {
        KeyCode::Char(c) => return encode_char(c, ctrl, alt),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => return b"\x1b[Z".to_vec(),
        KeyCode::Backspace if ctrl => vec![0x08],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![ESC],
        KeyCode::Up => cursor_key(b'A', xterm_modifier, application_cursor),
        KeyCode::Down => cursor_key(b'B', xterm_modifier, application_cursor),
        KeyCode::Right => cursor_key(b'C', xterm_modifier, application_cursor),
        KeyCode::Left => cursor_key(b'D', xterm_modifier, application_cursor),
        KeyCode::Home => cursor_key(b'H', xterm_modifier, application_cursor),
        KeyCode::End => cursor_key(b'F', xterm_modifier, application_cursor),
        KeyCode::Insert => tilde_key(2, xterm_modifier),
        KeyCode::Delete => tilde_key(3, xterm_modifier),
        KeyCode::PageUp => tilde_key(5, xterm_modifier),
        KeyCode::PageDown => tilde_key(6, xterm_modifier),
        KeyCode::F(n) => return function_key(n, xterm_modifier),
        _ => return Vec::new(),
    };
    let escape_prefixed = alt && matches!(key.code, KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Esc);
    if escape_prefixed {
        bytes.insert(0, ESC);
    }
    bytes
}

fn encode_char(c: char, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if alt {
        bytes.push(ESC);
    }
    match ctrl_byte(c).filter(|_| ctrl) {
        Some(byte) => bytes.push(byte),
        None => {
            let mut utf8 = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        }
    }
    bytes
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

fn xterm_modifier(modifiers: KeyModifiers) -> u8 {
    let mut value = 1;
    if modifiers.contains(KeyModifiers::SHIFT) {
        value += 1;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        value += 2;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        value += 4;
    }
    value
}

fn cursor_key(final_byte: u8, modifier: u8, application_cursor: bool) -> Vec<u8> {
    if modifier > 1 {
        format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes()
    } else if application_cursor {
        vec![ESC, b'O', final_byte]
    } else {
        vec![ESC, b'[', final_byte]
    }
}

fn tilde_key(number: u8, modifier: u8) -> Vec<u8> {
    if modifier > 1 {
        format!("\x1b[{number};{modifier}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn function_key(n: u8, modifier: u8) -> Vec<u8> {
    match n {
        1..=4 => {
            let final_byte = (b'P' + n - 1) as char;
            if modifier > 1 {
                format!("\x1b[1;{modifier}{final_byte}").into_bytes()
            } else {
                format!("\x1bO{final_byte}").into_bytes()
            }
        }
        5 => tilde_key(15, modifier),
        6 => tilde_key(17, modifier),
        7 => tilde_key(18, modifier),
        8 => tilde_key(19, modifier),
        9 => tilde_key(20, modifier),
        10 => tilde_key(21, modifier),
        11 => tilde_key(23, modifier),
        12 => tilde_key(24, modifier),
        _ => Vec::new(),
    }
}

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    bytes
}

pub fn encode_mouse(
    event: MouseEvent,
    origin: (u16, u16),
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    let column = event.column.checked_sub(origin.0)?;
    let row = event.row.checked_sub(origin.1)?;
    let (button, released) = mouse_button_code(event.kind, mode)?;

    let mut code = button;
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }

    let (x, y) = (column + 1, row + 1);
    if encoding == MouseProtocolEncoding::Sgr {
        let suffix = if released { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }
    if x > LEGACY_MOUSE_MAX_COORDINATE || y > LEGACY_MOUSE_MAX_COORDINATE {
        return None;
    }
    let code = if released { (code & !0b11) | 3 } else { code };
    Some(vec![
        ESC,
        b'[',
        b'M',
        code + 32,
        x as u8 + 32,
        y as u8 + 32,
    ])
}

fn mouse_button_code(kind: MouseEventKind, mode: MouseProtocolMode) -> Option<(u8, bool)> {
    let reports_release = mode != MouseProtocolMode::Press;
    let reports_drag = matches!(
        mode,
        MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
    );
    let reports_motion = mode == MouseProtocolMode::AnyMotion;
    let button_number = |button: MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    match mode {
        MouseProtocolMode::None => None,
        _ => match kind {
            MouseEventKind::Down(button) => Some((button_number(button), false)),
            MouseEventKind::Up(button) => reports_release.then_some((button_number(button), true)),
            MouseEventKind::Drag(button) => reports_drag.then_some((button_number(button) + 32, false)),
            MouseEventKind::Moved => reports_motion.then_some((35, false)),
            MouseEventKind::ScrollUp => Some((64, false)),
            MouseEventKind::ScrollDown => Some((65, false)),
            MouseEventKind::ScrollLeft => Some((66, false)),
            MouseEventKind::ScrollRight => Some((67, false)),
        },
    }
}

pub fn send_key(host: &PtyHost, key: KeyEvent) -> io::Result<()> {
    let application_cursor = host.snapshot().screen().application_cursor();
    let bytes = encode_key(key, application_cursor);
    if bytes.is_empty() {
        return Ok(());
    }
    host.write(&bytes)
}

pub fn send_paste(host: &PtyHost, text: &str) -> io::Result<()> {
    let bracketed = host.snapshot().screen().bracketed_paste();
    host.write(&encode_paste(text, bracketed))
}

pub fn send_mouse(host: &PtyHost, event: MouseEvent, origin: (u16, u16)) -> io::Result<bool> {
    let (mode, encoding) = {
        let snapshot = host.snapshot();
        let screen = snapshot.screen();
        (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding())
    };
    match encode_mouse(event, origin, mode, encoding) {
        Some(bytes) => host.write(&bytes).map(|()| true),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn encoded(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        encode_key(key(code, modifiers), false)
    }

    #[test]
    fn tricky_keys_match_xterm() {
        let none = KeyModifiers::NONE;
        let cases: Vec<(KeyCode, KeyModifiers, &[u8])> = vec![
            (KeyCode::Char('c'), KeyModifiers::CONTROL, b"\x03"),
            (KeyCode::Char('A'), KeyModifiers::SHIFT, b"A"),
            (KeyCode::Char('b'), KeyModifiers::ALT, b"\x1bb"),
            (KeyCode::Char('d'), KeyModifiers::ALT | KeyModifiers::CONTROL, b"\x1b\x04"),
            (KeyCode::Char('é'), none, "é".as_bytes()),
            (KeyCode::Enter, none, b"\r"),
            (KeyCode::Enter, KeyModifiers::ALT, b"\x1b\r"),
            (KeyCode::Backspace, none, b"\x7f"),
            (KeyCode::Esc, none, b"\x1b"),
            (KeyCode::Tab, none, b"\t"),
            (KeyCode::BackTab, KeyModifiers::SHIFT, b"\x1b[Z"),
            (KeyCode::Up, none, b"\x1b[A"),
            (KeyCode::Left, KeyModifiers::CONTROL, b"\x1b[1;5D"),
            (KeyCode::Right, KeyModifiers::SHIFT | KeyModifiers::ALT, b"\x1b[1;4C"),
            (KeyCode::Home, none, b"\x1b[H"),
            (KeyCode::End, none, b"\x1b[F"),
            (KeyCode::Insert, none, b"\x1b[2~"),
            (KeyCode::Delete, none, b"\x1b[3~"),
            (KeyCode::PageUp, none, b"\x1b[5~"),
            (KeyCode::PageDown, KeyModifiers::CONTROL, b"\x1b[6;5~"),
            (KeyCode::F(1), none, b"\x1bOP"),
            (KeyCode::F(4), KeyModifiers::SHIFT, b"\x1b[1;2S"),
            (KeyCode::F(5), none, b"\x1b[15~"),
            (KeyCode::F(12), none, b"\x1b[24~"),
            (KeyCode::F(12), KeyModifiers::CONTROL, b"\x1b[24;5~"),
        ];
        for (code, modifiers, expected) in cases {
            assert_eq!(encoded(code, modifiers), expected, "{code:?} {modifiers:?}");
        }
    }

    #[test]
    fn application_cursor_mode_switches_arrows_to_ss3() {
        assert_eq!(encode_key(key(KeyCode::Up, KeyModifiers::NONE), true), b"\x1bOA");
        assert_eq!(encode_key(key(KeyCode::Home, KeyModifiers::NONE), true), b"\x1bOH");
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::CONTROL), true),
            b"\x1b[1;5A"
        );
    }

    #[test]
    fn key_release_sends_nothing() {
        let mut release = key(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(encode_key(release, false).is_empty());
    }

    #[test]
    fn paste_is_bracketed_only_when_requested() {
        assert_eq!(encode_paste("hi\nthere", false), b"hi\nthere");
        assert_eq!(encode_paste("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_is_sgr_encoded_relative_to_origin_and_gated_by_mode() {
        let sgr = MouseProtocolEncoding::Sgr;
        let press = mouse(MouseEventKind::Down(MouseButton::Left), 12, 7);
        assert_eq!(encode_mouse(press, (10, 5), MouseProtocolMode::None, sgr), None);
        assert_eq!(
            encode_mouse(press, (10, 5), MouseProtocolMode::PressRelease, sgr),
            Some(b"\x1b[<0;3;3M".to_vec())
        );
        assert_eq!(encode_mouse(press, (20, 5), MouseProtocolMode::PressRelease, sgr), None);

        let release = mouse(MouseEventKind::Up(MouseButton::Right), 0, 0);
        assert_eq!(
            encode_mouse(release, (0, 0), MouseProtocolMode::PressRelease, sgr),
            Some(b"\x1b[<2;1;1m".to_vec())
        );
        assert_eq!(encode_mouse(release, (0, 0), MouseProtocolMode::Press, sgr), None);

        let drag = mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0);
        assert_eq!(encode_mouse(drag, (0, 0), MouseProtocolMode::PressRelease, sgr), None);
        assert_eq!(
            encode_mouse(drag, (0, 0), MouseProtocolMode::ButtonMotion, sgr),
            Some(b"\x1b[<32;1;1M".to_vec())
        );

        let moved = mouse(MouseEventKind::Moved, 0, 0);
        assert_eq!(encode_mouse(moved, (0, 0), MouseProtocolMode::ButtonMotion, sgr), None);
        assert_eq!(
            encode_mouse(moved, (0, 0), MouseProtocolMode::AnyMotion, sgr),
            Some(b"\x1b[<35;1;1M".to_vec())
        );

        let mut scroll = mouse(MouseEventKind::ScrollUp, 0, 0);
        scroll.modifiers = KeyModifiers::CONTROL;
        assert_eq!(
            encode_mouse(scroll, (0, 0), MouseProtocolMode::PressRelease, sgr),
            Some(b"\x1b[<80;1;1M".to_vec())
        );
    }

    #[test]
    fn mouse_without_sgr_uses_legacy_encoding() {
        let press = mouse(MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(
            encode_mouse(press, (0, 0), MouseProtocolMode::PressRelease, MouseProtocolEncoding::Default),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
    }
}
