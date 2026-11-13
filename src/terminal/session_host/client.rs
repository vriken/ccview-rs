use std::collections::HashSet;
use std::fmt;
use std::io;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::daemon::HOST_SET_ENV;
use super::protocol::{Frame, Request, Response, SessionInfo, read_frame, write_message};
use super::remote::RemoteHost;
use super::runtime_dir::RuntimeDir;
use crate::sys::command::CommandSpec;
use crate::terminal::host::Size;
use crate::terminal::spec::TerminalOptions;

const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(20);
const LOCKED_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct HostClient {
    dir: RuntimeDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUnreachable {
    pub holder: Option<u32>,
}

impl fmt::Display for HostUnreachable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let holder = self.holder.map_or("lock held".to_owned(), |pid| {
            format!("lock held by pid {pid}")
        });
        write!(
            formatter,
            "session host unreachable ({holder}, no socket): sessions in it cannot be attached; see AGENTS.md \"Recovering an unreachable host\""
        )
    }
}

impl std::error::Error for HostUnreachable {}

pub struct SpawnRequest {
    pub spec: CommandSpec,
    pub options: TerminalOptions,
    pub size: Size,
    pub title: String,
    pub harness: String,
    pub session_id: Option<String>,
}

fn unexpected(response: Response) -> io::Error {
    match response {
        Response::Error(message) => io::Error::other(message),
        other => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected reply {other:?}"),
        ),
    }
}

pub fn with_client_environment(mut spec: CommandSpec) -> CommandSpec {
    let overridden: HashSet<String> = spec
        .env
        .iter()
        .map(|(key, _)| key.clone())
        .chain(spec.env_remove.iter().cloned())
        .collect();
    let mut env: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(key, _)| !HOST_SET_ENV.contains(&key.as_str()) && !overridden.contains(key))
        .collect();
    env.append(&mut spec.env);
    spec.env = env;
    spec
}

fn clear_timeout_unless_peer_closed(stream: &UnixStream) {
    let _ = stream.set_read_timeout(None);
}

fn start_daemon(dir: &RuntimeDir) -> io::Result<()> {
    dir.ensure()?;
    let log = dir.open_private(&dir.log())?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("host")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    crate::sys::process::start_in_new_session(&mut command);
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn lock_holder(dir: &RuntimeDir) -> Option<u32> {
    [dir.pid_file(), dir.lock()]
        .iter()
        .filter_map(|path| {
            std::fs::read_to_string(path)
                .ok()?
                .trim()
                .parse::<u32>()
                .ok()
        })
        .find(|pid| crate::sys::process::pid_alive(*pid))
}

impl HostClient {
    pub fn new(dir: RuntimeDir) -> Self {
        Self { dir }
    }

    fn connect(&self) -> io::Result<UnixStream> {
        let stream = UnixStream::connect(self.dir.socket())?;
        stream.set_read_timeout(Some(CALL_TIMEOUT))?;
        stream.set_write_timeout(Some(CALL_TIMEOUT))?;
        Ok(stream)
    }

    pub fn unreachable_in(error: &io::Error) -> Option<&HostUnreachable> {
        error.get_ref()?.downcast_ref()
    }

    pub fn unreachable(&self) -> Option<HostUnreachable> {
        (self.connect().is_err() && self.lock_held()).then(|| self.unreachable_host())
    }

    fn lock_held(&self) -> bool {
        crate::sys::lock::is_held(&self.dir.lock()).unwrap_or(false)
    }

    fn unreachable_host(&self) -> HostUnreachable {
        HostUnreachable {
            holder: lock_holder(&self.dir),
        }
    }

    fn connect_or_start(&self) -> io::Result<UnixStream> {
        if let Ok(stream) = self.connect() {
            return Ok(stream);
        }
        if !self.lock_held() {
            start_daemon(&self.dir)?;
        }
        let deadline = Instant::now() + START_TIMEOUT;
        let mut held_since = None;
        loop {
            let error = match self.connect() {
                Ok(stream) => return Ok(stream),
                Err(error) => error,
            };
            if self.lock_held() {
                let since = *held_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= LOCKED_GRACE {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        self.unreachable_host(),
                    ));
                }
            }
            if Instant::now() >= deadline {
                return Err(error);
            }
            std::thread::sleep(START_POLL);
        }
    }

    fn call(stream: &mut UnixStream, request: &Request) -> io::Result<Response> {
        write_message(stream, request)?;
        read_frame(stream)?.decode()
    }

    pub fn list(&self) -> io::Result<Vec<SessionInfo>> {
        let Ok(mut stream) = self.connect() else {
            return Ok(Vec::new());
        };
        match Self::call(&mut stream, &Request::List)? {
            Response::Sessions(sessions) => Ok(sessions),
            other => Err(unexpected(other)),
        }
    }

    pub fn spawn(&self, request: SpawnRequest) -> io::Result<SessionInfo> {
        let mut stream = self.connect_or_start()?;
        let request = Request::Spawn {
            spec: Box::new(with_client_environment(request.spec)),
            options: request.options,
            size: request.size,
            title: request.title,
            harness: request.harness,
            session_id: request.session_id,
        };
        match Self::call(&mut stream, &request)? {
            Response::Spawned(info) => Ok(info),
            other => Err(unexpected(other)),
        }
    }

    pub fn restore(&self) -> io::Result<usize> {
        let mut stream = self.connect_or_start()?;
        match Self::call(&mut stream, &Request::Restore)? {
            Response::Restored { started } => Ok(started),
            other => Err(unexpected(other)),
        }
    }

    pub fn dismiss(&self, id: u64) -> io::Result<()> {
        let Ok(mut stream) = self.connect() else {
            return Ok(());
        };
        match Self::call(&mut stream, &Request::Dismiss { id })? {
            Response::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn stop(&self, id: u64) -> io::Result<()> {
        let mut stream = self.connect()?;
        match Self::call(&mut stream, &Request::Stop { id })? {
            Response::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub fn attach(&self, id: u64, size: Option<Size>) -> io::Result<(SessionInfo, RemoteHost)> {
        let mut stream = self.connect()?;
        let request = Request::Attach { id, size };
        let (info, modes) = match Self::call(&mut stream, &request)? {
            Response::Attached { info, modes } => (info, modes),
            other => return Err(unexpected(other)),
        };
        let Frame::Bytes(screen) = read_frame(&mut stream)? else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing screen snapshot",
            ));
        };
        clear_timeout_unless_peer_closed(&stream);
        let host = RemoteHost::new(stream, info.size, info.pid, &screen, modes)?;
        Ok((info, host))
    }
}
