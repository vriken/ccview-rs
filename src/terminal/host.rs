use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};

use super::responder::{Responder, SharedState, SharedWriter};
use super::spec::{CommandSpec, TerminalOptions};
use super::state::{KeyboardFlags, TerminalEvent, TerminalState};

const SCROLLBACK_LINES: usize = 5000;
const READ_CHUNK_BYTES: usize = 64 * 1024;
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const HANGUP_GRACE: Duration = Duration::from_millis(300);
const READER_JOIN_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    pub code: u32,
    pub signal: Option<String>,
}

impl ExitStatus {
    pub fn success(&self) -> bool {
        self.code == 0 && self.signal.is_none()
    }
}

type Waker = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Signal {
    generation: u64,
    exit: Option<ExitStatus>,
    output_closed: bool,
}

struct Shared {
    parser: Mutex<vt100::Parser<Responder>>,
    signal: Mutex<Signal>,
    changed: Condvar,
    waker: Mutex<Option<Waker>>,
}

impl Shared {
    fn update_signal(&self, update: impl FnOnce(&mut Signal)) {
        {
            let mut signal = lock(&self.signal);
            update(&mut signal);
            signal.generation += 1;
        }
        self.changed.notify_all();
        if let Some(waker) = lock(&self.waker).as_ref() {
            waker();
        }
    }

    fn wait_until<T>(
        &self,
        timeout: Duration,
        mut ready: impl FnMut(&Signal) -> Option<T>,
    ) -> Option<T> {
        let deadline = Instant::now() + timeout;
        let mut signal = lock(&self.signal);
        loop {
            if let Some(value) = ready(&signal) {
                return Some(value);
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            signal = self
                .changed
                .wait_timeout(signal, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap_or(NonZeroU16::MIN)
}

fn to_io(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: nonzero(rows).get(),
        cols: nonzero(cols).get(),
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub struct PtyHost {
    shared: Arc<Shared>,
    writer: SharedWriter,
    state: SharedState,
    master: Mutex<Box<dyn MasterPty + Send>>,
    process_group: Option<i32>,
    reader: Option<JoinHandle<()>>,
    waiter: Option<JoinHandle<()>>,
}

pub struct Snapshot<'a> {
    parser: MutexGuard<'a, vt100::Parser<Responder>>,
}

impl Snapshot<'_> {
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn size(&self) -> Size {
        let (rows, cols) = self.screen().size();
        Size {
            rows: rows.get(),
            cols: cols.get(),
        }
    }

    pub fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.screen();
        if screen.hide_cursor() || screen.scrollback() > 0 {
            None
        } else {
            Some(screen.cursor_position())
        }
    }

    pub fn scrollback_offset(&self) -> usize {
        self.screen().scrollback()
    }

    pub fn scrollback_len(&self) -> usize {
        self.screen().scrollback_len()
    }

    pub fn text(&self) -> String {
        self.screen().contents()
    }
}

impl PtyHost {
    pub fn spawn(spec: &CommandSpec, size: Size) -> io::Result<Self> {
        Self::spawn_with_options(spec, size, TerminalOptions::default())
    }

    pub fn spawn_with_options(
        spec: &CommandSpec,
        size: Size,
        options: TerminalOptions,
    ) -> io::Result<Self> {
        let pair = native_pty_system()
            .openpty(pty_size(size.rows, size.cols))
            .map_err(to_io)?;

        let mut command = CommandBuilder::new(&spec.program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.cwd(cwd);
        }
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for key in &spec.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }

        let mut child = pair.slave.spawn_command(command).map_err(to_io)?;
        drop(pair.slave);
        let process_group = child.process_id().and_then(|pid| i32::try_from(pid).ok());
        let mut reader = pair.master.try_clone_reader().map_err(to_io)?;
        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer().map_err(to_io)?));

        let state: SharedState = Arc::new(Mutex::new(TerminalState::default()));
        let shared = Arc::new(Shared {
            parser: Mutex::new(vt100::Parser::new_with_callbacks(
                nonzero(size.rows),
                nonzero(size.cols),
                SCROLLBACK_LINES,
                Responder {
                    writer: Arc::clone(&writer),
                    state: Arc::clone(&state),
                    options,
                },
            )),
            signal: Mutex::new(Signal::default()),
            changed: Condvar::new(),
            waker: Mutex::new(None),
        });

        let reader_shared = Arc::clone(&shared);
        let reader_thread = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut chunk = vec![0u8; READ_CHUNK_BYTES];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            lock(&reader_shared.parser).process(&chunk[..read]);
                            reader_shared.update_signal(|_| {});
                        }
                    }
                }
                reader_shared.update_signal(|signal| signal.output_closed = true);
            })?;

        let waiter_shared = Arc::clone(&shared);
        let waiter_thread = std::thread::Builder::new()
            .name("pty-waiter".into())
            .spawn(move || {
                let status = child.wait();
                waiter_shared.wait_until(OUTPUT_DRAIN_GRACE, |signal| {
                    signal.output_closed.then_some(())
                });
                let exit = match status {
                    Ok(status) => ExitStatus {
                        code: status.exit_code(),
                        signal: status.signal().map(str::to_owned),
                    },
                    Err(_) => ExitStatus {
                        code: u32::MAX,
                        signal: None,
                    },
                };
                waiter_shared.update_signal(|signal| signal.exit = Some(exit));
            })?;

        Ok(Self {
            shared,
            writer,
            state,
            master: Mutex::new(pair.master),
            process_group,
            reader: Some(reader_thread),
            waiter: Some(waiter_thread),
        })
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process_group.and_then(|group| u32::try_from(group).ok())
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = lock(&self.writer);
        writer.write_all(bytes)?;
        writer.flush()
    }

    pub fn keyboard_flags(&self) -> KeyboardFlags {
        let alternate_screen = self.snapshot().screen().alternate_screen();
        lock(&self.state).keyboard_flags(alternate_screen)
    }

    pub fn focus_reporting(&self) -> bool {
        lock(&self.state).focus_reporting()
    }

    pub fn send_focus(&self, focused: bool) -> io::Result<()> {
        if !self.focus_reporting() {
            return Ok(());
        }
        self.write(if focused { b"\x1b[I" } else { b"\x1b[O" })
    }

    pub fn synchronized_output(&self) -> bool {
        lock(&self.state).synchronized_output()
    }

    pub fn take_events(&self) -> Vec<TerminalEvent> {
        lock(&self.state).take_events()
    }

    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        let mut parser = lock(&self.shared.parser);
        lock(&self.master)
            .resize(pty_size(rows, cols))
            .map_err(to_io)?;
        parser.set_size(nonzero(rows), nonzero(cols));
        drop(parser);
        self.shared.update_signal(|_| {});
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            parser: lock(&self.shared.parser),
        }
    }

    pub fn scroll_up(&self, lines: usize) {
        self.adjust_scrollback(|offset| offset.saturating_add(lines));
    }

    pub fn scroll_down(&self, lines: usize) {
        self.adjust_scrollback(|offset| offset.saturating_sub(lines));
    }

    pub fn reset_scroll(&self) {
        self.adjust_scrollback(|_| 0);
    }

    fn adjust_scrollback(&self, next_offset: impl FnOnce(usize) -> usize) {
        {
            let mut parser = lock(&self.shared.parser);
            let screen = parser.screen_mut();
            let offset = next_offset(screen.scrollback());
            screen.set_scrollback(offset);
        }
        self.shared.update_signal(|_| {});
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        lock(&self.shared.signal).exit.clone()
    }

    pub fn kill(&self) {
        if self.exit_status().is_none() {
            self.signal_group(libc::SIGKILL);
        }
    }

    pub fn generation(&self) -> u64 {
        lock(&self.shared.signal).generation
    }

    pub fn wait_for_change(&self, seen: u64, timeout: Duration) -> u64 {
        self.shared
            .wait_until(timeout, |signal| (signal.generation != seen).then_some(signal.generation));
        self.generation()
    }

    pub fn wait_for_exit(&self, timeout: Duration) -> Option<ExitStatus> {
        self.shared
            .wait_until(timeout, |signal| signal.exit.clone())
    }

    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.waker) = Some(Box::new(waker));
    }

    fn signal_group(&self, signal: libc::c_int) {
        if let Some(group) = self.process_group {
            unsafe {
                libc::kill(-group, signal);
            }
        }
    }
}

impl Drop for PtyHost {
    fn drop(&mut self) {
        if self.exit_status().is_none() {
            self.signal_group(libc::SIGHUP);
            if self.wait_for_exit(HANGUP_GRACE).is_none() {
                self.signal_group(libc::SIGKILL);
            }
        }
        if let Some(waiter) = self.waiter.take() {
            let _ = waiter.join();
        }
        let output_closed = self
            .shared
            .wait_until(READER_JOIN_GRACE, |signal| signal.output_closed.then_some(()))
            .is_some();
        if let Some(reader) = self.reader.take().filter(|_| output_closed) {
            let _ = reader.join();
        }
    }
}
