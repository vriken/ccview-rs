use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::durable::{Durable, Persisted};
use super::host::{Output, PtyHost, Size};
use super::protocol::{Frame, Request, Response, SessionInfo, read_frame, write_bytes, write_message};
use super::restore;
use super::runtime_dir::{RuntimeDir, effective_uid};
use super::spec::CommandSpec;

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
pub const IDLE_EXIT: Duration = Duration::from_secs(60);
const ACCEPT_POLL_MS: i32 = 200;
const OUTPUT_POLL: Duration = Duration::from_millis(200);
const SOCKET_MODE: u32 = 0o600;
const SOCKET_UMASK: libc::mode_t = 0o177;
pub(super) const HOST_SET_ENV: [&str; 2] = ["TERM", "COLORTERM"];

#[derive(Debug, Clone, Copy)]
pub struct DaemonConfig {
    pub idle_exit: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self { idle_exit: IDLE_EXIT }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    AlreadyRunning,
    Idle,
    Shutdown,
}

struct Entry {
    id: u64,
    title: String,
    harness: String,
    cwd: Option<String>,
    session_id: Option<String>,
    launched_at_ms: u64,
    host: Arc<PtyHost>,
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
}

#[derive(Default)]
struct Shared {
    registry: Mutex<Registry>,
    clients: AtomicUsize,
    durable: Option<Mutex<Durable>>,
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
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(error)
    }
}

fn bind(dir: &RuntimeDir) -> io::Result<UnixListener> {
    let path = dir.socket();
    match fs::remove_file(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let previous = unsafe { libc::umask(SOCKET_UMASK) };
    let listener = UnixListener::bind(&path);
    unsafe { libc::umask(previous) };
    let listener = listener?;
    fs::set_permissions(&path, fs::Permissions::from_mode(SOCKET_MODE))?;
    Ok(listener)
}

#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &mut length,
        )
    };
    if result == 0 { Ok(credentials.uid) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(target_os = "linux"))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0 {
        Ok(uid)
    } else {
        Err(io::Error::last_os_error())
    }
}

fn accept_ready(listener: &UnixListener) -> bool {
    let mut poll = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ready = unsafe { libc::poll(&mut poll, 1, ACCEPT_POLL_MS) };
    ready > 0 && poll.revents & libc::POLLIN != 0
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

extern "C" fn on_shutdown_signal(_: libc::c_int) {
    SHUTTING_DOWN.store(true, Ordering::Release);
}

pub fn install_shutdown_handlers() {
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_shutdown_signal as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

pub fn run(dir: &RuntimeDir, config: &DaemonConfig) -> io::Result<Outcome> {
    run_durable(dir, config, None)
}

pub fn run_durable(dir: &RuntimeDir, config: &DaemonConfig, durable: Option<Durable>) -> io::Result<Outcome> {
    dir.ensure()?;
    let Some(_lock) = try_lock(dir)? else {
        return Ok(Outcome::AlreadyRunning);
    };
    fs::write(dir.pid_file(), std::process::id().to_string())?;
    let listener = bind(dir)?;
    eprintln!("ccview-rs host {} listening on {}", std::process::id(), dir.socket().display());
    let shared = Arc::new(Shared { durable: durable.map(Mutex::new), ..Shared::default() });
    let mut idle_since: Option<Instant> = None;
    loop {
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            eprintln!("ccview-rs host shutting down: keeping the durable session list");
            if let Some(durable) = &shared.durable {
                super::host::lock(durable).finalize_shutdown();
            }
            let _ = fs::remove_file(dir.socket());
            let _ = fs::remove_file(dir.pid_file());
            return Ok(Outcome::Shutdown);
        }
        maintain(&shared);
        if accept_ready(&listener) {
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
            || super::host::lock(&shared.registry).live_count() > 0;
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
    let registry = super::host::lock(&shared.registry);
    let mut durable = super::host::lock(durable);
    if !durable.due() {
        return;
    }
    let sessions_dir = crate::registry::claude_sessions_dir();
    for entry in &registry.entries {
        durable.observe(entry.id, entry.host.exit_status().is_some(), || {
            crate::registry::session_id_for_pid(sessions_dir.as_deref()?, entry.host.process_id()?)
        });
    }
}

fn restore_sessions(shared: &Shared) -> usize {
    let Some(durable) = &shared.durable else {
        return 0;
    };
    let (stale, running) = {
        let durable = super::host::lock(durable);
        (durable.stale(), durable.running_session_ids())
    };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let disable_agent_view = crate::ui_settings::UiSettingsStore::load_default().settings().disable_agent_view;
    let plans = restore::plan(stale, &running, &home, std::env::var("PATH").ok().as_deref(), disable_agent_view);
    let mut started = 0;
    for plan in plans {
        let persisted = plan.persisted;
        let response = spawn(
            shared,
            plan.spec,
            super::spec::TerminalOptions::default(),
            restore::RESTORE_SIZE,
            persisted.title,
            persisted.harness,
            persisted.session_id,
        );
        match response {
            Response::Spawned(info) => {
                started += 1;
                eprintln!("restore: started session {} ({})", info.id, info.harness);
            }
            other => eprintln!("restore: could not start a session: {other:?}"),
        }
    }
    super::host::lock(durable).drop_stale();
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
            Request::Spawn { spec, options, size, title, harness, session_id } => {
                spawn(shared, spec, options, size, title, harness, session_id)
            }
            Request::List => Response::Sessions(
                super::host::lock(&shared.registry)
                    .entries
                    .iter()
                    .map(Entry::info)
                    .collect(),
            ),
            Request::Dismiss { id } => dismiss(shared, id),
            Request::Restore => Response::Restored { started: restore_sessions(shared) },
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
    options: super::spec::TerminalOptions,
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
    let mut registry = super::host::lock(&shared.registry);
    registry.next_id += 1;
    let entry = Entry {
        id: registry.next_id,
        title,
        harness,
        cwd: spec.cwd.as_ref().map(|cwd| cwd.display().to_string()),
        session_id,
        launched_at_ms: now_ms(),
        host: Arc::new(host),
    };
    let info = entry.info();
    if let (Some(durable), Some(persisted)) = (&shared.durable, persisted) {
        super::host::lock(durable).add(entry.id, persisted);
    }
    eprintln!("spawned session {} ({}) pid {:?}", info.id, info.harness, info.pid);
    registry.entries.push(entry);
    Response::Spawned(info)
}

fn dismiss(shared: &Shared, id: u64) -> Response {
    let removed = {
        let mut registry = super::host::lock(&shared.registry);
        let Some(position) = registry.entries.iter().position(|entry| entry.id == id) else {
            return Response::Done;
        };
        if registry.entries[position].host.exit_status().is_none() {
            return Response::Error("session is still running".to_owned());
        }
        registry.entries.remove(position)
    };
    if let Some(durable) = &shared.durable {
        super::host::lock(durable).remove(removed.id);
    }
    drop(removed);
    Response::Done
}

fn attach(mut stream: UnixStream, shared: &Shared, id: u64, size: Option<Size>) -> io::Result<()> {
    let found = {
        let registry = super::host::lock(&shared.registry);
        registry.find(id).map(|entry| (Arc::clone(&entry.host), entry.info()))
    };
    let Some((host, info)) = found else {
        return write_message(&mut stream, &Response::Error(format!("no session {id}")));
    };
    let attachment = host.attach(size)?;
    let info = SessionInfo { size: attachment.size, exit: attachment.exit.clone(), ..info };
    write_message(&mut stream, &Response::Attached { info, modes: attachment.modes })?;
    write_bytes(&mut stream, &attachment.screen)?;
    if let Some(exit) = attachment.exit {
        return write_message(&mut stream, &Response::Exited(exit));
    }
    let closed = Arc::new(AtomicBool::new(false));
    let pump = {
        let stream = stream.try_clone()?;
        let closed = Arc::clone(&closed);
        std::thread::Builder::new()
            .name("host-output".into())
            .spawn(move || pump_output(stream, &attachment.output, &closed))?
    };
    forward_input(&mut stream, host.as_ref());
    closed.store(true, Ordering::Release);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = pump.join();
    Ok(())
}

fn forward_input(stream: &mut UnixStream, host: &PtyHost) {
    while let Ok(frame) = read_frame(stream) {
        match frame {
            Frame::Bytes(bytes) => {
                let _ = host.write(&bytes);
            }
            Frame::Message(_) => match frame.decode::<Request>() {
                Ok(Request::Resize { rows, cols }) => {
                    let _ = host.resize(rows, cols);
                }
                Ok(Request::Detach) | Err(_) => return,
                Ok(_) => {}
            },
        }
    }
}

fn pump_output(mut stream: UnixStream, output: &Receiver<Output>, closed: &AtomicBool) {
    loop {
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
