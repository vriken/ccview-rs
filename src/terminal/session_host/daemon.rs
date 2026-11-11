use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::durable::{Durable, Persisted};
use super::lease::Lease;
use super::protocol::{
    Frame, Request, Response, SessionInfo, read_frame, write_bytes, write_message,
};
use super::restore;
use super::runtime_dir::{RuntimeDir, effective_uid};
use crate::terminal::host::{Output, PtyHost, Size};
use crate::terminal::spec::CommandSpec;

pub const IDLE_EXIT: Duration = Duration::from_secs(60);
const ACCEPT_POLL_MS: i32 = 200;
const OUTPUT_POLL: Duration = Duration::from_millis(200);
const SOCKET_MODE: u32 = 0o600;
pub(super) const HOST_SET_ENV: [&str; 2] = ["TERM", "COLORTERM"];

#[derive(Debug, Clone, Copy)]
pub struct DaemonConfig {
    pub idle_exit: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_exit: IDLE_EXIT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    AlreadyRunning,
    Idle,
    Shutdown,
}

#[derive(Clone)]
struct Entry {
    id: u64,
    title: String,
    harness: String,
    cwd: Option<String>,
    session_id: Option<String>,
    launched_at_ms: u64,
    host: Arc<PtyHost>,
    lease: Arc<Lease>,
}

impl Entry {
    fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id,
            title: self.title.clone(),
            harness: self.harness.clone(),
            cwd: self.cwd.clone(),
            session_id: self.session_id.clone(),
            pid: self.host.process_id(),
            launched_at_ms: self.launched_at_ms,
            exit: self.host.exit_status(),
            size: self.host.snapshot().size(),
        }
    }
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    entries: Vec<Entry>,
}

impl Registry {
    fn live_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.host.exit_status().is_none())
            .count()
    }

    fn find(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    fn snapshot(&self) -> Vec<Entry> {
        self.entries.clone()
    }
}

#[derive(Default)]
struct Shared {
    registry: Mutex<Registry>,
    clients: AtomicUsize,
    durable: Option<Mutex<Durable>>,
    restoring: Mutex<()>,
}

struct ClientGuard(Arc<Shared>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn try_lock(dir: &RuntimeDir) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.lock())?;
    if crate::sys::lock::try_lock_exclusive(&file)? {
        Ok(Some(file))
    } else {
        Ok(None)
    }
}

fn bind(dir: &RuntimeDir) -> io::Result<UnixListener> {
    let path = dir.socket();
    match fs::remove_file(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_MODE))?;
    Ok(listener)
}

pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    crate::sys::socket::peer_uid(stream)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub fn run(dir: &RuntimeDir, config: &DaemonConfig) -> io::Result<Outcome> {
    run_durable(dir, config, None)
}

pub fn run_durable(
    dir: &RuntimeDir,
    config: &DaemonConfig,
    durable: Option<Durable>,
) -> io::Result<Outcome> {
    dir.ensure()?;
    let Some(_lock) = try_lock(dir)? else {
        return Ok(Outcome::AlreadyRunning);
    };
    fs::write(dir.pid_file(), std::process::id().to_string())?;
    let listener = bind(dir)?;
    eprintln!(
        "ccview-rs host {} listening on {}",
        std::process::id(),
        dir.socket().display()
    );
    let shared = Arc::new(Shared {
        durable: durable.map(Mutex::new),
        ..Shared::default()
    });
    let mut idle_since: Option<Instant> = None;
    loop {
        if crate::sys::signal::shutdown_requested() {
            eprintln!("ccview-rs host shutting down: keeping the durable session list");
            if let Some(durable) = &shared.durable {
                crate::terminal::host::lock(durable).finalize_shutdown();
            }
            let _ = fs::remove_file(dir.socket());
            let _ = fs::remove_file(dir.pid_file());
            return Ok(Outcome::Shutdown);
        }
        maintain(&shared);
        if crate::sys::socket::connection_ready(&listener, ACCEPT_POLL_MS) {
            match listener.accept() {
                Ok((stream, _)) => {
                    shared.clients.fetch_add(1, Ordering::AcqRel);
                    let guard = ClientGuard(Arc::clone(&shared));
                    let shared = Arc::clone(&shared);
                    std::thread::Builder::new()
                        .name("host-client".into())
                        .spawn(move || {
                            let _ = serve(stream, &shared, guard);
                        })?;
                }
                Err(error) => eprintln!("accept failed: {error}"),
            }
        }
        let busy = shared.clients.load(Ordering::Acquire) > 0
            || crate::terminal::host::lock(&shared.registry).live_count() > 0;
        if busy {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= config.idle_exit {
            break;
        }
    }
    eprintln!("ccview-rs host exiting: no live sessions and no clients");
    let _ = fs::remove_file(dir.socket());
    let _ = fs::remove_file(dir.pid_file());
    Ok(Outcome::Idle)
}

fn maintain(shared: &Shared) {
    let Some(durable) = &shared.durable else {
        return;
    };
    let entries = crate::terminal::host::lock(&shared.registry).snapshot();
    let mut durable = crate::terminal::host::lock(durable);
    if !durable.due() {
        return;
    }
    let sessions_dir = crate::sources::claude::registry::claude_sessions_dir();
    for entry in &entries {
        durable.observe(entry.id, entry.host.exit_status().is_some(), || {
            crate::sources::claude::registry::session_id_for_pid(
                sessions_dir.as_deref()?,
                entry.host.process_id()?,
            )
        });
    }
}

fn restore_sessions(shared: &Shared) -> usize {
    let Some(durable) = &shared.durable else {
        return 0;
    };
    let _serialized = crate::terminal::host::lock(&shared.restoring);
    let (stale, running) = {
        let durable = crate::terminal::host::lock(durable);
        (durable.stale(), durable.running_session_ids())
    };
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let disable_agent_view = crate::store::settings::UiSettingsStore::load_default()
        .settings()
        .disable_agent_view;
    let plans = restore::plan(
        stale,
        &running,
        &home,
        std::env::var("PATH").ok().as_deref(),
        disable_agent_view,
    );
    let mut started = 0;
    let mut settled = running;
    for plan in plans {
        let persisted = plan.persisted;
        let session_id = persisted.session_id.clone();
        let response = spawn(
            shared,
            plan.spec,
            crate::terminal::spec::TerminalOptions::default(),
            restore::RESTORE_SIZE,
            persisted.title,
            persisted.harness,
            persisted.session_id,
        );
        match response {
            Response::Spawned(info) => {
                started += 1;
                settled.extend(session_id);
                eprintln!("restore: started session {} ({})", info.id, info.harness);
            }
            other => eprintln!("restore: could not start a session: {other:?}"),
        }
    }
    crate::terminal::host::lock(durable).drop_restored(&settled);
    started
}

fn isolate_environment(spec: &mut CommandSpec) {
    for (key, _) in std::env::vars_os() {
        if let Some(key) = key.to_str()
            && !HOST_SET_ENV.contains(&key)
        {
            spec.env_remove.push(key.to_owned());
        }
    }
}

fn serve(mut stream: UnixStream, shared: &Arc<Shared>, _guard: ClientGuard) -> io::Result<()> {
    if peer_uid(&stream)? != effective_uid() {
        return Ok(());
    }
    loop {
        let request: Request = match read_frame(&mut stream).and_then(|frame| frame.decode()) {
            Ok(request) => request,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => {
                let _ = write_message(&mut stream, &Response::Error(error.to_string()));
                return Err(error);
            }
        };
        let response = match request {
            Request::Attach { id, size } => return attach(stream, shared, id, size),
            Request::Spawn {
                spec,
                options,
                size,
                title,
                harness,
                session_id,
            } => spawn(shared, spec, options, size, title, harness, session_id),
            Request::List => {
                let entries = crate::terminal::host::lock(&shared.registry).snapshot();
                Response::Sessions(entries.iter().map(Entry::info).collect())
            }
            Request::Dismiss { id } => dismiss(shared, id),
            Request::Stop { id } => stop(shared, id),
            Request::Restore => Response::Restored {
                started: restore_sessions(shared),
            },
            Request::Resize { .. } | Request::Detach => {
                Response::Error("not attached to a session".to_owned())
            }
        };
        write_message(&mut stream, &response)?;
    }
}

fn spawn(
    shared: &Shared,
    mut spec: CommandSpec,
    options: crate::terminal::spec::TerminalOptions,
    size: Size,
    title: String,
    harness: String,
    session_id: Option<String>,
) -> Response {
    let persisted = Persisted::describe(&harness, &spec, session_id.clone(), &title, now_ms());
    isolate_environment(&mut spec);
    let host = match PtyHost::spawn_with_options(&spec, size, options) {
        Ok(host) => host,
        Err(error) => return Response::Error(error.to_string()),
    };
    let mut registry = crate::terminal::host::lock(&shared.registry);
    registry.next_id += 1;
    let entry = Entry {
        id: registry.next_id,
        title,
        harness,
        cwd: spec.cwd.as_ref().map(|cwd| cwd.display().to_string()),
        session_id,
        launched_at_ms: now_ms(),
        host: Arc::new(host),
        lease: Arc::default(),
    };
    let info = entry.info();
    if let (Some(durable), Some(persisted)) = (&shared.durable, persisted) {
        crate::terminal::host::lock(durable).add(entry.id, persisted);
    }
    eprintln!(
        "spawned session {} ({}) pid {:?}",
        info.id, info.harness, info.pid
    );
    registry.entries.push(entry);
    Response::Spawned(info)
}

fn dismiss(shared: &Shared, id: u64) -> Response {
    let removed = {
        let mut registry = crate::terminal::host::lock(&shared.registry);
        let Some(position) = registry.entries.iter().position(|entry| entry.id == id) else {
            return Response::Done;
        };
        if registry.entries[position].host.exit_status().is_none() {
            return Response::Error("session is still running".to_owned());
        }
        registry.entries.remove(position)
    };
    if let Some(durable) = &shared.durable {
        crate::terminal::host::lock(durable).remove(removed.id);
    }
    drop(removed);
    Response::Done
}

fn stop(shared: &Shared, id: u64) -> Response {
    let found = crate::terminal::host::lock(&shared.registry)
        .find(id)
        .cloned();
    let Some(entry) = found else {
        return Response::Error(format!("no session {id}"));
    };
    if entry.host.exit_status().is_some() {
        return Response::Done;
    }
    match entry.host.terminate() {
        Ok(()) => Response::Done,
        Err(error) => Response::Error(error.to_string()),
    }
}

fn attach(mut stream: UnixStream, shared: &Shared, id: u64, size: Option<Size>) -> io::Result<()> {
    let found = {
        let registry = crate::terminal::host::lock(&shared.registry);
        registry.find(id).cloned()
    };
    let Some(entry) = found else {
        return write_message(&mut stream, &Response::Error(format!("no session {id}")));
    };
    let info = entry.info();
    let Entry { host, lease, .. } = entry;
    let token = lease.take();
    let attachment = match host.attach(size) {
        Ok(attachment) => attachment,
        Err(error) => {
            lease.release(token);
            return Err(error);
        }
    };
    let info = SessionInfo {
        size: attachment.size,
        exit: attachment.exit.clone(),
        ..info
    };
    write_message(
        &mut stream,
        &Response::Attached {
            info,
            modes: attachment.modes,
        },
    )?;
    write_bytes(&mut stream, &attachment.screen)?;
    if let Some(exit) = attachment.exit {
        lease.release(token);
        return write_message(&mut stream, &Response::Exited(exit));
    }
    let closed = Arc::new(AtomicBool::new(false));
    let held = Held {
        lease: Arc::clone(&lease),
        token,
    };
    let pump = {
        let stream = stream.try_clone()?;
        let closed = Arc::clone(&closed);
        let held = held.clone();
        std::thread::Builder::new()
            .name("host-output".into())
            .spawn(move || pump_output(stream, &attachment.output, &closed, &held))?
    };
    forward_input(&mut stream, host.as_ref(), &held);
    lease.release(token);
    closed.store(true, Ordering::Release);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = pump.join();
    Ok(())
}

#[derive(Clone)]
struct Held {
    lease: Arc<Lease>,
    token: u64,
}

impl Held {
    fn is_active(&self) -> bool {
        self.lease.holds(self.token)
    }
}

fn forward_input(stream: &mut UnixStream, host: &PtyHost, held: &Held) {
    while let Ok(frame) = read_frame(stream) {
        match frame {
            Frame::Bytes(bytes) => {
                if held.is_active() {
                    let _ = host.write(&bytes);
                }
            }
            Frame::Message(_) => match frame.decode::<Request>() {
                Ok(Request::Resize { rows, cols }) => {
                    if held.is_active() {
                        let _ = host.resize(rows, cols);
                    }
                }
                Ok(Request::Detach) | Err(_) => return,
                Ok(_) => {}
            },
        }
    }
}

fn pump_output(
    mut stream: UnixStream,
    output: &Receiver<Output>,
    closed: &AtomicBool,
    held: &Held,
) {
    let mut demotion_sent = false;
    loop {
        if !demotion_sent && !held.is_active() {
            demotion_sent = true;
            if write_message(&mut stream, &Response::Demoted).is_err() {
                break;
            }
        }
        let written = match output.recv_timeout(OUTPUT_POLL) {
            Ok(Output::Data(bytes)) => write_bytes(&mut stream, &bytes),
            Ok(Output::Exit(status)) => {
                let _ = write_message(&mut stream, &Response::Exited(status));
                break;
            }
            Err(RecvTimeoutError::Timeout) if closed.load(Ordering::Acquire) => break,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if written.is_err() {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}
