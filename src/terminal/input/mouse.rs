use super::key::ESC;
use crate::terminal::session::TerminalSession;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::io;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

const LEGACY_MOUSE_MAX_COORDINATE: u16 = 223;

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
    Some(vec![ESC, b'[', b'M', code + 32, x as u8 + 32, y as u8 + 32])
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
            MouseEventKind::Drag(button) => {
                reports_drag.then_some((button_number(button) + 32, false))
            }
            MouseEventKind::Moved => reports_motion.then_some((35, false)),
            MouseEventKind::ScrollUp => Some((64, false)),
            MouseEventKind::ScrollDown => Some((65, false)),
            MouseEventKind::ScrollLeft => Some((66, false)),
            MouseEventKind::ScrollRight => Some((67, false)),
        },
    }
}

pub fn send_mouse(
    host: &dyn TerminalSession,
    event: MouseEvent,
    origin: (u16, u16),
) -> io::Result<bool> {
    let (mode, encoding) = {
        let snapshot = host.snapshot();
        let screen = snapshot.screen();
        (
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding(),
        )
    };
    match encode_mouse(event, origin, mode, encoding) {
        Some(bytes) => host.write(&bytes).map(|()| true),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            encode_mouse(press, (10, 5), MouseProtocolMode::None, sgr),
            None
        );
        assert_eq!(
            encode_mouse(press, (10, 5), MouseProtocolMode::PressRelease, sgr),
            Some(b"\x1b[<0;3;3M".to_vec())
        );
        assert_eq!(
            encode_mouse(press, (20, 5), MouseProtocolMode::PressRelease, sgr),
            None
        );

        let release = mouse(MouseEventKind::Up(MouseButton::Right), 0, 0);
        assert_eq!(
            encode_mouse(release, (0, 0), MouseProtocolMode::PressRelease, sgr),
            Some(b"\x1b[<2;1;1m".to_vec())
        );
        assert_eq!(
            encode_mouse(release, (0, 0), MouseProtocolMode::Press, sgr),
            None
        );

        let drag = mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0);
        assert_eq!(
            encode_mouse(drag, (0, 0), MouseProtocolMode::PressRelease, sgr),
            None
        );
        assert_eq!(
            encode_mouse(drag, (0, 0), MouseProtocolMode::ButtonMotion, sgr),
            Some(b"\x1b[<32;1;1M".to_vec())
        );

        let moved = mouse(MouseEventKind::Moved, 0, 0);
        assert_eq!(
            encode_mouse(moved, (0, 0), MouseProtocolMode::ButtonMotion, sgr),
            None
        );
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
            encode_mouse(
                press,
                (0, 0),
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default
            ),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
    }
}
