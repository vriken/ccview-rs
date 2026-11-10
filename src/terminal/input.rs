use std::io;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use super::session::TerminalSession;
use super::state::KeyboardFlags;

const ESC: u8 = 0x1b;
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const LEGACY_MOUSE_MAX_COORDINATE: u16 = 223;

pub fn encode_key_with_flags(
    key: KeyEvent,
    application_cursor: bool,
    flags: KeyboardFlags,
) -> Vec<u8> {
    if flags.is_empty() {
        return encode_key(key, application_cursor);
    }
    encode_kitty(key, application_cursor, flags).unwrap_or_default()
}

fn kitty_modifier_value(modifiers: KeyModifiers) -> u16 {
    let bits = [
        (KeyModifiers::SHIFT, 1),
        (KeyModifiers::ALT, 2),
        (KeyModifiers::CONTROL, 4),
        (KeyModifiers::SUPER, 8),
        (KeyModifiers::HYPER, 16),
        (KeyModifiers::META, 32),
    ];
    1 + bits
        .into_iter()
        .filter(|(modifier, _)| modifiers.contains(*modifier))
        .map(|(_, bit)| bit)
        .sum::<u16>()
}

fn kitty_modifier_field(modifier: u16, event: u8) -> String {
    match (modifier, event) {
        (1, 1) => String::new(),
        (_, 1) => format!(";{modifier}"),
        _ => format!(";{modifier}:{event}"),
    }
}

fn kitty_unicode_key(code: u32, modifier: u16, event: u8) -> Vec<u8> {
    format!("\x1b[{code}{}u", kitty_modifier_field(modifier, event)).into_bytes()
}

fn kitty_functional_key(
    number: Option<u8>,
    final_byte: char,
    modifier: u16,
    event: u8,
    legacy: impl FnOnce() -> Vec<u8>,
) -> Vec<u8> {
    if modifier == 1 && event == 1 {
        return legacy();
    }
    let field = kitty_modifier_field(modifier, event);
    let number = number.map_or_else(|| "1".to_string(), |n| n.to_string());
    format!("\x1b[{number}{field}{final_byte}").into_bytes()
}

fn encode_kitty(key: KeyEvent, application_cursor: bool, flags: KeyboardFlags) -> Option<Vec<u8>> {
    let report_all = flags.contains(KeyboardFlags::REPORT_ALL_KEYS);
    let event = match key.kind {
        KeyEventKind::Press => 1,
        KeyEventKind::Repeat if flags.contains(KeyboardFlags::EVENT_TYPES) => 2,
        KeyEventKind::Repeat => 1,
        KeyEventKind::Release if flags.contains(KeyboardFlags::EVENT_TYPES) => 3,
        KeyEventKind::Release => return None,
    };
    let mut modifiers = key.modifiers;
    if key.code == KeyCode::BackTab {
        modifiers |= KeyModifiers::SHIFT;
    }
    let modifier = kitty_modifier_value(modifiers);
    let plain = modifier == 1;
    let legacy_press = |bytes: Vec<u8>| (event != 3 && !report_all).then_some(bytes);

    let legacy_fallback = |code: KeyCode| encode_key(KeyEvent::new(code, key.modifiers), application_cursor);
    let functional = |number: Option<u8>, final_byte: char| {
        kitty_functional_key(number, final_byte, modifier, event, || {
            legacy_fallback(key.code)
        })
    };

    let bytes = match key.code {
        KeyCode::Esc => kitty_unicode_key(27, modifier, event),
        KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::BackTab => {
            let code = match key.code {
                KeyCode::Enter => 13,
                KeyCode::Backspace => 127,
                _ => 9,
            };
            if event == 3 && !report_all {
                return None;
            }
            let unmodified_legacy = plain && key.code != KeyCode::BackTab;
            if unmodified_legacy && !report_all {
                return legacy_press(legacy_fallback(key.code));
            }
            kitty_unicode_key(code, modifier, event)
        }
        KeyCode::Char(c) => {
            let text_only = modifiers.difference(KeyModifiers::SHIFT).is_empty();
            if text_only && !report_all {
                return legacy_press(legacy_fallback(key.code));
            }
            let unshifted = c.to_lowercase().next().unwrap_or(c);
            kitty_unicode_key(u32::from(unshifted), modifier, event)
        }
        KeyCode::Up => functional(None, 'A'),
        KeyCode::Down => functional(None, 'B'),
        KeyCode::Right => functional(None, 'C'),
        KeyCode::Left => functional(None, 'D'),
        KeyCode::Home => functional(None, 'H'),
        KeyCode::End => functional(None, 'F'),
        KeyCode::Insert => functional(Some(2), '~'),
        KeyCode::Delete => functional(Some(3), '~'),
        KeyCode::PageUp => functional(Some(5), '~'),
        KeyCode::PageDown => functional(Some(6), '~'),
        KeyCode::F(n @ 1..=4) => functional(None, (b'P' + n - 1) as char),
        KeyCode::F(n @ 5..=12) => {
            let number = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)];
            functional(Some(number), '~')
        }
        _ => return None,
    };
    Some(bytes)
}

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

pub fn send_key(host: &dyn TerminalSession, key: KeyEvent) -> io::Result<()> {
    let application_cursor = host.snapshot().screen().application_cursor();
    let bytes = encode_key_with_flags(key, application_cursor, host.keyboard_flags());
    if bytes.is_empty() {
        return Ok(());
    }
    host.write(&bytes)
}

pub fn send_paste(host: &dyn TerminalSession, text: &str) -> io::Result<()> {
    let bracketed = host.snapshot().screen().bracketed_paste();
    host.write(&encode_paste(text, bracketed))
}

pub fn send_mouse(host: &dyn TerminalSession, event: MouseEvent, origin: (u16, u16)) -> io::Result<bool> {
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

    fn kitty(code: KeyCode, modifiers: KeyModifiers, flags: KeyboardFlags) -> Vec<u8> {
        encode_key_with_flags(key(code, modifiers), false, flags)
    }

    #[test]
    fn disambiguate_flag_makes_modified_and_escape_keys_distinguishable() {
        let flags = KeyboardFlags::DISAMBIGUATE;
        let none = KeyModifiers::NONE;
        let cases: Vec<(KeyCode, KeyModifiers, &[u8])> = vec![
            (KeyCode::Esc, none, b"\x1b[27u"),
            (KeyCode::Char('b'), KeyModifiers::ALT, b"\x1b[98;3u"),
            (KeyCode::Char('c'), KeyModifiers::CONTROL, b"\x1b[99;5u"),
            (KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, b"\x1b[99;6u"),
            (KeyCode::Enter, KeyModifiers::SHIFT, b"\x1b[13;2u"),
            (KeyCode::Enter, KeyModifiers::CONTROL, b"\x1b[13;5u"),
            (KeyCode::Tab, KeyModifiers::CONTROL, b"\x1b[9;5u"),
            (KeyCode::BackTab, KeyModifiers::SHIFT, b"\x1b[9;2u"),
            (KeyCode::Backspace, KeyModifiers::ALT, b"\x1b[127;3u"),
            (KeyCode::Char(' '), KeyModifiers::CONTROL, b"\x1b[32;5u"),
            (KeyCode::Char('a'), KeyModifiers::SUPER, b"\x1b[97;9u"),
            (KeyCode::Enter, none, b"\r"),
            (KeyCode::Tab, none, b"\t"),
            (KeyCode::Backspace, none, b"\x7f"),
            (KeyCode::Char('a'), none, b"a"),
            (KeyCode::Char('A'), KeyModifiers::SHIFT, b"A"),
            (KeyCode::Up, none, b"\x1b[A"),
            (KeyCode::Left, KeyModifiers::CONTROL, b"\x1b[1;5D"),
            (KeyCode::Delete, KeyModifiers::SHIFT, b"\x1b[3;2~"),
            (KeyCode::F(1), none, b"\x1bOP"),
            (KeyCode::F(5), KeyModifiers::ALT, b"\x1b[15;3~"),
        ];
        for (code, modifiers, expected) in cases {
            assert_eq!(kitty(code, modifiers, flags), expected, "{code:?} {modifiers:?}");
        }
    }

    #[test]
    fn report_all_keys_flag_escapes_plain_text_and_editing_keys() {
        let flags = KeyboardFlags::DISAMBIGUATE | KeyboardFlags::REPORT_ALL_KEYS;
        let none = KeyModifiers::NONE;
        assert_eq!(kitty(KeyCode::Char('a'), none, flags), b"\x1b[97u");
        assert_eq!(kitty(KeyCode::Char('A'), KeyModifiers::SHIFT, flags), b"\x1b[97;2u");
        assert_eq!(kitty(KeyCode::Enter, none, flags), b"\x1b[13u");
        assert_eq!(kitty(KeyCode::Backspace, none, flags), b"\x1b[127u");
    }

    #[test]
    fn event_types_flag_reports_repeat_and_release_where_kitty_does() {
        let flags = KeyboardFlags::DISAMBIGUATE | KeyboardFlags::EVENT_TYPES;
        let with_kind = |code, modifiers, kind| {
            let mut event = key(code, modifiers);
            event.kind = kind;
            encode_key_with_flags(event, false, flags)
        };
        let none = KeyModifiers::NONE;
        assert_eq!(with_kind(KeyCode::Up, none, KeyEventKind::Release), b"\x1b[1;1:3A");
        assert_eq!(with_kind(KeyCode::Up, none, KeyEventKind::Repeat), b"\x1b[1;1:2A");
        assert_eq!(
            with_kind(KeyCode::Char('c'), KeyModifiers::CONTROL, KeyEventKind::Release),
            b"\x1b[99;5:3u"
        );
        assert!(with_kind(KeyCode::Enter, none, KeyEventKind::Release).is_empty());
        assert!(with_kind(KeyCode::Char('a'), none, KeyEventKind::Release).is_empty());
        assert_eq!(with_kind(KeyCode::Char('a'), none, KeyEventKind::Repeat), b"a");
    }

    #[test]
    fn without_event_types_release_is_silent_even_when_kitty_is_on() {
        let mut release = key(KeyCode::Up, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(encode_key_with_flags(release, false, KeyboardFlags::DISAMBIGUATE).is_empty());
    }

    #[test]
    fn empty_flags_use_legacy_encoding() {
        assert_eq!(kitty(KeyCode::Enter, KeyModifiers::SHIFT, KeyboardFlags::NONE), b"\r");
        assert_eq!(kitty(KeyCode::Esc, KeyModifiers::NONE, KeyboardFlags::NONE), b"\x1b");
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
