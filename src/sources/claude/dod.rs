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
const OWNER_END: [&str; 3] = ["—", " - ", "|"];
const OWN_OWNER: &str = "me";
const WAITING_PHRASE: &str = "waiting on";
const BULLETS: [char; 2] = ['-', '*'];
const HEADING_PREFIX: &str = "## ";
const AMENDMENT_HEADING: &str = "amendment";
const HISTORY_HEADINGS: [&str; 2] = ["core", "log"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DodCounts {
    pub(crate) owed: usize,
    pub(crate) open: usize,
    pub(crate) modified: Option<SystemTime>,
}

struct Section<'a> {
    name: Option<&'a str>,
    owed: usize,
}

pub(crate) fn dod_dir() -> Option<PathBuf> {
    super::paths::claude_config_dir().map(|dir| dir.join("dod"))
}

pub(crate) fn parse(text: &str) -> DodCounts {
    let mut open = 0;
    let mut sections = vec![Section {
        name: None,
        owed: 0,
    }];
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix(HEADING_PREFIX) {
            sections.push(Section {
                name: Some(heading_name(heading)),
                owed: 0,
            });
            continue;
        }
        let item = line.trim_start().trim_start_matches(BULLETS).trim_start();
        if item.starts_with(OPEN_MARK) {
            open += 1;
        } else if item.starts_with(HANDOFF_MARK)
            && owed_to_user(item)
            && let Some(section) = sections.last_mut()
        {
            section.owed += 1;
        }
    }
    DodCounts {
        owed: current_owed(&sections),
        open,
        modified: None,
    }
}

fn heading_name(heading: &str) -> &str {
    let end = heading
        .find(|c: char| !c.is_alphanumeric())
        .unwrap_or(heading.len());
    &heading[..end]
}

fn current_owed(sections: &[Section]) -> usize {
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
        Some(section) => section.owed,
        None => sections.iter().map(|section| section.owed).sum(),
    }
}

fn owed_to_user(item: &str) -> bool {
    if item.to_lowercase().contains(WAITING_PHRASE) {
        return false;
    }
    let Some(start) = item.find(OWNER_KEY) else {
        return false;
    };
    let rest = &item[start + OWNER_KEY.len()..];
    let end = OWNER_END
        .iter()
        .filter_map(|separator| rest.find(separator))
        .min()
        .unwrap_or(rest.len());
    let owner = rest[..end].trim();
    !owner.is_empty() && !owner.eq_ignore_ascii_case(OWN_OWNER)
}

struct Tracked {
    path: PathBuf,
    stamp: Option<FileStamp>,
    counts: DodCounts,
}

pub(crate) struct DodWatcher {
    dir: PathBuf,
    listing: Option<FileStamp>,
    files: HashMap<String, Option<Tracked>>,
}

impl DodWatcher {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            listing: None,
            files: HashMap::new(),
        }
    }

    pub(crate) fn from_env() -> Option<Self> {
        dod_dir().map(Self::new)
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
                    counts: DodCounts::default(),
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
                    .map(|text| DodCounts {
                        modified: stamp.and_then(|stamp| stamp.modified()),
                        ..parse(&text)
                    })
                    .unwrap_or_default();
            }
            if tracked.counts.owed > 0 || tracked.counts.open > 0 {
                counts.insert(id.clone(), tracked.counts);
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
        let counts = parse(file);
        assert_eq!((counts.owed, counts.open), (2, 2));
    }

    #[test]
    fn only_the_newest_amendment_counts_and_without_amendments_the_last_working_section_does() {
        const HANDOFF: &str = "- [>] x — owner: Alex — action: y — why not mine: z\n";
        let owed = |sections: &[(&str, usize)]| {
            let text: String = sections
                .iter()
                .map(|(heading, handoffs)| format!("{heading}\n{}", HANDOFF.repeat(*handoffs)))
                .collect();
            parse(&text).owed
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
    fn a_missing_directory_or_file_yields_nothing_and_a_changed_file_is_read_again() {
        let dir = temp_dir("watch");
        let mut watcher = DodWatcher::new(dir.join("dod"));
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
        let counts = watcher.poll(["abc"]).get("abc").copied().unwrap();
        assert_eq!((counts.owed, counts.open), (2, 0));
        assert_eq!(
            counts.modified,
            fs::metadata(&newer).unwrap().modified().ok()
        );
        let _ = fs::remove_dir_all(dir);
    }
}
