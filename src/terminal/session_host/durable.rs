use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::store::write_atomically;
use crate::sys::command::CommandSpec;

const STATE_HOME_ENV: &str = "XDG_STATE_HOME";
const SESSIONS_FILE: &str = "sessions.json";
const HOST_LOG_FILE: &str = "host.log";
pub const EXIT_GRACE: Duration = Duration::from_secs(1);
pub const MAINTAIN_INTERVAL: Duration = Duration::from_millis(500);

pub fn state_dir() -> PathBuf {
    let home = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    home(STATE_HOME_ENV)
        .or_else(|| home("HOME").map(|home| home.join(".local").join("state")))
        .unwrap_or_default()
        .join("ccview-rs")
}

pub fn sessions_file() -> PathBuf {
    state_dir().join(SESSIONS_FILE)
}

pub fn host_log_file() -> PathBuf {
    state_dir().join(HOST_LOG_FILE)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Persisted {
    pub harness: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub program: Option<String>,
    pub launch: String,
    pub title: String,
    pub started_ms: u64,
}

impl Persisted {
    pub fn describe(
        harness: &str,
        spec: &CommandSpec,
        session_id: Option<String>,
        title: &str,
        started_ms: u64,
    ) -> Option<Self> {
        let cwd = spec.cwd.as_ref()?;
        let path_var = spec
            .env
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var("PATH").ok());
        Some(Self {
            harness: harness.to_owned(),
            cwd: cwd.display().to_string(),
            session_id,
            program: resolve_program(&spec.program, cwd, path_var.as_deref())
                .map(|path| path.display().to_string()),
            launch: launch_kind(&spec.args).to_owned(),
            title: title.to_owned(),
            started_ms,
        })
    }
}

pub fn launch_kind(args: &[String]) -> &'static str {
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    match args.first().map(String::as_str) {
        Some("attach") => "attach",
        Some("fork") => "fork",
        Some("resume") if has("--last") => "continue",
        _ if has("--fork-session") || has("--fork") => "fork",
        _ if has("--continue") => "continue",
        Some("resume") | Some("--resume") | Some("--session") => "resume",
        _ => "new",
    }
}

pub fn resolve_program(program: &str, cwd: &Path, path_var: Option<&str>) -> Option<PathBuf> {
    if program.contains('/') {
        let path = cwd.join(program);
        return path.is_file().then_some(path);
    }
    std::env::split_paths(path_var?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .map(|candidate| {
            if candidate.is_absolute() {
                candidate
            } else {
                cwd.join(candidate)
            }
        })
}

#[derive(Serialize, Deserialize, Default)]
struct StateFile {
    #[serde(default)]
    sessions: Vec<Persisted>,
}

struct Record {
    owner: Option<u64>,
    data: Persisted,
    exit_seen: Option<Instant>,
}

pub struct Durable {
    path: PathBuf,
    records: Vec<Record>,
    exit_grace: Duration,
    last_maintained: Option<Instant>,
}

fn read_state_file(path: &Path) -> StateFile {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return StateFile::default(),
        Err(error) => {
            eprintln!("could not read {}: {error}", path.display());
            keep_unreadable(path);
            return StateFile::default();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        eprintln!("could not parse {}: {error}", path.display());
        keep_unreadable(path);
        StateFile::default()
    })
}

fn keep_unreadable(path: &Path) {
    let mut kept = path.as_os_str().to_owned();
    kept.push(".bad");
    if let Err(error) = std::fs::rename(path, &kept) {
        eprintln!("could not keep {} aside: {error}", path.display());
    }
}

impl Durable {
    pub fn open(path: PathBuf) -> Self {
        let file = read_state_file(&path);
        let records = file
            .sessions
            .into_iter()
            .map(|data| Record {
                owner: None,
                data,
                exit_seen: None,
            })
            .collect();
        Self {
            path,
            records,
            exit_grace: EXIT_GRACE,
            last_maintained: None,
        }
    }

    pub fn with_exit_grace(mut self, grace: Duration) -> Self {
        self.exit_grace = grace;
        self
    }

    fn save(&self) {
        let file = StateFile {
            sessions: self
                .records
                .iter()
                .map(|record| record.data.clone())
                .collect(),
        };
        let written = serde_json::to_vec_pretty(&file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_atomically(&self.path, &bytes));
        if let Err(error) = written {
            eprintln!("could not write {}: {error}", self.path.display());
        }
    }

    pub fn add(&mut self, owner: u64, data: Persisted) {
        self.records.push(Record {
            owner: Some(owner),
            data,
            exit_seen: None,
        });
        self.save();
    }

    pub fn remove(&mut self, owner: u64) {
        let before = self.records.len();
        self.records.retain(|record| record.owner != Some(owner));
        if self.records.len() != before {
            self.save();
        }
    }

    pub fn bind(&mut self, owner: u64, session_id: String) {
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.owner == Some(owner))
        else {
            return;
        };
        if record.data.session_id.as_deref() != Some(session_id.as_str()) {
            record.data.session_id = Some(session_id);
            self.save();
        }
    }

    pub fn running_session_ids(&self) -> Vec<String> {
        self.records
            .iter()
            .filter(|record| record.owner.is_some())
            .filter_map(|record| record.data.session_id.clone())
            .collect()
    }

    pub fn stale(&self) -> Vec<Persisted> {
        self.records
            .iter()
            .filter(|record| record.owner.is_none())
            .map(|record| record.data.clone())
            .collect()
    }

    pub fn drop_restored(&mut self, session_ids: &[String]) {
        self.records.retain(|record| {
            record.owner.is_some()
                || record
                    .data
                    .session_id
                    .as_ref()
                    .is_some_and(|id| !session_ids.contains(id))
        });
        self.save();
    }

    pub fn finalize_shutdown(&mut self) {
        self.records
            .retain(|record| record.data.session_id.is_some());
        self.save();
    }

    pub fn due(&mut self) -> bool {
        let due = self
            .last_maintained
            .is_none_or(|last| last.elapsed() >= MAINTAIN_INTERVAL);
        if due {
            self.last_maintained = Some(Instant::now());
        }
        due
    }

    pub fn observe(
        &mut self,
        owner: u64,
        exited: bool,
        claude_session_id: impl FnOnce() -> Option<String>,
    ) {
        let Some(position) = self
            .records
            .iter()
            .position(|record| record.owner == Some(owner))
        else {
            return;
        };
        if exited {
            let seen = *self.records[position]
                .exit_seen
                .get_or_insert_with(Instant::now);
            if seen.elapsed() >= self.exit_grace {
                self.records.remove(position);
                self.save();
            }
        } else if self.records[position].data.harness == "claude"
            && let Some(session_id) = claude_session_id()
        {
            self.bind(owner, session_id);
        }
    }
}

pub fn login_path(home: &Path, inherited: Option<&str>) -> String {
    let mut dirs: Vec<PathBuf> = vec![
        home.join(".local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".cargo/bin"),
    ];
    let inherited = inherited
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| {
            ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
                .map(PathBuf::from)
                .to_vec()
        });
    for dir in inherited {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).map_or_else(
        |_| String::new(),
        |joined| joined.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ccview-durable-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_corrupt_state_file_is_kept_as_bad_before_the_next_write_replaces_it() {
        let dir = temp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        std::fs::write(&path, "{not json").unwrap();
        let mut durable = Durable::open(path.clone());
        assert!(durable.stale().is_empty());
        let data = Persisted {
            harness: "claude".into(),
            cwd: "/work/app".into(),
            session_id: Some("abc".into()),
            program: None,
            launch: "new".into(),
            title: "t".into(),
            started_ms: 1,
        };
        durable.add(1, data.clone());
        assert_eq!(Durable::open(path).stale(), vec![data]);
        assert_eq!(
            std::fs::read_to_string(dir.join("sessions.json.bad")).unwrap(),
            "{not json"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_login_path_puts_the_user_directories_first_and_keeps_the_inherited_ones_once() {
        let path = login_path(Path::new("/home/u"), Some("/usr/bin:/opt/homebrew/bin"));
        assert_eq!(
            path,
            "/home/u/.local/bin:/opt/homebrew/bin:/usr/local/bin:/home/u/.cargo/bin:/usr/bin"
        );
    }
}
