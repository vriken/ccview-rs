use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::SystemTime,
};

pub(crate) const SKETCH_SIZE: usize = 128;
pub(crate) const MIN_SHARED_SAMPLES: usize = 8;
const CONTAINED_PERCENT: usize = 95;
const MESSAGE_PREFIX: &[u8] = br#"{"parentUuid""#;
const UUID_KEY: &[u8] = br#""uuid":""#;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const HEX_DIGITS_PER_HASH: usize = 8;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Fingerprint {
    pub(crate) count: u32,
    #[serde(with = "hex_hashes")]
    sketch: Vec<u32>,
}

impl Fingerprint {
    pub(crate) fn add_line(&mut self, line: &[u8]) {
        if let Some(uuid) = uuid_of(line) {
            self.add_uuid(uuid);
        }
    }

    pub(crate) fn add_uuid(&mut self, uuid: &[u8]) {
        self.count += 1;
        let hash = hash_uuid(uuid);
        if self.sketch.len() == SKETCH_SIZE
            && self.sketch.last().is_some_and(|&largest| hash >= largest)
        {
            return;
        }
        if let Err(position) = self.sketch.binary_search(&hash) {
            self.sketch.insert(position, hash);
            self.sketch.truncate(SKETCH_SIZE);
        }
    }

    fn saturated(&self) -> bool {
        self.sketch.len() >= SKETCH_SIZE
    }

    fn threshold(&self) -> u32 {
        match self.sketch.last() {
            Some(&largest) if self.saturated() => largest,
            _ => u32::MAX,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sketch.is_empty()
    }
}

fn uuid_of(line: &[u8]) -> Option<&[u8]> {
    if !line.starts_with(MESSAGE_PREFIX) {
        return None;
    }
    let start = line
        .windows(UUID_KEY.len())
        .rposition(|window| window == UUID_KEY)?
        + UUID_KEY.len();
    let length = line[start..].iter().position(|&byte| byte == b'"')?;
    Some(&line[start..start + length])
}

fn hash_uuid(uuid: &[u8]) -> u32 {
    let mut hash = uuid.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    });
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51afd7ed558ccd);
    hash ^= hash >> 33;
    (hash >> 32) as u32
}

mod hex_hashes {
    use super::HEX_DIGITS_PER_HASH;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::fmt::Write;

    pub(crate) fn serialize<S: Serializer>(
        hashes: &[u32],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut text = String::with_capacity(hashes.len() * HEX_DIGITS_PER_HASH);
        for hash in hashes {
            let _ = write!(text, "{hash:08x}");
        }
        serializer.serialize_str(&text)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u32>, D::Error> {
        let text = String::deserialize(deserializer)?;
        if !text.is_ascii() || text.len() % HEX_DIGITS_PER_HASH != 0 {
            return Err(D::Error::custom("malformed fingerprint"));
        }
        text.as_bytes()
            .chunks(HEX_DIGITS_PER_HASH)
            .map(|chunk| {
                std::str::from_utf8(chunk)
                    .ok()
                    .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                    .ok_or_else(|| D::Error::custom("malformed fingerprint"))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    Unrelated,
    Duplicate,
    FirstIsAncestor,
    SecondIsAncestor,
    Fork,
}

pub(crate) fn relation(first: &Fingerprint, second: &Fingerprint) -> Relation {
    let threshold = first.threshold().min(second.threshold());
    let in_range = |fingerprint: &Fingerprint| -> Vec<u32> {
        fingerprint
            .sketch
            .iter()
            .copied()
            .filter(|&hash| hash <= threshold)
            .collect()
    };
    let (first_range, second_range) = (in_range(first), in_range(second));
    let second_set: HashSet<u32> = second_range.iter().copied().collect();
    let shared = first_range
        .iter()
        .filter(|hash| second_set.contains(hash))
        .count();
    if shared == 0 || first_range.len().min(second_range.len()) < MIN_SHARED_SAMPLES {
        return Relation::Unrelated;
    }
    let first_contained = shared * 100 >= first_range.len() * CONTAINED_PERCENT;
    let second_contained = shared * 100 >= second_range.len() * CONTAINED_PERCENT;
    match (first_contained, second_contained) {
        (true, true) => Relation::Duplicate,
        (true, false) => Relation::FirstIsAncestor,
        (false, true) => Relation::SecondIsAncestor,
        (false, false) => Relation::Fork,
    }
}

pub(crate) struct Member<'a> {
    pub(crate) path: &'a PathBuf,
    pub(crate) last_used: SystemTime,
    pub(crate) fingerprint: Option<&'a Fingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) visible: usize,
    pub(crate) copies: Vec<usize>,
}

fn recency<'a>(members: &[Member<'a>], index: usize) -> (SystemTime, &'a PathBuf) {
    (members[index].last_used, members[index].path)
}

fn newer(members: &[Member], a: usize, b: usize) -> Ordering {
    recency(members, a).cmp(&recency(members, b))
}

pub(crate) fn candidate_pairs(members: &[Member]) -> Vec<(usize, usize)> {
    let mut by_hash: HashMap<u32, Vec<usize>> = HashMap::new();
    for (index, member) in members.iter().enumerate() {
        if let Some(fingerprint) = member.fingerprint {
            for &hash in &fingerprint.sketch {
                by_hash.entry(hash).or_default().push(index);
            }
        }
    }
    let mut pairs = HashSet::new();
    for sharing in by_hash.values().filter(|sharing| sharing.len() > 1) {
        for (position, &first) in sharing.iter().enumerate() {
            for &second in &sharing[position + 1..] {
                pairs.insert((first, second));
            }
        }
    }
    let mut pairs: Vec<(usize, usize)> = pairs.into_iter().collect();
    pairs.sort_unstable();
    pairs
}

pub(crate) fn group(members: &[Member]) -> Vec<Group> {
    let mut descendants: Vec<Vec<usize>> = vec![Vec::new(); members.len()];
    for (first, second) in candidate_pairs(members) {
        let (Some(a), Some(b)) = (members[first].fingerprint, members[second].fingerprint) else {
            continue;
        };
        match relation(a, b) {
            Relation::FirstIsAncestor => descendants[first].push(second),
            Relation::SecondIsAncestor => descendants[second].push(first),
            Relation::Duplicate => {
                let (older, newest) = if newer(members, first, second) == Ordering::Less {
                    (first, second)
                } else {
                    (second, first)
                };
                descendants[older].push(newest);
            }
            Relation::Fork | Relation::Unrelated => {}
        }
    }
    let mut attached: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..members.len() {
        attached
            .entry(leaf_of(index, &descendants, members))
            .or_default()
            .push(index);
    }
    let mut groups: Vec<Group> = attached
        .into_values()
        .map(|all| {
            let visible = *all
                .iter()
                .max_by(|&&a, &&b| newer(members, a, b))
                .unwrap_or(&all[0]);
            let mut copies: Vec<usize> =
                all.into_iter().filter(|&index| index != visible).collect();
            copies.sort_by(|&a, &b| newer(members, b, a));
            Group { visible, copies }
        })
        .collect();
    groups.sort_by_key(|group| group.visible);
    groups
}

fn leaf_of(start: usize, descendants: &[Vec<usize>], members: &[Member]) -> usize {
    let mut current = start;
    let mut seen = HashSet::from([start]);
    while let Some(&next) = descendants[current]
        .iter()
        .max_by(|&&a, &&b| newer(members, a, b))
    {
        if !seen.insert(next) {
            return start;
        }
        current = next;
    }
    current
}

#[cfg(test)]
pub(crate) fn fingerprint_of(uuids: impl IntoIterator<Item = u32>) -> Fingerprint {
    let mut fingerprint = Fingerprint::default();
    for uuid in uuids {
        fingerprint.add_uuid(format!("{uuid:08}-0000-4000-8000-000000000000").as_bytes());
    }
    fingerprint
}

#[cfg(test)]
mod tests;
