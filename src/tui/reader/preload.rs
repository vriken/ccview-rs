use super::{conversation::Conversation, follow};
use crate::{
    session::{entry::TranscriptHandle, harness_id::HarnessId},
    sys::priority,
};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

pub(crate) const MAX_CONCURRENT: usize = 2;

pub(crate) struct Wanted {
    pub(crate) key: PathBuf,
    pub(crate) harness: HarnessId,
    pub(crate) handle: TranscriptHandle,
}

pub(crate) struct Loaded {
    pub(crate) key: PathBuf,
    pub(crate) conversation: Conversation,
    pub(crate) messages: usize,
}

pub(crate) struct Preloader {
    queue: VecDeque<Wanted>,
    attempted: HashSet<PathBuf>,
    running: usize,
    finished: Receiver<Option<Loaded>>,
    sender: Sender<Option<Loaded>>,
    cancel: Arc<AtomicBool>,
}

impl Default for Preloader {
    fn default() -> Self {
        let (sender, finished) = mpsc::channel();
        Self {
            queue: VecDeque::new(),
            attempted: HashSet::new(),
            running: 0,
            finished,
            sender,
            cancel: Arc::default(),
        }
    }
}

impl Drop for Preloader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Preloader {
    pub(crate) fn want(&mut self, wanted: Vec<Wanted>) {
        let mut seen = HashSet::new();
        let wanted: Vec<Wanted> = wanted
            .into_iter()
            .filter(|wanted| seen.insert(wanted.key.clone()))
            .collect();
        self.attempted.retain(|key| seen.contains(key));
        self.queue = wanted
            .into_iter()
            .filter(|wanted| !self.attempted.contains(&wanted.key))
            .collect();
    }

    pub(crate) fn finished(&mut self) -> Vec<Loaded> {
        let results: Vec<Option<Loaded>> = self.finished.try_iter().collect();
        self.running -= results.len();
        results.into_iter().flatten().collect()
    }

    pub(crate) fn next(&mut self) -> Option<Wanted> {
        if self.running >= MAX_CONCURRENT {
            return None;
        }
        self.queue.pop_front()
    }

    pub(crate) fn drop_queue(&mut self) {
        self.queue.clear();
    }

    pub(crate) fn skip(&mut self, key: PathBuf) {
        self.attempted.insert(key);
    }

    pub(crate) fn start(&mut self, wanted: Wanted) {
        self.attempted.insert(wanted.key.clone());
        self.running += 1;
        let sender = self.sender.clone();
        let cancel = Arc::clone(&self.cancel);
        thread::spawn(move || {
            priority::lower_current_thread();
            let loaded = follow::load_first_page(wanted.harness, wanted.handle, &cancel)
                .filter(|(messages, _)| !messages.is_empty())
                .map(|(messages, earlier)| {
                    let mut conversation = Conversation::default();
                    conversation.replace_messages(&messages, earlier);
                    Loaded {
                        key: wanted.key,
                        conversation,
                        messages: messages.len(),
                    }
                });
            let _ = sender.send(loaded.filter(|_| !cancel.load(Ordering::Relaxed)));
        });
    }

    pub(crate) fn running(&self) -> usize {
        self.running
    }

    #[cfg(test)]
    pub(crate) fn attempted(&self) -> usize {
        self.attempted.len()
    }
}
