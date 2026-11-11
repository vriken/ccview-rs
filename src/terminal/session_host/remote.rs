use std::io;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use super::protocol::{Frame, Request, Response, read_frame, write_bytes, write_message};
use crate::terminal::host::{ExitStatus, Size, Snapshot, lock};
use crate::terminal::responder::{Responder, SharedState};
use crate::terminal::session::{TerminalSession, Waker};
use crate::terminal::spec::TerminalOptions;
use crate::terminal::state::{KeyboardFlags, ModeSnapshot, TerminalEvent, TerminalState};

const SCROLLBACK_LINES: usize = 5000;
const HOST_LOST_SIGNAL: &str = "host lost";
const OUTGOING_QUEUE: usize = 256;
const INPUT_CHUNK_BYTES: usize = 64 * 1024;

enum Outgoing {
    Input(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Detach,
}

type SharedExit = Arc<Mutex<Option<ExitStatus>>>;
type SharedWaker = Arc<Mutex<Option<Waker>>>;
type SharedStandby = Arc<AtomicBool>;
type SharedParser = Arc<Mutex<vt100::Parser<Responder>>>;

pub struct RemoteHost {
    parser: SharedParser,
    state: SharedState,
    outgoing: SyncSender<Outgoing>,
    stream: UnixStream,
    exit: SharedExit,
    waker: SharedWaker,
    standby: SharedStandby,
    pid: Option<u32>,
}

fn nonzero(value: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(value).unwrap_or(std::num::NonZeroU16::MIN)
}

fn wake(waker: &SharedWaker) {
    if let Some(waker) = lock(waker).as_ref() {
        waker();
    }
}

impl RemoteHost {
    pub(super) fn new(
        stream: UnixStream,
        size: Size,
        pid: Option<u32>,
        screen: &[u8],
        modes: ModeSnapshot,
    ) -> io::Result<Self> {
        let (discard, _) = sync_channel(0);
        let state: SharedState = Arc::new(Mutex::new(TerminalState::default()));
        let mut parser = vt100::Parser::new_with_callbacks(
            nonzero(size.rows),
            nonzero(size.cols),
            SCROLLBACK_LINES,
            Responder {
                writer: discard,
                state: Arc::clone(&state),
                options: TerminalOptions::default(),
            },
        );
        parser.process(screen);
        lock(&state).import(modes);
        let parser: SharedParser = Arc::new(Mutex::new(parser));
        let exit: SharedExit = Arc::new(Mutex::new(None));
        let waker: SharedWaker = Arc::new(Mutex::new(None));
        let standby: SharedStandby = Arc::new(AtomicBool::new(false));
        let reader = stream.try_clone()?;
        let shared = Relayed {
            parser: Arc::clone(&parser),
            exit: Arc::clone(&exit),
            waker: Arc::clone(&waker),
            standby: Arc::clone(&standby),
        };
        std::thread::Builder::new()
            .name("remote-reader".into())
            .spawn(move || relay(reader, &shared))?;
        let (outgoing, queue) = sync_channel(OUTGOING_QUEUE);
        let sending = Sending {
            stream: stream.try_clone()?,
            waker: Arc::clone(&waker),
            standby: Arc::clone(&standby),
        };
        std::thread::Builder::new()
            .name("remote-writer".into())
            .spawn(move || send_queued(&queue, sending))?;
        Ok(Self {
            parser,
            state,
            outgoing,
            stream,
            exit,
            waker,
            standby,
            pid,
        })
    }

    fn enqueue(&self, message: Outgoing) -> io::Result<()> {
        self.outgoing
            .try_send(message)
            .map_err(|error| match error {
                TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "the host is not taking input; dropped",
                ),
                TrySendError::Disconnected(_) => io::Error::from(io::ErrorKind::BrokenPipe),
            })
    }

    fn adjust_scrollback(&self, next_offset: impl FnOnce(usize) -> usize) {
        {
            let mut parser = lock(&self.parser);
            let screen = parser.screen_mut();
            let offset = next_offset(screen.scrollback());
            screen.set_scrollback(offset);
        }
        wake(&self.waker);
    }
}

struct Relayed {
    parser: SharedParser,
    exit: SharedExit,
    waker: SharedWaker,
    standby: SharedStandby,
}

struct Sending {
    stream: UnixStream,
    waker: SharedWaker,
    standby: SharedStandby,
}

fn send_queued(queue: &Receiver<Outgoing>, mut sending: Sending) {
    for message in queue {
        let sent = match message {
            Outgoing::Input(bytes) => write_bytes(&mut sending.stream, &bytes),
            Outgoing::Resize { rows, cols } => {
                write_message(&mut sending.stream, &Request::Resize { rows, cols })
            }
            Outgoing::Detach => {
                let _ = write_message(&mut sending.stream, &Request::Detach);
                break;
            }
        };
        if sent.is_err() {
            sending.standby.store(true, Ordering::Release);
            wake(&sending.waker);
            break;
        }
    }
    let _ = sending.stream.shutdown(std::net::Shutdown::Both);
}

fn relay(mut stream: UnixStream, shared: &Relayed) {
    loop {
        match read_frame(&mut stream) {
            Ok(Frame::Bytes(bytes)) => {
                lock(&shared.parser).process(&bytes);
                wake(&shared.waker);
            }
            Ok(frame) => match frame.decode() {
                Ok(Response::Exited(status)) => {
                    *lock(&shared.exit) = Some(status);
                    wake(&shared.waker);
                    return;
                }
                Ok(Response::Demoted) => {
                    shared.standby.store(true, Ordering::Release);
                    wake(&shared.waker);
                }
                _ => {}
            },
            Err(_) => break,
        }
    }
    let mut exit = lock(&shared.exit);
    if exit.is_none() {
        *exit = Some(ExitStatus {
            code: u32::MAX,
            signal: Some(HOST_LOST_SIGNAL.to_owned()),
        });
        drop(exit);
        wake(&shared.waker);
    }
}

impl TerminalSession for RemoteHost {
    fn snapshot(&self) -> Snapshot<'_> {
        Snapshot::new(lock(&self.parser))
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if self.is_standby() {
            return Ok(());
        }
        bytes
            .chunks(INPUT_CHUNK_BYTES)
            .try_for_each(|chunk| self.enqueue(Outgoing::Input(chunk.to_vec())))
    }

    fn keyboard_flags(&self) -> KeyboardFlags {
        let alternate_screen = self.snapshot().screen().alternate_screen();
        lock(&self.state).keyboard_flags(alternate_screen)
    }

    fn focus_reporting(&self) -> bool {
        lock(&self.state).focus_reporting()
    }

    fn take_events(&self) -> Vec<TerminalEvent> {
        lock(&self.state).take_events()
    }

    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        if self.is_standby() {
            return Ok(());
        }
        lock(&self.parser).set_size(nonzero(rows), nonzero(cols));
        let sent = self.enqueue(Outgoing::Resize { rows, cols });
        wake(&self.waker);
        sent
    }

    fn scroll_up(&self, lines: usize) {
        self.adjust_scrollback(|offset| offset.saturating_add(lines));
    }

    fn scroll_down(&self, lines: usize) {
        self.adjust_scrollback(|offset| offset.saturating_sub(lines));
    }

    fn reset_scroll(&self) {
        self.adjust_scrollback(|_| 0);
    }

    fn exit_status(&self) -> Option<ExitStatus> {
        lock(&self.exit).clone()
    }

    fn process_id(&self) -> Option<u32> {
        self.pid
    }

    fn is_standby(&self) -> bool {
        self.standby.load(Ordering::Acquire)
    }

    fn set_waker(&self, waker: Waker) {
        *lock(&self.waker) = Some(waker);
    }
}

impl Drop for RemoteHost {
    fn drop(&mut self) {
        if self.enqueue(Outgoing::Detach).is_err() {
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::{Duration, Instant};

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not met in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn remote() -> (RemoteHost, UnixStream) {
        let (client, server) = UnixStream::pair().unwrap();
        let host = RemoteHost::new(
            client,
            Size { rows: 5, cols: 30 },
            None,
            b"",
            ModeSnapshot::default(),
        )
        .unwrap();
        (host, server)
    }

    const QUERIES: &[u8] = b"\x1b[6n\x1b[5n\x1b[c\x1b[?u\x1b[?2004$p\x1b[18t\x1b]10;?\x07\x1b[>q";

    #[test]
    fn terminal_queries_in_the_relayed_bytes_are_never_answered_to_the_child() {
        let (host, mut server) = remote();
        write_bytes(&mut server, QUERIES).unwrap();
        write_bytes(&mut server, b"after").unwrap();
        wait_until(|| host.snapshot().text().contains("after"));
        host.write(b"x").unwrap();
        let mut received = [0u8; 64];
        let read = server.read(&mut received).unwrap();
        assert_eq!(&received[..read], [&[0, 0, 0, 2, 1][..], b"x"].concat());
    }

    #[test]
    fn input_for_a_host_that_stops_reading_never_blocks_the_caller_and_is_dropped_when_the_queue_is_full()
     {
        let (host, _server) = remote();
        let paste = vec![b'a'; 1 << 20];
        let started = Instant::now();
        let results: Vec<_> = (0..32).map(|_| host.write(&paste)).collect();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(results[0].is_ok());
        assert!(results.iter().any(
            |result| matches!(result, Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        ));
    }

    #[test]
    fn losing_the_host_marks_the_session_exited_without_a_clean_exit_code() {
        let (host, server) = remote();
        drop(server);
        wait_until(|| host.exit_status().is_some());
        let status = host.exit_status().unwrap();
        assert!(!status.success());
        assert_eq!(status.signal.as_deref(), Some(HOST_LOST_SIGNAL));
    }
}
