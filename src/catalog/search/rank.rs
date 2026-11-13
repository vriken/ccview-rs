use super::{
    fuzzy,
    query::{Query, Term, fold, tokens},
};
use crate::session::{entry::SessionEntry, harness_id::HarnessId};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime},
};

const EXACT_TOKEN: f64 = 2.0;
const PREFIX_TOKEN: f64 = 1.0;
const MIN_FUZZY_CHARS: usize = 3;
const RECENCY_HALF_LIFE_DAYS: f64 = 7.0;
const SECONDS_PER_DAY: f64 = 86_400.0;

#[derive(Debug, Clone)]
pub(crate) struct SnapshotEntry {
    pub(crate) path: PathBuf,
    pub(crate) session_id: String,
    pub(crate) title: String,
    pub(crate) cwd: String,
    pub(crate) harness: HarnessId,
    pub(crate) last_used: SystemTime,
    pub(crate) copy_of: Option<String>,
}

impl From<&SessionEntry> for SnapshotEntry {
    fn from(entry: &SessionEntry) -> Self {
        Self {
            path: entry.path.clone(),
            session_id: entry.session_id.clone(),
            title: entry.title.clone(),
            cwd: entry.cwd.clone(),
            harness: entry.harness,
            last_used: entry.last_used,
            copy_of: entry.copy_of.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    Name,
    Fuzzy,
    Body,
}

#[derive(Debug, Clone)]
pub(crate) struct BodyHit {
    pub(crate) path: PathBuf,
    pub(crate) matching_messages: u32,
    pub(crate) bm25: f64,
    pub(crate) rowid: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ranked {
    pub(crate) visible: usize,
    pub(crate) tier: Tier,
    pub(crate) score: f64,
    pub(crate) matching_messages: u32,
    pub(crate) best_hit: Option<(usize, i64)>,
}

struct Group {
    visible: usize,
    members: Vec<usize>,
}

pub(crate) fn rank(
    query: &Query,
    snapshot: &[SnapshotEntry],
    body: &[BodyHit],
    now: SystemTime,
) -> Vec<Ranked> {
    let hits: HashMap<&PathBuf, &BodyHit> = body.iter().map(|hit| (&hit.path, hit)).collect();
    let name_tiers = !query.terms.is_empty() && !query.scoped_to_columns();
    let mut ranked = Vec::new();
    for group in groups(snapshot) {
        let visible = &snapshot[group.visible];
        if !query.admits(&visible.title, &visible.cwd, visible.harness) {
            continue;
        }
        let title = Title::of(&visible.title);
        if query
            .excluded
            .iter()
            .any(|term| title.matches_run(term, term.prefix))
        {
            continue;
        }
        let body = body_hit(&group, snapshot, &hits);
        let matched = if query.terms.is_empty() {
            Some((Tier::Name, 0.0))
        } else {
            name_tiers
                .then(|| title.tier_of(&query.terms))
                .flatten()
                .or_else(|| {
                    body.as_ref().map(|hit| {
                        (
                            Tier::Body,
                            body_score(hit.bm25, age_days(now, visible.last_used)),
                        )
                    })
                })
        };
        let Some((tier, score)) = matched else {
            continue;
        };
        let (target, matching_messages) = match body {
            Some(hit) => (Some((hit.member, hit.rowid)), hit.matching_messages),
            None => (None, 0),
        };
        ranked.push(Ranked {
            visible: group.visible,
            tier,
            score,
            matching_messages,
            best_hit: target,
        });
    }
    ranked.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then(right.score.total_cmp(&left.score))
            .then(
                snapshot[right.visible]
                    .last_used
                    .cmp(&snapshot[left.visible].last_used),
            )
            .then(
                snapshot[left.visible]
                    .session_id
                    .cmp(&snapshot[right.visible].session_id),
            )
    });
    ranked
}

struct GroupHit {
    member: usize,
    rowid: i64,
    bm25: f64,
    matching_messages: u32,
}

fn body_hit(
    group: &Group,
    snapshot: &[SnapshotEntry],
    hits: &HashMap<&PathBuf, &BodyHit>,
) -> Option<GroupHit> {
    let member_hits: Vec<(usize, &BodyHit)> = group
        .members
        .iter()
        .filter_map(|&member| Some((member, *hits.get(&snapshot[member].path)?)))
        .collect();
    let matching_messages = member_hits
        .iter()
        .map(|(_, hit)| hit.matching_messages)
        .max()?;
    let (member, hit) = member_hits
        .iter()
        .find(|(member, _)| *member == group.visible)
        .or_else(|| {
            member_hits
                .iter()
                .min_by(|left, right| left.1.bm25.total_cmp(&right.1.bm25))
        })?;
    Some(GroupHit {
        member: *member,
        rowid: hit.rowid,
        bm25: hit.bm25,
        matching_messages,
    })
}

fn body_score(bm25: f64, age_days: f64) -> f64 {
    -bm25 * (1.0 + 1.0 / (1.0 + age_days / RECENCY_HALF_LIFE_DAYS))
}

fn age_days(now: SystemTime, last_used: SystemTime) -> f64 {
    now.duration_since(last_used)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
        / SECONDS_PER_DAY
}

fn groups(snapshot: &[SnapshotEntry]) -> Vec<Group> {
    let mut by_root: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, entry) in snapshot.iter().enumerate() {
        let root = entry.copy_of.as_deref().unwrap_or(&entry.session_id);
        by_root.entry(root).or_default().push(index);
    }
    by_root
        .into_values()
        .map(|members| {
            let visible = members
                .iter()
                .copied()
                .find(|&member| snapshot[member].copy_of.is_none())
                .or_else(|| {
                    members
                        .iter()
                        .copied()
                        .max_by_key(|&member| snapshot[member].last_used)
                })
                .unwrap_or(members[0]);
            Group { visible, members }
        })
        .collect()
}

struct Title {
    tokens: Vec<String>,
    letters: Vec<char>,
}

impl Title {
    fn of(title: &str) -> Self {
        let folded = fold(title);
        Self {
            tokens: tokens(&folded),
            letters: folded.chars().collect(),
        }
    }

    fn tier_of(&self, terms: &[Term]) -> Option<(Tier, f64)> {
        let mut name_score = 0.0;
        for term in terms {
            match self.name_score(term) {
                Some(score) => name_score += score,
                None => return self.fuzzy_tier(terms),
            }
        }
        Some((Tier::Name, name_score))
    }

    fn name_score(&self, term: &Term) -> Option<f64> {
        self.exact(term).or_else(|| self.prefix(term))
    }

    fn exact(&self, term: &Term) -> Option<f64> {
        self.matches_run(term, false).then_some(EXACT_TOKEN)
    }

    fn prefix(&self, term: &Term) -> Option<f64> {
        let lenient = term.prefix || term.folded.iter().all(|token| token.chars().count() >= 2);
        (lenient && self.matches_run(term, true)).then_some(PREFIX_TOKEN)
    }

    fn matches_run(&self, term: &Term, last_is_prefix: bool) -> bool {
        let wanted = &term.folded;
        let Some(window) = self.tokens.len().checked_sub(wanted.len()) else {
            return false;
        };
        (0..=window).any(|start| {
            wanted.iter().enumerate().all(|(offset, token)| {
                let candidate = &self.tokens[start + offset];
                if last_is_prefix && offset + 1 == wanted.len() {
                    candidate.starts_with(token.as_str())
                } else {
                    candidate == token
                }
            })
        })
    }

    fn fuzzy_tier(&self, terms: &[Term]) -> Option<(Tier, f64)> {
        let mut total = 0;
        for term in terms {
            let [token] = term.folded.as_slice() else {
                return None;
            };
            if token.chars().count() < MIN_FUZZY_CHARS {
                return None;
            }
            let needle: Vec<char> = token.chars().collect();
            total += fuzzy::score(&needle, &self.letters)?;
        }
        Some((Tier::Fuzzy, f64::from(total)))
    }
}
