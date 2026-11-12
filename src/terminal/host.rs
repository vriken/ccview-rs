use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};

use super::responder::{Responder, SharedState, SharedWriter};
use super::spec::TerminalOptions;
use super::state::{KeyboardFlags, ModeSnapshot, TerminalEvent, TerminalState};
use crate::sys::command::CommandSpec;

const SCROLLBACK_LINES: usize = 5000;
const READ_CHUNK_BYTES: usize = 64 * 1024;
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const HANGUP_GRACE: Duration = Duration::from_millis(300);
const READER_JOIN_GRACE: Duration = Duration::from_millis(500);
const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
const SUBSCRIBER_QUEUE_CHUNKS: usize = 4096;
const INPUT_QUEUE_CHUNKS: usize = 256;
const OPENPTY_ATTEMPTS: u32 = 5;
const OPENPTY_BACKOFF: Duration = Duration::from_millis(20);

type BoxedChild = Box<dyn portable_pty::Child + Send + Sync>;
type ChildSlot = Arc<Mutex<Option<BoxedChild>>>;

fn retry<T, E>(
    attempts: u32,
    backoff: Duration,
    mut operation: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut attempt = 1;
    loop {
        match operation() {
            Err(_) if attempt < attempts => {
                std::thread::sleep(backoff * attempt);
                attempt += 1;
            }
            result => return result,
        }
    }
}

fn kill_group(group: Option<i32>) {
    if let Some(group) = group {
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

struct SpawnGuard {
    slot: ChildSlot,
    group: Option<i32>,
    armed: bool,
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        kill_group(self.group);
        if let Some(mut child) = lock(&self.slot).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Data(Vec<u8>),
    Exit(ExitStatus),
}

pub struct Attachment {
    pub screen: Vec<u8>,
    pub modes: ModeSnapshot,
    pub size: Size,
    pub exit: Option<ExitStatus>,
    pub output: Receiver<Output>,
}

#[derive(Default)]
struct Signal {
    generation: u64,
    exit: Option<ExitStatus>,
    output_closed: bool,
}

struct Shared {
    reaped: AtomicBool,
    parser: Mutex<vt100::Parser<Responder>>,
    signal: Mutex<Signal>,
    changed: Condvar,
    waker: Mutex<Option<Waker>>,
    subscribers: Mutex<Vec<SyncSender<Output>>>,
}

impl Shared {
    fn broadcast(&self, output: impl Fn() -> Output) {
        let mut subscribers = lock(&self.subscribers);
        if subscribers.is_empty() {
            return;
        }
        subscribers.retain(|subscriber| match subscriber.try_send(output()) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        });
    }

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

const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;
const RESET_ATTRIBUTES: &[u8] = b"\x1b[0m";

pub fn history(screen: &vt100::Screen) -> Vec<u8> {
    capped_history(screen, MAX_HISTORY_BYTES)
}

fn capped_history(screen: &vt100::Screen, max_bytes: usize) -> Vec<u8> {
    if screen.alternate_screen() || screen.scrollback_len() == 0 {
        return Vec::new();
    }
    let mut text = String::new();
    let mut state = vt100::capture::BasicFormattedCaptureState::new();
    let _ = screen.write_contents_formatted_basic(
        &mut text,
        vt100::capture::BasicFormattedCaptureRange::Full(&mut state),
    );
    let mut bytes = text.replace('\n', "\r\n").into_bytes();
    if bytes.len() > max_bytes {
        let from = bytes.len() - max_bytes;
        let start = bytes[from..]
            .windows(2)
            .position(|pair| pair == b"\r\n")
            .map_or(bytes.len(), |offset| from + offset + 2);
        bytes.drain(..start);
        bytes.splice(0..0, RESET_ATTRIBUTES.iter().copied());
    }
    bytes.extend_from_slice(RESET_ATTRIBUTES);
    bytes
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

impl<'a> Snapshot<'a> {
    pub(super) fn new(parser: MutexGuard<'a, vt100::Parser<Responder>>) -> Self {
        Self { parser }
    }
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

fn open_pty(size: Size) -> io::Result<portable_pty::PtyPair> {
    retry(OPENPTY_ATTEMPTS, OPENPTY_BACKOFF, || {
        native_pty_system().openpty(pty_size(size.rows, size.cols))
    })
    .map_err(to_io)
}

fn build_command(spec: &CommandSpec) -> CommandBuilder {
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
    command
}

fn start_writer(mut writer: Box<dyn Write + Send>) -> io::Result<SharedWriter> {
    let (sender, queue) = sync_channel::<Vec<u8>>(INPUT_QUEUE_CHUNKS);
    std::thread::Builder::new()
        .name("pty-writer".into())
        .spawn(move || {
            for chunk in queue {
                if writer
                    .write_all(&chunk)
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })?;
    Ok(sender)
}

fn start_reader(
    mut reader: Box<dyn Read + Send>,
    reader_shared: Arc<Shared>,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut chunk = vec![0u8; READ_CHUNK_BYTES];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        {
                            let mut parser = lock(&reader_shared.parser);
                            parser.process(&chunk[..read]);
                            reader_shared.broadcast(|| Output::Data(chunk[..read].to_vec()));
                        }
                        reader_shared.update_signal(|_| {});
                    }
                }
            }
            reader_shared.update_signal(|signal| signal.output_closed = true);
        })
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
        let pair = open_pty(size)?;
        let command = build_command(spec);

        let child = pair.slave.spawn_command(command).map_err(to_io)?;
        drop(pair.slave);
        let process_group = child.process_id().and_then(|pid| i32::try_from(pid).ok());
        let slot: ChildSlot = Arc::new(Mutex::new(Some(child)));
        let mut guard = SpawnGuard {
            slot: Arc::clone(&slot),
            group: process_group,
            armed: true,
        };
        let reader = pair.master.try_clone_reader().map_err(to_io)?;
        let writer = start_writer(pair.master.take_writer().map_err(to_io)?)?;

        let state: SharedState = Arc::new(Mutex::new(TerminalState::default()));
        let shared = Arc::new(Shared {
            reaped: AtomicBool::new(false),
            parser: Mutex::new(vt100::Parser::new_with_callbacks(
                nonzero(size.rows),
                nonzero(size.cols),
                SCROLLBACK_LINES,
                Responder {
                    writer: writer.clone(),
                    state: Arc::clone(&state),
                    options,
                },
            )),
            signal: Mutex::new(Signal::default()),
            changed: Condvar::new(),
            waker: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
        });

        let reader_thread = start_reader(reader, Arc::clone(&shared))?;

        let waiter_shared = Arc::clone(&shared);
        let waiter_thread = std::thread::Builder::new()
            .name("pty-waiter".into())
            .spawn(move || {
                let child = lock(&slot).take();
                let status = match child {
                    Some(mut child) => child.wait(),
                    None => Err(io::Error::other("child already taken")),
                };
                waiter_shared.reaped.store(true, Ordering::Release);
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
                waiter_shared.update_signal(|signal| signal.exit = Some(exit.clone()));
                waiter_shared.broadcast(|| Output::Exit(exit.clone()));
            })?;

        guard.armed = false;
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
        self.process_group
            .and_then(|group| u32::try_from(group).ok())
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.writer
            .send(bytes.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
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

    pub fn attach(&self, resize_to: Option<Size>) -> io::Result<Attachment> {
        let mut parser = lock(&self.shared.parser);
        if let Some(size) = resize_to {
            lock(&self.master)
                .resize(pty_size(size.rows, size.cols))
                .map_err(to_io)?;
            parser.set_size(nonzero(size.rows), nonzero(size.cols));
        }
        let mut screen = history(parser.screen());
        if parser.screen().alternate_screen() {
            screen.extend_from_slice(ENTER_ALTERNATE_SCREEN);
        }
        screen.extend_from_slice(parser.screen().state_formatted().as_bytes());
        let (rows, cols) = parser.screen().size();
        let modes = lock(&self.state).export();
        let (sender, output) = sync_channel(SUBSCRIBER_QUEUE_CHUNKS);
        let exit = {
            let mut subscribers = lock(&self.shared.subscribers);
            let exit = self.exit_status();
            if exit.is_none() {
                subscribers.push(sender);
            }
            exit
        };
        drop(parser);
        Ok(Attachment {
            screen,
            modes,
            size: Size {
                rows: rows.get(),
                cols: cols.get(),
            },
            exit,
            output,
        })
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
        self.shared.wait_until(timeout, |signal| {
            (signal.generation != seen).then_some(signal.generation)
        });
        self.generation()
    }

    pub fn wait_for_exit(&self, timeout: Duration) -> Option<ExitStatus> {
        self.shared
            .wait_until(timeout, |signal| signal.exit.clone())
    }

    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.waker) = Some(Box::new(waker));
    }

    fn reaped(&self) -> bool {
        self.shared.reaped.load(Ordering::Acquire)
    }

    pub fn terminate(&self) -> io::Result<()> {
        if self.reaped() {
            return Ok(());
        }
        let pid = self
            .process_id()
            .ok_or_else(|| io::Error::other("session has no process"))?;
        crate::sys::process::terminate(pid)
    }

    fn signal_group(&self, signal: libc::c_int) {
        if let Some(group) = self.process_group {
            unsafe {
                libc::kill(-group, signal);
            }
        }
    }
}

impl PtyHost {
    pub fn tear_down(&self) {
        if self.exit_status().is_none() {
            if !self.reaped() {
                self.signal_group(libc::SIGHUP);
                self.wait_for_exit(HANGUP_GRACE);
            }
            self.signal_group(libc::SIGKILL);
        }
    }
}

impl Drop for PtyHost {
    fn drop(&mut self) {
        self.tear_down();
        if let Some(waiter) = self.waiter.take() {
            let _ = waiter.join();
        }
        let output_closed = self
            .shared
            .wait_until(READER_JOIN_GRACE, |signal| {
                signal.output_closed.then_some(())
            })
            .is_some();
        if let Some(reader) = self.reader.take().filter(|_| output_closed) {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn history_over_the_cap_keeps_the_newest_rows_and_starts_on_a_row_boundary() {
        let mut parser = vt100::Parser::new(nonzero(5), nonzero(20), 1000);
        for row in 1..=400 {
            parser.process(format!("row-{row}\r\n").as_bytes());
        }
        let full = capped_history(parser.screen(), usize::MAX);
        let capped = capped_history(parser.screen(), 600);
        assert!(capped.len() <= 600 + 2 * RESET_ATTRIBUTES.len());
        assert!(capped.len() < full.len());
        let text = String::from_utf8(capped).unwrap();
        assert!(text.contains("row-400"));
        assert!(!text.contains("row-1\r\n"));
        let first_row = text.trim_start_matches("\x1b[0m");
        assert!(first_row.starts_with("row-"), "{first_row:?}");
    }

    use super::*;
    use std::cell::Cell;

    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    fn wait_until_dead(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_transient_openpty_failure_is_retried_until_it_succeeds() {
        let calls = Cell::new(0);
        let result = retry(OPENPTY_ATTEMPTS, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            if calls.get() < OPENPTY_ATTEMPTS {
                Err("busy")
            } else {
                Ok(calls.get())
            }
        });
        assert_eq!(result, Ok(OPENPTY_ATTEMPTS));
    }

    #[test]
    fn a_persistent_openpty_failure_is_reported_after_the_last_attempt() {
        let calls = Cell::new(0);
        let result: Result<(), &str> = retry(OPENPTY_ATTEMPTS, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Err("busy")
        });
        assert_eq!((result, calls.get()), (Err("busy"), OPENPTY_ATTEMPTS));
    }

    #[test]
    fn a_failed_spawn_does_not_leave_the_child_running() {
        let pair = native_pty_system().openpty(pty_size(5, 20)).unwrap();
        let mut command = CommandBuilder::new("sleep");
        command.arg("60");
        let child = pair.slave.spawn_command(command).unwrap();
        let pid = child.process_id().unwrap();
        let group = i32::try_from(pid).ok();
        let guard = SpawnGuard {
            slot: Arc::new(Mutex::new(Some(child))),
            group,
            armed: true,
        };
        assert!(alive(pid));
        drop(guard);
        assert!(wait_until_dead(pid));
    }
}
