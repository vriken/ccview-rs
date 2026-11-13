pub(crate) mod db;
pub(crate) mod extract;
pub(crate) mod indexer;

use crate::sources::SessionFile;
use extract::LineParser;
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

pub(crate) fn default_path() -> PathBuf {
    crate::store::cache_file("search-v1.sqlite")
}

pub(crate) fn spawn_indexer(database: PathBuf) -> Sender<Vec<Listed>> {
    let (sender, listings) = mpsc::channel::<Vec<Listed>>();
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
