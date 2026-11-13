pub(crate) mod db;
pub(crate) mod extract;
#[allow(dead_code)]
pub(crate) mod fuzzy;
#[allow(dead_code)]
pub(crate) mod handle;
#[allow(dead_code)]
pub(crate) mod hit;
pub(crate) mod indexer;
pub(crate) mod opencode;
#[allow(dead_code)]
pub(crate) mod query;
#[allow(dead_code)]
pub(crate) mod rank;
#[allow(dead_code)]
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
    sync::mpsc::{self, Sender},
    thread,
};

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

pub(crate) fn spawn_indexer(database: PathBuf) -> Sender<Listing> {
    let (sender, listings) = mpsc::channel::<Listing>();
    thread::spawn(move || {
        crate::sys::priority::lower_current_thread();
        let Ok(mut indexer) = indexer::Indexer::open(database) else {
            return;
        };
        let mut next = listings.recv().ok();
        while let Some(first) = next.take() {
            let mut listing = first;
            listing = listings.try_iter().last().unwrap_or(listing);
            let _ = indexer.sync(&listing, &mut || {
                next = listings.try_iter().last();
                next.is_some()
            });
            if next.is_none() {
                next = listings.recv().ok();
            }
        }
    });
    sender
}
