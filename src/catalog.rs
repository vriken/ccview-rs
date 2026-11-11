pub(crate) mod checkout;
pub(crate) mod feed;
pub(crate) mod index;
pub(crate) mod live;
pub(crate) mod scanner;
pub(crate) mod worker;

use crate::{
    catalog::checkout::{CheckoutKeys, SharedCheckouts},
    session::{
        activity::{Activity, ActivityMap},
        entry::{SessionEntry, TitleSource},
        lineage::{self},
        live::LiveSession,
    },
    store::dismissed::{Dismissed, Observed},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

#[derive(Default)]
pub(crate) struct Catalog {
    entries: Vec<SessionEntry>,
    live: Vec<LiveSession>,
    raw_activity: ActivityMap,
    activity: ActivityMap,
    expanded: bool,
    groups: Vec<lineage::Group>,
    dismissed: Dismissed,
    checkouts: CheckoutKeys,
    shared: SharedCheckouts,
    live_seen: bool,
    activity_seen: bool,
}

impl Catalog {
    pub(crate) fn live(&self) -> &[LiveSession] {
        &self.live
    }

    pub(crate) fn activity(&self) -> &ActivityMap {
        &self.activity
    }

    pub(crate) fn shared_checkouts(&self) -> &SharedCheckouts {
        &self.shared
    }

    pub(crate) fn set_checkouts(&mut self, checkouts: CheckoutKeys) {
        self.checkouts = checkouts;
        self.derive_shared();
    }

    pub(crate) fn dismissed(&self) -> &Dismissed {
        &self.dismissed
    }

    pub(crate) fn set_dismissed(&mut self, dismissed: Dismissed) {
        self.dismissed = dismissed;
        self.derive_activity();
    }

    pub(crate) fn dismiss(&mut self, session_id: &str, pid: Option<u32>) -> std::io::Result<()> {
        let saved = self.dismissed.dismiss(session_id, pid);
        self.derive_activity();
        saved
    }

    pub(crate) fn undismiss(&mut self, session_id: &str) -> std::io::Result<()> {
        let saved = self.dismissed.undismiss(session_id);
        self.derive_activity();
        saved
    }

    pub(crate) fn set_activity(&mut self, activity: ActivityMap) {
        self.raw_activity = activity;
        self.activity_seen = true;
        self.derive_activity();
    }

    pub(crate) fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub(crate) fn expanded(&self) -> bool {
        self.expanded
    }

    pub(crate) fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.apply_groups();
    }

    pub(crate) fn lineage_ids(&self, session_id: &str) -> Vec<String> {
        let ids = |group: &lineage::Group| -> Vec<String> {
            std::iter::once(group.visible)
                .chain(group.copies.iter().copied())
                .map(|index| self.entries[index].session_id.clone())
                .collect()
        };
        self.groups
            .iter()
            .map(ids)
            .find(|ids| ids.iter().any(|id| id == session_id))
            .unwrap_or_else(|| vec![session_id.to_owned()])
    }

    pub(crate) fn is_collapsed_copy(&self, entry: &SessionEntry) -> bool {
        !self.expanded && entry.copy_of.is_some()
    }

    pub(crate) fn merge(&mut self, scanned: Vec<SessionEntry>) {
        let mut positions: HashMap<PathBuf, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.path.clone(), position))
            .collect();
        for mut entry in scanned {
            entry.live = live_for(&self.live, &entry.session_id);
            match positions.get(&entry.path) {
                Some(&position) => self.entries[position] = entry,
                None => {
                    positions.insert(entry.path.clone(), self.entries.len());
                    self.entries.push(entry);
                }
            }
        }
        self.entries.sort_by(|a, b| {
            b.last_used
                .cmp(&a.last_used)
                .then_with(|| a.path.cmp(&b.path))
        });
        self.regroup();
    }

    pub(crate) fn remove(&mut self, paths: &[PathBuf]) {
        let gone: HashSet<&PathBuf> = paths.iter().collect();
        self.entries.retain(|entry| !gone.contains(&entry.path));
        self.regroup();
    }

    pub(crate) fn set_live(&mut self, live: Vec<LiveSession>) {
        self.live = live;
        self.live_seen = true;
        self.apply_groups();
    }

    fn regroup(&mut self) {
        let members: Vec<lineage::Member> = self
            .entries
            .iter()
            .map(|entry| lineage::Member {
                path: &entry.path,
                last_used: entry.last_used,
                fingerprint: entry.fingerprint.as_deref(),
            })
            .collect();
        self.groups = lineage::group(&members)
            .into_iter()
            .filter(|group| !group.copies.is_empty())
            .collect();
        self.apply_groups();
    }

    fn apply_groups(&mut self) {
        for entry in &mut self.entries {
            entry.live = live_for(&self.live, &entry.session_id);
            if let Some(name) = entry.live.as_ref().and_then(user_given_name) {
                entry.title = name;
                entry.title_source = TitleSource::Custom;
            }
            entry.copy_of = None;
            entry.copies.clear();
        }
        let live: Vec<Option<LiveSession>> = self
            .entries
            .iter()
            .map(|entry| entry.live.clone())
            .collect();
        for group in &self.groups {
            let visible_id = self.entries[group.visible].session_id.clone();
            for &copy in &group.copies {
                self.entries[copy].copy_of = Some(visible_id.clone());
            }
            if self.expanded {
                continue;
            }
            let copy_ids: Vec<String> = group
                .copies
                .iter()
                .map(|&copy| self.entries[copy].session_id.clone())
                .collect();
            let visible = &mut self.entries[group.visible];
            visible.copies = copy_ids;
            if visible.live.is_none() {
                visible.live = group.copies.iter().find_map(|&copy| live[copy].clone());
            }
        }
        self.derive_activity();
    }

    fn derive_activity(&mut self) {
        let mut effective = self.raw_activity.clone();
        if !self.expanded {
            for entry in self.entries.iter().filter(|entry| !entry.copies.is_empty()) {
                let strongest = std::iter::once(&entry.session_id)
                    .chain(&entry.copies)
                    .filter_map(|id| self.raw_activity.get(id))
                    .max_by_key(|activity| (activity.is_active(), activity.state.precedence()))
                    .cloned();
                for id in &entry.copies {
                    effective.remove(id);
                }
                match strongest {
                    Some(activity) => effective.insert(entry.session_id.clone(), activity),
                    None => effective.remove(&entry.session_id),
                };
            }
        }
        if self.live_seen && self.activity_seen {
            let observed: HashMap<&str, Observed> = self
                .entries
                .iter()
                .map(|entry| {
                    let pid = entry
                        .live
                        .as_ref()
                        .filter(|live| live.pid_alive)
                        .map(|live| live.pid);
                    (
                        entry.session_id.as_str(),
                        Observed {
                            pid,
                            activity: effective.get(&entry.session_id),
                        },
                    )
                })
                .collect();
            self.dismissed.reconcile(&observed);
        }
        effective.retain(|session_id, _| !self.dismissed.contains(session_id));
        self.activity = effective;
        self.derive_shared();
    }

    fn derive_shared(&mut self) {
        let running = |session_id: &str| {
            !self.dismissed.contains(session_id)
                && self
                    .activity
                    .get(session_id)
                    .is_some_and(Activity::is_active)
                && self
                    .live
                    .iter()
                    .any(|live| live.pid_alive && live.session_id.as_deref() == Some(session_id))
        };
        self.shared = SharedCheckouts::compute(&self.entries, &self.checkouts, running);
    }
}

fn user_given_name(live: &LiveSession) -> Option<String> {
    let name = live.name.as_deref()?.trim();
    (live.name_source.as_deref() == Some("user") && !name.is_empty()).then(|| name.to_owned())
}

fn live_for(live: &[LiveSession], session_id: &str) -> Option<LiveSession> {
    live.iter()
        .find(|session| session.session_id.as_deref() == Some(session_id))
        .cloned()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod lineage_tests;

#[cfg(test)]
mod age_tests;
