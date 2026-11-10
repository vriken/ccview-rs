use std::io;

use super::host::{ExitStatus, PtyHost, Snapshot};
use super::state::{KeyboardFlags, TerminalEvent};

pub type Waker = Box<dyn Fn() + Send + Sync>;

pub trait TerminalSession: Send + Sync {
    fn snapshot(&self) -> Snapshot<'_>;
    fn write(&self, bytes: &[u8]) -> io::Result<()>;
    fn keyboard_flags(&self) -> KeyboardFlags;
    fn focus_reporting(&self) -> bool;
    fn take_events(&self) -> Vec<TerminalEvent>;
    fn resize(&self, rows: u16, cols: u16) -> io::Result<()>;
    fn scroll_up(&self, lines: usize);
    fn scroll_down(&self, lines: usize);
    fn reset_scroll(&self);
    fn exit_status(&self) -> Option<ExitStatus>;
    fn process_id(&self) -> Option<u32>;
    fn set_waker(&self, waker: Waker);

    fn send_focus(&self, focused: bool) -> io::Result<()> {
        if !self.focus_reporting() {
            return Ok(());
        }
        self.write(if focused { b"\x1b[I" } else { b"\x1b[O" })
    }
}

impl TerminalSession for PtyHost {
    fn snapshot(&self) -> Snapshot<'_> {
        PtyHost::snapshot(self)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        PtyHost::write(self, bytes)
    }

    fn keyboard_flags(&self) -> KeyboardFlags {
        PtyHost::keyboard_flags(self)
    }

    fn focus_reporting(&self) -> bool {
        PtyHost::focus_reporting(self)
    }

    fn take_events(&self) -> Vec<TerminalEvent> {
        PtyHost::take_events(self)
    }

    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        PtyHost::resize(self, rows, cols)
    }

    fn scroll_up(&self, lines: usize) {
        PtyHost::scroll_up(self, lines);
    }

    fn scroll_down(&self, lines: usize) {
        PtyHost::scroll_down(self, lines);
    }

    fn reset_scroll(&self) {
        PtyHost::reset_scroll(self);
    }

    fn exit_status(&self) -> Option<ExitStatus> {
        PtyHost::exit_status(self)
    }

    fn process_id(&self) -> Option<u32> {
        PtyHost::process_id(self)
    }

    fn set_waker(&self, waker: Waker) {
        PtyHost::set_waker(self, move || waker());
    }
}
