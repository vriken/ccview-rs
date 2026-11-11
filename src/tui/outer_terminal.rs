use crate::terminal::TerminalOptions;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    queue,
};
use std::io::{self, Write};

pub(crate) const CLIPBOARD_LIMIT_BYTES: usize = 100 * 1024;
const BELL: u8 = 0x07;
const KEYBOARD_FLAGS: KeyboardEnhancementFlags =
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        .union(KeyboardEnhancementFlags::REPORT_EVENT_TYPES);
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub(crate) keyboard_enhancement: bool,
}

pub(crate) fn detect() -> Capabilities {
    Capabilities {
        keyboard_enhancement: crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false),
    }
}

pub(crate) fn enter(out: &mut impl Write, capabilities: Capabilities) -> io::Result<()> {
    if capabilities.keyboard_enhancement {
        queue!(out, PushKeyboardEnhancementFlags(KEYBOARD_FLAGS))?;
    }
    queue!(out, EnableBracketedPaste, EnableFocusChange)?;
    out.flush()
}

pub(crate) fn leave(out: &mut impl Write, capabilities: Capabilities) -> io::Result<()> {
    queue!(
        out,
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture
    )?;
    if capabilities.keyboard_enhancement {
        queue!(out, PopKeyboardEnhancementFlags)?;
    }
    out.flush()
}

pub(crate) fn install_panic_hook(capabilities: Capabilities) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = leave(&mut io::stdout(), capabilities);
        previous(info);
    }));
}

pub(crate) fn terminal_options() -> TerminalOptions {
    let defaults = TerminalOptions::default();
    TerminalOptions {
        cell_pixels: crossterm::terminal::window_size()
            .ok()
            .and_then(|size| cell_pixels(size.columns, size.rows, size.width, size.height))
            .unwrap_or(defaults.cell_pixels),
        ..defaults
    }
}

pub(crate) fn cell_pixels(columns: u16, rows: u16, width: u16, height: u16) -> Option<(u16, u16)> {
    if columns == 0 || rows == 0 || width == 0 || height == 0 {
        return None;
    }
    Some((width / columns, height / rows))
}

pub(crate) fn bell() -> Vec<u8> {
    vec![BELL]
}

pub(crate) fn clipboard_sequence(selection: &str, data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > CLIPBOARD_LIMIT_BYTES {
        return None;
    }
    let mut targets: String = selection
        .chars()
        .filter(|c| matches!(c, 'c' | 'p' | 'q' | 's' | '0'..='7'))
        .collect();
    if targets.is_empty() {
        targets.push('c');
    }
    let mut sequence = format!("\x1b]52;{targets};").into_bytes();
    sequence.extend(base64_encode(data));
    sequence.extend_from_slice(b"\x1b\\");
    Some(sequence)
}

fn base64_encode(data: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |acc, (index, &byte)| {
            acc | u32::from(byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                encoded.push(BASE64_ALPHABET[(group >> (18 - 6 * index) & 0x3f) as usize]);
            } else {
                encoded.push(b'=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_KEYBOARD: Capabilities = Capabilities {
        keyboard_enhancement: true,
    };
    const WITHOUT_KEYBOARD: Capabilities = Capabilities {
        keyboard_enhancement: false,
    };

    fn bytes(run: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        run(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn enter_pushes_keyboard_flags_first_and_leave_pops_them_last() {
        let entered = bytes(|out| enter(out, WITH_KEYBOARD));
        let left = bytes(|out| leave(out, WITH_KEYBOARD));
        let at = |text: &str, needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle:?} in {text:?}"))
        };
        assert!(at(&entered, "\x1b[>3u") < at(&entered, "\x1b[?2004h"));
        assert!(at(&entered, "\x1b[?2004h") < at(&entered, "\x1b[?1004h"));
        assert!(at(&left, "\x1b[?1004l") < at(&left, "\x1b[?2004l"));
        assert!(at(&left, "\x1b[?2004l") < at(&left, "\x1b[?1000l"));
        assert!(at(&left, "\x1b[?1000l") < at(&left, "\x1b[<1u"));
        assert!(left.ends_with("\x1b[<1u"));
    }

    #[test]
    fn without_keyboard_enhancement_nothing_is_pushed_or_popped() {
        let entered = bytes(|out| enter(out, WITHOUT_KEYBOARD));
        let left = bytes(|out| leave(out, WITHOUT_KEYBOARD));
        assert!(!entered.contains("\x1b[>") && !left.contains("\x1b[<"));
        assert!(entered.contains("\x1b[?1004h") && left.contains("\x1b[?1004l"));
    }

    #[test]
    fn clipboard_is_reencoded_and_capped() {
        assert_eq!(
            clipboard_sequence("c", b"hello").unwrap(),
            b"\x1b]52;c;aGVsbG8=\x1b\\"
        );
        assert_eq!(
            clipboard_sequence("", b"ab").unwrap(),
            b"\x1b]52;c;YWI=\x1b\\"
        );
        assert_eq!(
            clipboard_sequence("x\x1b!", b"a").unwrap(),
            b"\x1b]52;c;YQ==\x1b\\"
        );
        assert!(clipboard_sequence("c", &vec![0; CLIPBOARD_LIMIT_BYTES + 1]).is_none());
        assert!(clipboard_sequence("c", &vec![0; CLIPBOARD_LIMIT_BYTES]).is_some());
    }
}
