use crate::{parse::Message, tail::Tailer};
use std::{
    path::PathBuf,
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
    Initial { messages: Vec<Message>, hidden_earlier: usize },
    Appended(Vec<Message>),
    Failed(String),
}

pub struct Follower {
    pub path: PathBuf,
    pub batches: Receiver<Batch>,
    stop: Arc<AtomicBool>,
}

impl Follower {
    pub fn start(path: PathBuf) -> Self {
        let (sender, batches) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_path = path.clone();
        thread::spawn(move || {
            let mut tailer = Tailer::new(thread_path);
            let mut initial = true;
            while !thread_stop.load(Ordering::Relaxed) {
                let batch = match tailer.poll() {
                    Ok(mut messages) if initial => {
                        let hidden_earlier = messages.len().saturating_sub(MAX_INITIAL_MESSAGES);
                        messages.drain(..hidden_earlier);
                        Some(Batch::Initial { messages, hidden_earlier })
                    }
                    Ok(messages) if messages.is_empty() => None,
                    Ok(messages) => Some(Batch::Appended(messages)),
                    Err(err) => Some(Batch::Failed(err.to_string())),
                };
                initial = false;
                if let Some(batch) = batch {
                    if thread_stop.load(Ordering::Relaxed) || sender.send(batch).is_err() {
                        return;
                    }
                }
                thread::sleep(POLL_INTERVAL);
            }
        });
        Self { path, batches, stop }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
