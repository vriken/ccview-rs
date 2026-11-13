use crate::sys::file_stamp::{FileStamp, stamp_of};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

const OPEN_MARK: &str = "[ ]";
const HANDOFF_MARK: &str = "[>]";
const OWNER_KEY: &str = "owner:";
const OWNER_JOINS: [&str; 4] = [", then ", " then ", "/", "+"];
const FIELD_END: [&str; 3] = ["—", " - ", "|"];
const OWN_OWNER: &str = "me";
const WAITING_PHRASE: &str = "waiting on";
const WAIT_LABEL_MAX: usize = 40;
const ELLIPSIS: char = '…';
const BULLETS: [char; 2] = ['-', '*'];
const HEADING_PREFIX: &str = "## ";
const AMENDMENT_HEADING: &str = "amendment";
const HISTORY_HEADINGS: [&str; 2] = ["core", "log"];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DodCounts {
    pub(crate) owed: usize,
    pub(crate) open: usize,
    pub(crate) file_open: usize,
    pub(crate) waits: usize,
    pub(crate) last_wait: Option<String>,
    pub(crate) modified: Option<SystemTime>,
}

#[derive(Default, Clone)]
struct Tally {
    owed: usize,
    open: usize,
}

impl Tally {
    fn plus(self, other: &Tally) -> Tally {
        Tally {
            owed: self.owed + other.owed,
            open: self.open + other.open,
        }
    }
}

struct Section<'a> {
    name: Option<&'a str>,
    tally: Tally,
}

pub(crate) fn dod_dir() -> Option<PathBuf> {
    super::paths::claude_config_dir().map(|dir| dir.join("dod"))
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HandoffOwners(Vec<String>);

impl HandoffOwners {
    pub(crate) fn from_setting(setting: &str) -> Self {
        Self(
            setting
                .split(',')
                .map(|name| fold(name.trim()))
                .filter(|name| !name.is_empty())
                .collect(),
        )
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn names(&self, owner: &str) -> bool {
        let owner = fold(owner.trim_matches(['<', '>']).trim());
        self.0.iter().any(|name| {
            owner
                .strip_prefix(name.as_str())
                .is_some_and(|rest| !rest.starts_with(char::is_alphanumeric))
        })
    }
}

fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            other => other,
        })
        .collect()
}

pub(crate) fn parse(text: &str, owners: &HandoffOwners) -> DodCounts {
    let mut sections = vec![Section {
        name: None,
        tally: Tally::default(),
    }];
    let mut file_open = 0;
    let mut waits = 0;
    let mut last_wait = None;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix(HEADING_PREFIX) {
            sections.push(Section {
                name: Some(heading_name(heading)),
                tally: Tally::default(),
            });
            continue;
        }
        let item = line.trim_start().trim_start_matches(BULLETS).trim_start();
        let Some(section) = sections.last_mut() else {
            continue;
        };
        let tally = &mut section.tally;
        if item.starts_with(OPEN_MARK) {
            tally.open += 1;
            file_open += 1;
        } else if item.starts_with(HANDOFF_MARK) {
            file_open += 1;
            match classify(item, owners) {
                Handoff::Wait(label) => {
                    waits += 1;
                    last_wait = Some(label);
                }
                Handoff::Owed => tally.owed += 1,
                Handoff::Mine | Handoff::Unowned => {}
            }
        }
    }
    let current = current_tally(&sections);
    DodCounts {
        owed: current.owed,
        open: current.open,
        file_open,
        waits,
        last_wait,
        modified: None,
    }
}

fn heading_name(heading: &str) -> &str {
    let end = heading
        .find(|c: char| !c.is_alphanumeric())
        .unwrap_or(heading.len());
    &heading[..end]
}

fn current_tally(sections: &[Section]) -> Tally {
    let last_named = |wanted: &dyn Fn(&str) -> bool| {
        sections
            .iter()
            .rev()
            .find(|section| section.name.is_some_and(wanted))
    };
    let is_amendment = |name: &str| name.eq_ignore_ascii_case(AMENDMENT_HEADING);
    let is_current = |name: &str| {
        !HISTORY_HEADINGS
            .iter()
            .any(|history| name.eq_ignore_ascii_case(history))
    };
    match last_named(&is_amendment).or_else(|| last_named(&is_current)) {
        Some(section) => section.tally.clone(),
        None => sections.iter().fold(Tally::default(), |total, section| {
            total.plus(&section.tally)
        }),
    }
}

fn wait_label(item: &str) -> Option<String> {
    let start = item.to_ascii_lowercase().find(WAITING_PHRASE)?;
    let rest = &item[start + WAITING_PHRASE.len()..];
    let end = field_end(rest);
    Some(truncated(rest[..end].trim()))
}

fn truncated(label: &str) -> String {
    if label.chars().count() <= WAIT_LABEL_MAX {
        return label.to_owned();
    }
    let kept: String = label.chars().take(WAIT_LABEL_MAX - 1).collect();
    format!("{}{ELLIPSIS}", kept.trim_end())
}

fn field_end(rest: &str) -> usize {
    FIELD_END
        .iter()
        .filter_map(|separator| rest.find(separator))
        .min()
        .unwrap_or(rest.len())
}

struct Owner<'a> {
    lead: &'a str,
    then_others: bool,
}

fn owner_of(item: &str) -> Option<Owner<'_>> {
    let start = item.find(OWNER_KEY)?;
    let rest = &item[start + OWNER_KEY.len()..];
    let field = rest[..field_end(rest)].trim();
    let (lead, then_others) = match first_join(field) {
        Some((at, len)) => (&field[..at], !field[at + len..].trim().is_empty()),
        None => (field, false),
    };
    let lead = lead.trim();
    (!lead.is_empty()).then_some(Owner { lead, then_others })
}

fn first_join(field: &str) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    for (at, c) in field.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => {
                if let Some(join) = OWNER_JOINS
                    .iter()
                    .find(|join| field[at..].starts_with(**join))
                {
                    return Some((at, join.len()));
                }
            }
            _ => {}
        }
    }
    None
}

enum Handoff {
    Mine,
    Owed,
    Wait(String),
    Unowned,
}

fn classify(item: &str, owners: &HandoffOwners) -> Handoff {
    if let Some(label) = wait_label(item) {
        return Handoff::Wait(label);
    }
    let Some(owner) = owner_of(item) else {
        return Handoff::Unowned;
    };
    if owner.lead.eq_ignore_ascii_case(OWN_OWNER) {
        return Handoff::Mine;
    }
    let owed = if owners.is_empty() {
        !owner.then_others
    } else {
        owners.names(owner.lead)
    };
    if owed {
        Handoff::Owed
    } else {
        Handoff::Wait(truncated(owner.lead.trim_matches(['<', '>']).trim()))
    }
}

struct Tracked {
    path: PathBuf,
    stamp: Option<FileStamp>,
    counts: Option<DodCounts>,
}

pub(crate) struct DodWatcher {
    dir: PathBuf,
    owners: HandoffOwners,
    listing: Option<FileStamp>,
    files: HashMap<String, Option<Tracked>>,
}

impl DodWatcher {
    pub(crate) fn new(dir: PathBuf, owners: HandoffOwners) -> Self {
        Self {
            dir,
            owners,
            listing: None,
            files: HashMap::new(),
        }
    }

    pub(crate) fn from_env(owners: HandoffOwners) -> Option<Self> {
        dod_dir().map(|dir| Self::new(dir, owners))
    }

    pub(crate) fn poll<'a>(
        &mut self,
        session_ids: impl IntoIterator<Item = &'a str>,
    ) -> HashMap<String, DodCounts> {
        let mut ids: Vec<&str> = session_ids.into_iter().collect();
        ids.sort_unstable();
        ids.dedup();
        let listing = stamp_of(&self.dir);
        if listing != self.listing {
            self.listing = listing;
            self.files.clear();
        }
        self.files.retain(|id, _| ids.contains(&id.as_str()));
        let unseen: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| !self.files.contains_key(*id))
            .collect();
        if !unseen.is_empty() {
            let mut found = newest_files(&self.dir, &unseen);
            for id in unseen {
                let tracked = found.remove(id).map(|path| Tracked {
                    path,
                    stamp: None,
                    counts: None,
                });
                self.files.insert(id.to_owned(), tracked);
            }
        }
        let mut counts = HashMap::new();
        for (id, tracked) in &mut self.files {
            let Some(tracked) = tracked else { continue };
            let stamp = stamp_of(&tracked.path);
            if stamp != tracked.stamp {
                tracked.stamp = stamp;
                tracked.counts = fs::read_to_string(&tracked.path)
                    .ok()
                    .map(|text| DodCounts {
                        modified: stamp.and_then(|stamp| stamp.modified()),
                        ..parse(&text, &self.owners)
                    });
            }
            if let Some(read) = &tracked.counts {
                counts.insert(id.clone(), read.clone());
            }
        }
        counts
    }
}

fn newest_files(dir: &Path, ids: &[&str]) -> HashMap<String, PathBuf> {
    let suffixes: Vec<(String, &str)> = ids.iter().map(|id| (format!("-{id}.md"), *id)).collect();
    let mut newest: HashMap<&str, (SystemTime, PathBuf)> = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return HashMap::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((_, id)) = suffixes.iter().find(|(suffix, _)| name.ends_with(suffix)) else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if newest.get(id).is_none_or(|(best, _)| modified > *best) {
            newest.insert(id, (modified, entry.path()));
        }
    }
    newest
        .into_iter()
        .map(|(id, (_, path))| (id.to_owned(), path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccview-dod-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn only_unresolved_handoffs_owned_by_someone_other_than_me_and_not_waiting_are_owed() {
        let file = "\
# definition of done

- [x] shipped the parser
- [~] waived the benchmark
- [ ] write the docs
- [ ] update the changelog
- [>] approve the release — owner: Alex — action: press merge — why not mine: needs a human
- [>] pick a name — owner: Alex — action: choose one — why not mine: product call
- [>] rerun the sweep — owner: me — action: run it — why not mine: n/a
- [>] vendor answer — owner: <external> — waiting on a reply — why not mine: theirs
- [>x] confirm the key — owner: Alex — action: done — why not mine: resolved
";
        let counts = parse(file, &HandoffOwners::default());
        assert_eq!((counts.owed, counts.open), (2, 2));
    }

    #[test]
    fn with_configured_names_only_those_owners_are_owed_and_every_other_named_owner_is_a_wait() {
        let outcome = |owner: &str, names: &str| {
            let counts = parse(
                &format!("- [>] x — owner: {owner} — action: y — why not mine: z\n"),
                &HandoffOwners::from_setting(names),
            );
            (counts.owed, counts.waits, counts.last_wait)
        };
        let wait = |label: &str| (0, 1, Some(label.to_owned()));
        for owner in [
            "Alex",
            "alex",
            "Alex/Vendor",
            "Alex, then Vendor",
            "Alex (me)",
        ] {
            assert_eq!(outcome(owner, "Alex"), (1, 0, None), "{owner}");
        }
        assert_eq!(outcome("Álex", "alex"), (1, 0, None));
        assert_eq!(outcome("Alexander", "Alex"), wait("Alexander"));
        assert_eq!(outcome("Vendor/Alex", "Alex"), wait("Vendor"));
        assert_eq!(outcome("Vendor (client)", "Alex"), wait("Vendor (client)"));
        assert_eq!(outcome("#team (x/y)", "Alex"), wait("#team (x/y)"));
        assert_eq!(outcome("me", "Alex"), (0, 0, None));
        assert_eq!(outcome("Alex", "Sam, Alex"), (1, 0, None));
        assert_eq!(outcome("Vendor (client)", ""), (1, 0, None));
        assert_eq!(outcome("#team (x/y)", ""), (1, 0, None));
        assert_eq!(outcome("Vendor/Alex", ""), wait("Vendor"));
    }

    #[test]
    fn only_the_newest_amendment_counts_and_without_amendments_the_last_working_section_does() {
        const HANDOFF: &str = "- [>] x — owner: Alex — action: y — why not mine: z\n";
        let owed = |sections: &[(&str, usize)]| {
            let text: String = sections
                .iter()
                .map(|(heading, handoffs)| format!("{heading}\n{}", HANDOFF.repeat(*handoffs)))
                .collect();
            parse(&text, &HandoffOwners::default()).owed
        };

        assert_eq!(
            owed(&[
                ("## Core", 4),
                ("## Amendment 1", 3),
                ("## Amendment 2 (ask: \"x\")", 1),
                ("## Log", 5),
            ]),
            1
        );
        assert_eq!(owed(&[("## Amendment 1", 2), ("## Amendment 2", 0)]), 0);
        assert_eq!(
            owed(&[("## Core", 4), ("## Findings", 2), ("## Log", 5)]),
            2
        );
        assert_eq!(owed(&[("## Core", 4), ("## Log", 5)]), 9);
        assert_eq!(owed(&[("# definition of done", 3)]), 3);
    }

    #[test]
    fn waits_and_done_are_decided_over_the_whole_file_while_owed_and_open_stay_in_the_newest_amendment()
     {
        let file = "\
## Core
- [>] old — owner: <external> — waiting on the old review — check: x
## Amendment 2
- [ ] one open
- [>] reviews — owner: <external> — waiting on PR review from the platform team — check: gh pr view
- [>] vendor — owner: <external> — waiting on a reply
- [>x] closed — owner: <external> — waiting on a closed thread — check: x
- [>] mine — owner: Alex — action: choose — why not mine: call
- [x] waiting on a note
- [>] gate — owner: <external party>, then Alex — action: decide — why not mine: call
- [>] review — owner: Vendor then Alex — action: sign — why not mine: call
- [>] rerun — owner: me, then Alex — action: run it — why not mine: n/a
## Amendment 3
- [x] small tweak
## Log
- [>] log — waiting on a log entry
";
        let counts = parse(file, &HandoffOwners::default());
        assert_eq!(
            (counts.waits, counts.owed, counts.file_open, counts.open),
            (6, 0, 9, 0)
        );
        assert_eq!(counts.last_wait.as_deref(), Some("a log entry"));

        let stale_newest = "\
## Amendment 1
- [>] reviews — owner: <external> — waiting on the data — check: x
- [>] sign off — owner: Alex — action: press merge — why not mine: call
## Amendment 2
- [x] small tweak
";
        let counts = parse(stale_newest, &HandoffOwners::default());
        assert_eq!(
            (counts.waits, counts.owed, counts.open, counts.file_open),
            (1, 0, 0, 2)
        );
        let no_wait = stale_newest.replace("waiting on the data", "ask later");
        let counts = parse(&no_wait, &HandoffOwners::default());
        assert_eq!((counts.waits, counts.file_open), (0, 2));
        let clean =
            "## Amendment 1\n- [>x] a — owner: Alex\n- [x] b\n- [~] c\n## Amendment 2\n- [x] d\n";
        assert_eq!(parse(clean, &HandoffOwners::default()).file_open, 0);

        let gated = parse(
            "- [>] gate — owner: <external party>, then Alex — action: decide\n",
            &HandoffOwners::default(),
        );
        assert_eq!(gated.last_wait.as_deref(), Some("external party"));
        assert_eq!(gated.owed, 0);

        let long = format!("- [>] x — waiting on {}\n", "abcdefghij".repeat(6));
        let label = parse(&long, &HandoffOwners::default()).last_wait.unwrap();
        assert_eq!(label.chars().count(), WAIT_LABEL_MAX);
        assert!(label.ends_with(ELLIPSIS));
    }

    #[test]
    fn a_missing_directory_or_file_yields_nothing_and_a_changed_file_is_read_again() {
        let dir = temp_dir("watch");
        let mut watcher = DodWatcher::new(dir.join("dod"), HandoffOwners::default());
        assert!(watcher.poll(["abc"]).is_empty());

        fs::create_dir_all(dir.join("dod")).unwrap();
        assert!(watcher.poll(["abc"]).is_empty());

        let older = dir.join("dod/2026-01-01-abc.md");
        let newer = dir.join("dod/2026-01-02-abc.md");
        fs::write(
            &older,
            "- [>] a — owner: Alex — action: x — why not mine: y\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&newer, "- [ ] one\n").unwrap();
        assert_eq!(
            watcher
                .poll(["abc"])
                .get("abc")
                .map(|counts| (counts.owed, counts.open)),
            Some((0, 1))
        );

        fs::write(
            &newer,
            "- [>] a — owner: Alex — action: x — why not mine: y\n- [>] b — owner: Alex — action: x — why not mine: y\n",
        )
        .unwrap();
        let counts = watcher.poll(["abc"]).get("abc").cloned().unwrap();
        assert_eq!((counts.owed, counts.open), (2, 0));
        assert_eq!(
            counts.modified,
            fs::metadata(&newer).unwrap().modified().ok()
        );

        fs::write(&newer, "- [x] all done\n").unwrap();
        assert_eq!(watcher.poll(["abc"]).get("abc").map(|c| c.open), Some(0));
        let _ = fs::remove_dir_all(dir);
    }
}
