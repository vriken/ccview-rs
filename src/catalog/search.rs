pub(crate) mod db;
pub(crate) mod extract;
pub(crate) mod fuzzy;
pub(crate) mod handle;
pub(crate) mod hit;
pub(crate) mod indexer;
pub(crate) mod opencode;
pub(crate) mod query;
pub(crate) mod rank;
pub(crate) mod snippet;
#[cfg(test)]
mod tests;

use super::scanner::{CLAUDE_SOURCE, CODEX_SOURCE};
use crate::sources::{
    SessionFile, claude::transcript::parse_claude_line, codex::parse_rollout_line,
};
use extract::LineParser;
use opencode::OpenCodeSession;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Sender},
    },
    thread,
};

#[derive(Default)]
pub(crate) struct IndexProgress {
    done: AtomicUsize,
    total: AtomicUsize,
    built: AtomicBool,
}

impl IndexProgress {
    pub(crate) fn begin(&self, total: usize) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    pub(crate) fn advance(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn finish(&self) {
        self.built.store(true, Ordering::Relaxed);
    }

    pub(crate) fn building(&self) -> Option<(usize, usize)> {
        if self.built.load(Ordering::Relaxed) {
            return None;
        }
        let total = self.total.load(Ordering::Relaxed);
        Some((self.done.load(Ordering::Relaxed).min(total), total))
    }
}

#[derive(Clone)]
pub(crate) struct Listed {
    pub(crate) source: &'static str,
    pub(crate) parser: LineParser,
    pub(crate) file: SessionFile,
}

#[derive(Clone, Default)]
pub(crate) struct Listing {
    pub(crate) files: Vec<Listed>,
    pub(crate) opencode: Vec<OpenCodeSession>,
}

impl Listing {
    #[cfg(test)]
    pub(crate) fn files(files: Vec<Listed>) -> Self {
        Self {
            files,
            opencode: Vec::new(),
        }
    }
}

pub(crate) fn parser_for(source: &str) -> Option<LineParser> {
    match source {
        CLAUDE_SOURCE => Some(parse_claude_line),
        CODEX_SOURCE => Some(parse_rollout_line),
        _ => None,
    }
}

pub(crate) fn default_path() -> PathBuf {
    crate::store::cache_file("search-v1.sqlite")
}

pub(crate) fn spawn_indexer(database: PathBuf, progress: Arc<IndexProgress>) -> Sender<Listing> {
    let (sender, listings) = mpsc::channel::<Listing>();
    thread::spawn(move || {
        crate::sys::priority::lower_current_thread();
        let Ok(opened) = indexer::Indexer::open(database) else {
            progress.finish();
            return;
        };
        let mut indexer = opened.with_progress(Arc::clone(&progress));
        let mut next = listings.recv().ok();
        while let Some(first) = next.take() {
            let mut listing = first;
            listing = listings.try_iter().last().unwrap_or(listing);
            let _ = indexer.sync(&listing, &mut || {
                if next.is_none() {
                    next = listings.try_iter().last();
                }
                next.is_some()
            });
            if next.is_none() {
                progress.finish();
                next = listings.recv().ok();
            }
        }
    });
    sender
}
