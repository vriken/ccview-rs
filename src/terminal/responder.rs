use std::io::Write;
use std::sync::{Arc, Mutex};

use super::host::lock;
use super::spec::{Rgb, TerminalOptions};
use super::state::{KeyboardFlags, TerminalEvent, TerminalState, decode_base64};

pub(super) type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;
pub(super) type SharedState = Arc<Mutex<TerminalState>>;

const DECRQM_NOT_RECOGNIZED: u8 = 0;
const DECRQM_SET: u8 = 1;
const DECRQM_RESET: u8 = 2;
const MODE_FOCUS_REPORTING: u16 = 1004;
const MODE_SYNCHRONIZED_OUTPUT: u16 = 2026;
const OSC_9_PROGRESS: &[u8] = b"4";

pub(super) struct Responder {
    pub(super) writer: SharedWriter,
    pub(super) state: SharedState,
    pub(super) options: TerminalOptions,
}

impl Responder {
    fn reply(&self, text: &str) {
        let mut writer = lock(&self.writer);
        let _ = writer.write_all(text.as_bytes());
        let _ = writer.flush();
    }

    fn push_event(&self, event: TerminalEvent) {
        lock(&self.state).push_event(event);
    }

    fn mode_status(&self, screen: &vt100::Screen, mode: u16) -> u8 {
        use vt100::{MouseProtocolEncoding, MouseProtocolMode};
        let status = |set: bool| if set { DECRQM_SET } else { DECRQM_RESET };
        match mode {
            1 => status(screen.application_cursor()),
            25 => status(!screen.hide_cursor()),
            47 | 1049 => status(screen.alternate_screen()),
            1000 => status(screen.mouse_protocol_mode() == MouseProtocolMode::PressRelease),
            1002 => status(screen.mouse_protocol_mode() == MouseProtocolMode::ButtonMotion),
            1003 => status(screen.mouse_protocol_mode() == MouseProtocolMode::AnyMotion),
            1006 => status(screen.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr),
            2004 => status(screen.bracketed_paste()),
            MODE_FOCUS_REPORTING => status(lock(&self.state).focus_reporting()),
            MODE_SYNCHRONIZED_OUTPUT => status(lock(&self.state).synchronized_output()),
            _ => DECRQM_NOT_RECOGNIZED,
        }
    }

    fn private_mode_change(&self, params: &[&[u16]], enabled: bool) {
        let mut state = lock(&self.state);
        for param in params.iter().filter_map(|p| p.first()) {
            match *param {
                MODE_FOCUS_REPORTING => state.set_focus_reporting(enabled),
                MODE_SYNCHRONIZED_OUTPUT => state.set_synchronized_output(enabled),
                _ => {}
            }
        }
    }

    fn keyboard_protocol(&self, screen: &vt100::Screen, marker: u8, params: &[&[u16]]) {
        let alternate = screen.alternate_screen();
        let param = |index: usize| params.get(index).and_then(|p| p.first()).copied().unwrap_or(0);
        let or_default = |value: u16, default: u16| if value == 0 { default } else { value };
        let flags = KeyboardFlags::from_bits(u8::try_from(param(0)).unwrap_or(0));
        match marker {
            b'?' => {
                let current = lock(&self.state).keyboard_flags(alternate);
                self.reply(&format!("\x1b[?{}u", current.bits()));
            }
            b'>' => lock(&self.state).push_keyboard_flags(alternate, flags),
            b'<' => lock(&self.state)
                .pop_keyboard_flags(alternate, usize::from(or_default(param(0), 1))),
            b'=' => lock(&self.state).set_keyboard_flags(alternate, flags, or_default(param(1), 1)),
            _ => {}
        }
    }

    fn window_report(&self, screen: &vt100::Screen, operation: u16) {
        let (rows, cols) = screen.size();
        let (rows, cols) = (rows.get(), cols.get());
        let (cell_width, cell_height) = self.options.cell_pixels;
        match operation {
            14 => self.reply(&format!(
                "\x1b[4;{};{}t",
                u32::from(rows) * u32::from(cell_height),
                u32::from(cols) * u32::from(cell_width)
            )),
            16 => self.reply(&format!("\x1b[6;{cell_height};{cell_width}t")),
            18 => self.reply(&format!("\x1b[8;{rows};{cols}t")),
            _ => {}
        }
    }

    fn color_report(&self, code: &str, color: Rgb) {
        let Rgb(red, green, blue) = color;
        self.reply(&format!(
            "\x1b]{code};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
            u16::from(red) * 257,
            u16::from(green) * 257,
            u16::from(blue) * 257
        ));
    }

    fn palette_queries(&self, params: &[&[u8]]) {
        for pair in params.chunks(2) {
            let [index, query] = pair else { continue };
            if *query != b"?" {
                continue;
            }
            let Some(index) = std::str::from_utf8(index)
                .ok()
                .and_then(|text| text.parse::<u8>().ok())
            else {
                continue;
            };
            self.color_report(&format!("4;{index}"), self.options.palette_color(index));
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn joined(params: &[&[u8]]) -> String {
    params.iter().map(|p| text(p)).collect::<Vec<_>>().join(";")
}

impl vt100::Callbacks for Responder {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.push_event(TerminalEvent::Bell);
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.push_event(TerminalEvent::Title(text(title)));
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, selection: &[u8], data: &[u8]) {
        if let Some(data) = decode_base64(data) {
            self.push_event(TerminalEvent::Clipboard {
                selection: text(selection),
                data,
            });
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            [b"10", b"?", ..] => self.color_report("10", self.options.foreground),
            [b"11", b"?", ..] => self.color_report("11", self.options.background),
            [b"4", rest @ ..] => self.palette_queries(rest),
            [b"9", subcommand, ..] if *subcommand == OSC_9_PROGRESS => {}
            [b"9", body @ ..] => self.push_event(TerminalEvent::Notification {
                title: None,
                body: joined(body),
            }),
            [b"777", b"notify", title, body @ ..] => {
                self.push_event(TerminalEvent::Notification {
                    title: Some(text(title)),
                    body: joined(body),
                })
            }
            _ => {}
        }
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        intermediate: Option<u8>,
        second_intermediate: Option<u8>,
        params: &[&[u16]],
        action: char,
    ) {
        let first_param = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (intermediate, second_intermediate, action) {
            (None, None, 'n') if first_param == 5 => self.reply("\x1b[0n"),
            (None, None, 'n') if first_param == 6 => {
                let (row, col) = screen.cursor_position();
                self.reply(&format!("\x1b[{};{}R", row + 1, col + 1));
            }
            (None, None, 'c') if first_param == 0 => self.reply("\x1b[?62;22c"),
            (None, None, 't') => self.window_report(screen, first_param),
            (Some(b'>'), None, 'q') if first_param == 0 => {
                self.reply(&format!("\x1bP>|ccview-rs {}\x1b\\", env!("CARGO_PKG_VERSION")));
            }
            (Some(marker @ (b'?' | b'>' | b'<' | b'=')), None, 'u') => {
                self.keyboard_protocol(screen, marker, params);
            }
            (Some(b'?'), Some(b'$'), 'p') => {
                let status = self.mode_status(screen, first_param);
                self.reply(&format!("\x1b[?{first_param};{status}$y"));
            }
            (Some(b'?'), None, 'h') => self.private_mode_change(params, true),
            (Some(b'?'), None, 'l') => self.private_mode_change(params, false),
            _ => {}
        }
    }
}
