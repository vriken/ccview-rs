use super::App;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
};
use std::{
    io::{self, Write},
    sync::mpsc::{Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

const IDLE_POLL: Duration = Duration::from_millis(50);
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub enum Message {
    Input(Event),
    InputClosed,
    Output(u64),
    Updated,
}

pub fn spawn_input_reader(sender: Sender<Message>) {
    thread::spawn(move || {
        while let Ok(event) = event::read() {
            if sender.send(Message::Input(event)).is_err() {
                return;
            }
        }
        let _ = sender.send(Message::InputClosed);
    });
}

pub fn set_mouse_capture(enabled: bool) -> io::Result<()> {
    if enabled {
        execute!(io::stdout(), EnableMouseCapture)
    } else {
        execute!(io::stdout(), DisableMouseCapture)
    }
}

pub fn write_outer(bytes: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}

pub fn event_loop(
    app: &mut App,
    messages: &Receiver<Message>,
    mut draw: impl FnMut(&mut App) -> io::Result<()>,
    mut apply_mouse_capture: impl FnMut(bool) -> io::Result<()>,
    mut write_outer: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut dirty = true;
    let mut urgent = true;
    let mut last_draw: Option<Instant> = None;
    let mut mouse_captured = false;
    while !app.should_quit() {
        let since_draw = last_draw.map(|at| at.elapsed());
        let throttled = since_draw.is_some_and(|elapsed| elapsed < MIN_FRAME_INTERVAL);
        if dirty && (urgent || !throttled) {
            draw(app)?;
            last_draw = Some(Instant::now());
            dirty = false;
            urgent = false;
        }
        let wait = match (dirty, since_draw) {
            (true, Some(elapsed)) => MIN_FRAME_INTERVAL.saturating_sub(elapsed),
            _ => IDLE_POLL,
        };
        match messages.recv_timeout(wait) {
            Ok(message) => {
                handle(app, message, &mut dirty, &mut urgent);
                while let Ok(message) = messages.try_recv() {
                    handle(app, message, &mut dirty, &mut urgent);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        dirty |= app.tick();
        let outer_bytes = app.take_outer_output();
        if !outer_bytes.is_empty() {
            write_outer(&outer_bytes)?;
        }
        let want_capture = app.wants_mouse_capture();
        if want_capture != mouse_captured {
            apply_mouse_capture(want_capture)?;
            mouse_captured = want_capture;
        }
    }
    if mouse_captured {
        apply_mouse_capture(false)?;
    }
    Ok(())
}

fn handle(app: &mut App, message: Message, dirty: &mut bool, urgent: &mut bool) {
    match message {
        Message::Input(event) => {
            if app.handle_event(event) {
                *dirty = true;
                *urgent = true;
            }
        }
        Message::InputClosed => app.quit(),
        Message::Output(id) => *dirty |= app.output_ready(id),
        Message::Updated => {}
    }
}
