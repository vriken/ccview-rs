use crate::{
    parse::Message,
    sources::{
        HarnessId, TranscriptHandle, codex,
        follow::JsonlFollower,
        opencode::DatabaseFollower,
    },
    tail,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const MAX_INITIAL_MESSAGES: usize = 1500;

pub enum Batch {
    Appended(Vec<Message>),
    Replaced { messages: Vec<Message>, hidden_earlier: usize },
    Failed(String),
}

pub struct Follower {
    pub batches: Receiver<Batch>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::Thread>,
}

#[cfg(test)]
pub static LIVE_THREADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
struct LiveThread;

#[cfg(test)]
impl LiveThread {
    fn enter() -> Self {
        LIVE_THREADS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

#[cfg(test)]
impl Drop for LiveThread {
    fn drop(&mut self) {
        LIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
    }
}

enum Poller {
    Jsonl(JsonlFollower),
    Database(DatabaseFollower),
}

impl Poller {
    fn open(harness: HarnessId, handle: TranscriptHandle) -> Self {
        match (harness, handle) {
            (_, TranscriptHandle::Database { path, session_id }) => {
                Poller::Database(DatabaseFollower::new(path, session_id))
            }
            (HarnessId::Codex, TranscriptHandle::Rollout(path)) => Poller::Jsonl(codex::follower(path)),
            (_, TranscriptHandle::Rollout(path)) => Poller::Jsonl(tail::tailer(path)),
        }
    }

    fn next_batch(&mut self, first_poll: bool) -> Option<Batch> {
        match self {
            Poller::Jsonl(follower) => match follower.poll() {
                Ok(messages) if first_poll => {
                    let (messages, hidden_earlier) = keep_latest(messages);
                    Some(Batch::Replaced { messages, hidden_earlier })
                }
                Ok(messages) if messages.is_empty() => None,
                Ok(messages) => Some(Batch::Appended(messages)),
                Err(err) => Some(Batch::Failed(err.to_string())),
            },
            Poller::Database(follower) => match follower.poll() {
                Ok(Some(session)) => {
                    let (messages, hidden_earlier) = keep_latest(session.messages);
                    Some(Batch::Replaced { messages, hidden_earlier })
                }
                Ok(None) => None,
                Err(err) => Some(Batch::Failed(err.to_string())),
            },
        }
    }
}

fn keep_latest(mut messages: Vec<Message>) -> (Vec<Message>, usize) {
    let hidden_earlier = messages.len().saturating_sub(MAX_INITIAL_MESSAGES);
    messages.drain(..hidden_earlier);
    (messages, hidden_earlier)
}

impl Follower {
    pub fn start(harness: HarnessId, handle: TranscriptHandle) -> Self {
        let (sender, batches) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            #[cfg(test)]
            let _live = LiveThread::enter();
            let mut poller = Poller::open(harness, handle);
            let mut first_poll = true;
            while !thread_stop.load(Ordering::Relaxed) {
                let batch = poller.next_batch(first_poll);
                first_poll = false;
                if let Some(batch) = batch {
                    if thread_stop.load(Ordering::Relaxed) || sender.send(batch).is_err() {
                        return;
                    }
                }
                thread::park_timeout(POLL_INTERVAL);
            }
        });
        Self { batches, stop, worker: Some(worker.thread().clone()) }
    }

    #[cfg(test)]
    pub fn detached(batches: Receiver<Batch>) -> Self {
        Self { batches, stop: Arc::default(), worker: None }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = &self.worker {
            worker.unpark();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Block, Role};
    use std::{fs, path::PathBuf, time::Instant};

    fn next(follower: &Follower) -> Batch {
        follower.batches.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block {
                Block::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccview-follow-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_harness_selects_the_line_format_for_a_rollout_path() {
        let dir = temp_dir("dispatch");
        let claude_line = r#"{"type":"user","message":{"content":"from claude"}}"#;
        let codex_line = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"from codex"}]}}"#;
        let path = dir.join("session.jsonl");
        fs::write(&path, format!("{claude_line}\n{codex_line}\n")).unwrap();

        let claude = Follower::start(HarnessId::Claude, TranscriptHandle::Rollout(path.clone()));
        let Batch::Replaced { messages, .. } = next(&claude) else { panic!("expected initial batch") };
        assert_eq!(texts(&messages), ["from claude"]);

        let codex = Follower::start(HarnessId::Codex, TranscriptHandle::Rollout(path));
        let Batch::Replaced { messages, .. } = next(&codex) else { panic!("expected initial batch") };
        assert_eq!(texts(&messages), ["from codex"]);
        assert_eq!(messages[0].role, Role::User);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_database_handle_yields_replacing_snapshots_capped_to_the_latest_messages() {
        let dir = temp_dir("database");
        let database = dir.join("opencode.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT, time_updated INTEGER, time_archived INTEGER);
                 CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
                 CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
                 INSERT INTO session VALUES ('s1', NULL, '/w', 't', 1, NULL);",
            )
            .unwrap();
        for index in 0..MAX_INITIAL_MESSAGES + 5 {
            connection
                .execute(
                    "INSERT INTO message VALUES (?1, 's1', ?2, ?2, '{\"role\":\"user\"}')",
                    rusqlite::params![format!("m{index:05}"), index as i64],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO part VALUES (?1, ?2, 's1', ?3, ?3, ?4)",
                    rusqlite::params![
                        format!("p{index:05}"),
                        format!("m{index:05}"),
                        index as i64,
                        format!("{{\"type\":\"text\",\"text\":\"msg {index}\"}}")
                    ],
                )
                .unwrap();
        }
        drop(connection);

        let follower = Follower::start(
            HarnessId::OpenCode,
            TranscriptHandle::Database { path: database.clone(), session_id: "s1".to_owned() },
        );
        let Batch::Replaced { messages, hidden_earlier } = next(&follower) else {
            panic!("expected a replacing snapshot")
        };
        assert_eq!((messages.len(), hidden_earlier), (MAX_INITIAL_MESSAGES, 5));
        assert_eq!(texts(&messages).last().copied(), Some("msg 1504"));

        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute("INSERT INTO message VALUES ('m99999', 's1', 99999, 99999, '{\"role\":\"assistant\"}')", [])
            .unwrap();
        connection
            .execute("INSERT INTO part VALUES ('p99999', 'm99999', 's1', 99999, 99999, '{\"type\":\"text\",\"text\":\"late\"}')", [])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let Batch::Replaced { messages, hidden_earlier } = next(&follower) else {
                panic!("expected a replacing snapshot")
            };
            if texts(&messages).last().copied() == Some("late") {
                assert_eq!((messages.len(), hidden_earlier), (MAX_INITIAL_MESSAGES, 6));
                break;
            }
            assert!(Instant::now() < deadline);
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
