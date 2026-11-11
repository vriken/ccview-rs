use crate::{
    catalog::checkout::CheckoutKeys,
    session::{activity::ActivityMap, entry::SessionEntry, live::LiveSession},
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

pub(crate) enum Update {
    Scanned(Vec<SessionEntry>),
    Removed(Vec<PathBuf>),
    InitialScanDone,
    Live(Vec<LiveSession>),
    Activity(ActivityMap),
    Checkouts(CheckoutKeys),
}

#[derive(Clone)]
pub(crate) struct Feed {
    pub(super) sender: mpsc::Sender<Update>,
    pub(super) wake: Arc<dyn Fn() + Send + Sync>,
}

impl Feed {
    pub(crate) fn send(&self, update: Update) -> bool {
        let delivered = self.sender.send(update).is_ok();
        if delivered {
            (self.wake)();
        }
        delivered
    }
}
