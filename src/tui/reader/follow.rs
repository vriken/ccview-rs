use crate::{
    session::{
        cursor::Cursor, entry::TranscriptHandle, harness_id::HarnessId, transcript::Message,
    },
    sources::{claude::transcript, codex, follow::JsonlFollower, opencode::DatabaseFollower},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
pub const INITIAL_MESSAGES: usize = 200;
pub const PAGE_MESSAGES: usize = 200;

pub enum Batch {
    Appended(Vec<Message>),
    Replaced {
        messages: Vec<Message>,
        earlier: Option<Cursor>,
    },
    Earlier {
        requested: Cursor,
        messages: Vec<Message>,
        earlier: Option<Cursor>,
    },
    Failed(String),
}

pub struct Follower {
    pub batches: Receiver<Batch>,
    requests: Sender<Cursor>,
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
            (HarnessId::Codex, TranscriptHandle::Rollout(path)) => {
                Poller::Jsonl(codex::follower(path))
            }
            (_, TranscriptHandle::Rollout(path)) => Poller::Jsonl(transcript::follow(path)),
        }
    }

    fn next_batch(&mut self, cancel: &AtomicBool) -> Option<Batch> {
        match self {
            Poller::Jsonl(follower) => match follower.poll(INITIAL_MESSAGES, cancel) {
                Ok(polled) if polled.replaced => Some(Batch::Replaced {
                    messages: polled.messages,
                    earlier: polled.earlier,
                }),
                Ok(polled) if polled.messages.is_empty() => None,
                Ok(polled) => Some(Batch::Appended(polled.messages)),
                Err(err) => Some(Batch::Failed(err.to_string())),
            },
            Poller::Database(follower) => match follower.poll(INITIAL_MESSAGES) {
                Ok(Some(loaded)) => Some(Batch::Replaced {
                    messages: loaded.messages,
                    earlier: loaded.earlier,
                }),
                Ok(None) => None,
                Err(err) => Some(Batch::Failed(err.to_string())),
            },
        }
    }

    fn earlier(&mut self, requested: Cursor, cancel: &AtomicBool) -> Batch {
        let loaded = match self {
            Poller::Jsonl(follower) => follower
                .earlier(&requested, PAGE_MESSAGES, cancel)
                .map(|page| (page.messages, page.earlier))
                .map_err(|err| err.to_string()),
            Poller::Database(follower) => follower
                .earlier(&requested, PAGE_MESSAGES)
                .map(|loaded| (loaded.messages, loaded.earlier))
                .map_err(|err| err.to_string()),
        };
        match loaded {
            Ok((messages, earlier)) => Batch::Earlier {
                requested,
                messages,
                earlier,
            },
            Err(err) => Batch::Failed(err),
        }
    }
}

pub fn load_first_page(
    harness: HarnessId,
    handle: TranscriptHandle,
    cancel: &AtomicBool,
) -> Option<(Vec<Message>, Option<Cursor>)> {
    match Poller::open(harness, handle).next_batch(cancel) {
        Some(Batch::Replaced { messages, earlier }) => Some((messages, earlier)),
        _ => None,
    }
}

impl Follower {
    pub fn start(harness: HarnessId, handle: TranscriptHandle) -> Self {
        let (sender, batches) = mpsc::channel();
        let (requests, requested) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            #[cfg(test)]
            let _live = LiveThread::enter();
            let mut poller = Poller::open(harness, handle);
            while !thread_stop.load(Ordering::Relaxed) {
                for cursor in requested.try_iter() {
                    let batch = poller.earlier(cursor, &thread_stop);
                    if thread_stop.load(Ordering::Relaxed) || sender.send(batch).is_err() {
                        return;
                    }
                }
                let batch = poller.next_batch(&thread_stop);
                if let Some(batch) = batch
                    && (thread_stop.load(Ordering::Relaxed) || sender.send(batch).is_err())
                {
                    return;
                }
                thread::park_timeout(POLL_INTERVAL);
            }
        });
        Self {
            batches,
            requests,
            stop,
            worker: Some(worker.thread().clone()),
        }
    }

    pub fn request_earlier(&self, before: Cursor) {
        if self.requests.send(before).is_ok()
            && let Some(worker) = &self.worker
        {
            worker.unpark();
        }
    }

    #[cfg(test)]
    pub fn detached(batches: Receiver<Batch>) -> Self {
        Self {
            batches,
            requests: mpsc::channel().0,
            stop: Arc::default(),
            worker: None,
        }
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
    use crate::session::transcript::{Block, Role};
    use std::{fs, path::PathBuf, time::Instant};

    fn next(follower: &Follower) -> Batch {
        follower
            .batches
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
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
        let Batch::Replaced { messages, .. } = next(&claude) else {
            panic!("expected initial batch")
        };
        assert_eq!(texts(&messages), ["from claude"]);

        let codex = Follower::start(HarnessId::Codex, TranscriptHandle::Rollout(path));
        let Batch::Replaced { messages, .. } = next(&codex) else {
            panic!("expected initial batch")
        };
        assert_eq!(texts(&messages), ["from codex"]);
        assert_eq!(messages[0].role, Role::User);
        fs::remove_dir_all(dir).unwrap();
    }

    const LINE_A: &str = r#"{"type":"user","message":{"content":"first"}}"#;
    const LINE_B: &str = r#"{"type":"user","message":{"content":"second"}}"#;
    const LINE_C: &str = r#"{"type":"user","message":{"content":"third"}}"#;

    #[test]
    fn a_truncated_transcript_replaces_the_conversation_instead_of_appending() {
        let dir = temp_dir("truncate");
        let path = dir.join("session.jsonl");
        fs::write(&path, format!("{LINE_A}\n{LINE_B}\n")).unwrap();
        let follower = Follower::start(HarnessId::Claude, TranscriptHandle::Rollout(path.clone()));
        let Batch::Replaced { messages, .. } = next(&follower) else {
            panic!("expected initial batch")
        };
        assert_eq!(texts(&messages), ["first", "second"]);

        fs::write(&path, format!("{LINE_C}\n")).unwrap();
        let Batch::Replaced { messages, .. } = next(&follower) else {
            panic!("a truncated file must replace, not append")
        };
        assert_eq!(texts(&messages), ["third"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_replaced_file_replaces_the_conversation() {
        let dir = temp_dir("replace");
        let path = dir.join("session.jsonl");
        fs::write(&path, format!("{LINE_A}\n")).unwrap();
        let follower = Follower::start(HarnessId::Claude, TranscriptHandle::Rollout(path.clone()));
        let Batch::Replaced { .. } = next(&follower) else {
            panic!("expected initial batch")
        };

        let successor = dir.join("successor.jsonl");
        fs::write(&successor, format!("{LINE_B}\n{LINE_C}\n{LINE_C}\n")).unwrap();
        fs::rename(&successor, &path).unwrap();
        let Batch::Replaced { messages, .. } = next(&follower) else {
            panic!("a swapped inode must replace, not append")
        };
        assert_eq!(texts(&messages), ["second", "third", "third"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn growth_after_a_failed_first_poll_arrives_as_a_replacing_snapshot() {
        let dir = temp_dir("first-failure");
        let path = dir.join("session.jsonl");
        fs::create_dir(&path).unwrap();
        let follower = Follower::start(HarnessId::Claude, TranscriptHandle::Rollout(path.clone()));
        let Batch::Failed(_) = next(&follower) else {
            panic!("expected a failure")
        };

        fs::remove_dir(&path).unwrap();
        fs::write(&path, format!("{LINE_A}\n")).unwrap();
        loop {
            match next(&follower) {
                Batch::Failed(_) => continue,
                Batch::Replaced { messages, .. } => {
                    assert_eq!(texts(&messages), ["first"]);
                    break;
                }
                Batch::Appended(_) | Batch::Earlier { .. } => {
                    panic!("the first successful batch must replace")
                }
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_database_handle_opens_on_the_latest_messages_and_pages_back_on_request() {
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
        for index in 0..INITIAL_MESSAGES + 5 {
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
            TranscriptHandle::Database {
                path: database.clone(),
                session_id: "s1".to_owned(),
            },
        );
        let Batch::Replaced { messages, earlier } = next(&follower) else {
            panic!("expected a replacing snapshot")
        };
        assert_eq!(messages.len(), INITIAL_MESSAGES);
        assert_eq!(texts(&messages).last().copied(), Some("msg 204"));
        let cursor = earlier.expect("five older messages exist");

        follower.request_earlier(cursor.clone());
        let Batch::Earlier {
            requested,
            messages,
            earlier,
        } = next(&follower)
        else {
            panic!("expected the earlier page")
        };
        assert_eq!(requested, cursor);
        assert_eq!(
            texts(&messages),
            ["msg 0", "msg 1", "msg 2", "msg 3", "msg 4"]
        );
        assert!(earlier.is_none());

        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute("INSERT INTO message VALUES ('m99999', 's1', 99999, 99999, '{\"role\":\"assistant\"}')", [])
            .unwrap();
        connection
            .execute("INSERT INTO part VALUES ('p99999', 'm99999', 's1', 99999, 99999, '{\"type\":\"text\",\"text\":\"late\"}')", [])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let Batch::Replaced { messages, earlier } = next(&follower) else {
                panic!("expected a replacing snapshot")
            };
            if texts(&messages).last().copied() == Some("late") {
                assert_eq!(messages.len(), INITIAL_MESSAGES + 5 + 1);
                assert!(earlier.is_none());
                break;
            }
            assert!(Instant::now() < deadline);
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
