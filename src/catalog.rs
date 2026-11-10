pub mod feed;
pub mod index;
pub mod live;
pub mod scanner;
pub mod worker;

use crate::{
    session::{
        activity::ActivityMap,
        entry::SessionEntry,
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
pub struct Catalog {
    entries: Vec<SessionEntry>,
    live: Vec<LiveSession>,
    raw_activity: ActivityMap,
    activity: ActivityMap,
    expanded: bool,
    groups: Vec<lineage::Group>,
    dismissed: Dismissed,
    live_seen: bool,
    activity_seen: bool,
}

impl Catalog {
    pub fn activity(&self) -> &ActivityMap {
        &self.activity
    }

    pub fn dismissed(&self) -> &Dismissed {
        &self.dismissed
    }

    pub fn set_dismissed(&mut self, dismissed: Dismissed) {
        self.dismissed = dismissed;
        self.derive_activity();
    }

    pub fn dismiss(&mut self, session_id: &str, pid: Option<u32>) -> std::io::Result<()> {
        let saved = self.dismissed.dismiss(session_id, pid);
        self.derive_activity();
        saved
    }

    pub fn undismiss(&mut self, session_id: &str) -> std::io::Result<()> {
        let saved = self.dismissed.undismiss(session_id);
        self.derive_activity();
        saved
    }

    pub fn set_activity(&mut self, activity: ActivityMap) {
        self.raw_activity = activity;
        self.activity_seen = true;
        self.derive_activity();
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub fn live(&self) -> &[LiveSession] {
        &self.live
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.apply_groups();
    }

    pub fn is_collapsed_copy(&self, entry: &SessionEntry) -> bool {
        !self.expanded && entry.copy_of.is_some()
    }

    pub fn merge(&mut self, scanned: Vec<SessionEntry>) {
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

    pub fn remove(&mut self, paths: &[PathBuf]) {
        let gone: HashSet<&PathBuf> = paths.iter().collect();
        self.entries.retain(|entry| !gone.contains(&entry.path));
        self.regroup();
    }

    pub fn set_live(&mut self, live: Vec<LiveSession>) {
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
    }
}

fn live_for(live: &[LiveSession], session_id: &str) -> Option<LiveSession> {
    live.iter()
        .find(|session| session.session_id.as_deref() == Some(session_id))
        .cloned()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::{
        catalog::{
            feed::{Feed, Update},
            index::IndexCache,
            scanner::{CLAUDE_SOURCE, CODEX_SOURCE, Scanner},
            worker::{CodexWatch, scan_codex},
        },
        sources::{
            SessionFile, SourceEntry,
            claude::scan::{FileMeta, FileRecord, extend_record, list_files},
            codex::list_codex_files,
        },
    };
    use crate::{
        session::{
            entry::{TitleSource, TranscriptHandle},
            harness_id::HarnessId,
        },
        sources::claude::scan::{choose_title, cwd_from_project_dir, is_date_only},
    };
    use std::fs::{self, File};
    use std::{
        path::Path,
        sync::{
            Arc,
            mpsc::{self, Receiver},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };

    fn meta_of(lines: &[&str]) -> FileMeta {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ccview-lib-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let mut record = FileRecord::default();
        extend_record(&path, &mut record);
        fs::remove_file(&path).unwrap();
        record.meta
    }

    const WRAPPER: &str = r#"{"type":"user","cwd":"/work/app","message":{"content":"<command-name>/x</command-name>"}}"#;
    const PROMPT: &str =
        r#"{"type":"user","cwd":"/work/app","message":{"content":"fix   the\nlogin bug"}}"#;

    #[test]
    fn prompt_skips_wrappers_and_collapses_whitespace() {
        let meta = meta_of(&[WRAPPER, PROMPT]);
        assert_eq!(meta.first_prompt.as_deref(), Some("fix the login bug"));
        assert_eq!(meta.cwd.as_deref(), Some("/work/app"));
    }

    #[test]
    fn last_title_wins_and_priority_is_custom_agent_derived_prompt_id() {
        let derived_old = r#"{"type":"ai-title","aiTitle":"old"}"#;
        let derived_new = r#"{"type":"ai-title","aiTitle":"new"}"#;
        let meta = meta_of(&[PROMPT, derived_old, derived_new]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "new");

        let named = r#"{"type":"agent-name","agentName":"agent"}"#;
        let meta = meta_of(&[PROMPT, derived_new, named]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "agent");

        let custom = r#"{"type":"custom-title","customTitle":"mine"}"#;
        let meta = meta_of(&[PROMPT, derived_new, named, custom]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "mine");

        let meta = meta_of(&[PROMPT]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "fix the login bug");

        let meta = meta_of(&[WRAPPER, r#"{"type":"mode"}"#]);
        assert_eq!(
            choose_title(&meta, "abcdef123456"),
            ("abcdef12".to_owned(), TitleSource::SessionId)
        );
    }

    #[test]
    fn date_only_titles_fall_through_to_the_first_prompt() {
        let date_title = r#"{"type":"ai-title","aiTitle":"2026-11-10"}"#;
        let meta = meta_of(&[PROMPT, date_title]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "fix the login bug");
        let blank = r#"{"type":"ai-title","aiTitle":"  "}"#;
        let meta = meta_of(&[PROMPT, blank]);
        assert_eq!(choose_title(&meta, "abcdef123456").0, "fix the login bug");
        assert!(
            is_date_only("2026-11-10")
                && !is_date_only("2026-11-10 notes")
                && !is_date_only("2026-1-10")
        );
    }

    #[test]
    fn a_trailing_interrupt_marker_does_not_erase_the_last_real_prompt() {
        let later = r#"{"type":"user","message":{"content":"now add tests"}}"#;
        let interrupt = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        let meta = meta_of(&[PROMPT, later, interrupt]);
        assert_eq!(meta.last_prompt.as_deref(), Some("now add tests"));
    }

    #[test]
    fn last_prompt_is_the_final_genuine_user_message_not_a_tool_result() {
        let later = r#"{"type":"user","message":{"content":"now add tests"}}"#;
        let tool =
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        let meta = meta_of(&[PROMPT, later, tool]);
        assert_eq!(meta.last_prompt.as_deref(), Some("now add tests"));
    }

    pub(super) struct Projects(PathBuf);

    impl Projects {
        pub(super) fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "ccview-proj-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("-w-app")).unwrap();
            Self(root)
        }

        pub(super) fn file(&self, name: &str) -> PathBuf {
            self.0.join("-w-app").join(format!("{name}.jsonl"))
        }

        pub(super) fn append(&self, name: &str, text: &str) {
            use std::io::Write;
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.file(name))
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }

        pub(super) fn files(&self) -> Vec<SessionFile> {
            list_files(&self.0)
        }
    }

    impl Drop for Projects {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn scan_one(scanner: &mut Scanner, projects: &Projects, name: &str) -> SessionEntry {
        let path = projects.file(name);
        let file = projects
            .files()
            .into_iter()
            .find(|f| f.path == path)
            .unwrap();
        scanner.scan(&file)
    }

    fn fingerprint(
        entry: &SessionEntry,
    ) -> (String, String, TitleSource, Option<String>, SystemTime) {
        (
            entry.cwd.clone(),
            entry.title.clone(),
            entry.title_source,
            entry.last_prompt.clone(),
            entry.last_used,
        )
    }

    #[test]
    fn appended_lines_merge_without_rereading_the_prefix_and_equal_a_full_rescan() {
        let projects = Projects::new();
        let padding = format!(
            r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#,
            "x".repeat(20000)
        );
        projects.append(
            "s",
            &format!(
                "{PROMPT}\n{padding}\n{}\n",
                r#"{"type":"ai-title","aiTitle":"old"}"#
            ),
        );
        let mut incremental = Scanner::default();
        scan_one(&mut incremental, &projects, "s");
        let first_read = incremental.bytes_read;

        projects.append(
            "s",
            &format!(
                "{}\n{}\n",
                r#"{"type":"custom-title","customTitle":"renamed"}"#,
                r#"{"type":"user","message":{"content":"and then this"}}"#
            ),
        );
        let merged = scan_one(&mut incremental, &projects, "s");
        let full = scan_one(&mut Scanner::default(), &projects, "s");

        assert_eq!(fingerprint(&merged), fingerprint(&full));
        assert_eq!(merged.title, "renamed");
        assert_eq!(merged.last_prompt.as_deref(), Some("and then this"));
        assert!(
            incremental.bytes_read - first_read < 10_000,
            "re-read the prefix"
        );
    }

    #[test]
    fn partial_last_line_is_not_consumed_until_it_is_complete() {
        let projects = Projects::new();
        projects.append("s", &format!("{PROMPT}\n"));
        projects.append("s", r#"{"type":"custom-title","customTi"#);
        let mut scanner = Scanner::default();
        assert_eq!(
            scan_one(&mut scanner, &projects, "s").title,
            "fix the login bug"
        );

        projects.append("s", "tle\":\"done\"}\n");
        assert_eq!(scan_one(&mut scanner, &projects, "s").title, "done");
    }

    #[test]
    fn rewritten_or_shrunk_files_are_rescanned_in_full() {
        let projects = Projects::new();
        projects.append("s", &format!("{PROMPT}\n"));
        let mut scanner = Scanner::default();
        scan_one(&mut scanner, &projects, "s");

        fs::write(
            projects.file("s"),
            format!(
                "{}\n{}\n",
                r#"{"type":"user","message":{"content":"totally different start"}}"#,
                r#"{"type":"mode"}"#
            ),
        )
        .unwrap();
        assert_eq!(
            scan_one(&mut scanner, &projects, "s").title,
            "totally different start"
        );
    }

    #[test]
    fn same_length_rewrite_with_new_mtime_is_not_trusted() {
        let projects = Projects::new();
        projects.append("s", &format!("{PROMPT}\n"));
        let mut scanner = Scanner::default();
        scan_one(&mut scanner, &projects, "s");
        let rewritten = format!("{PROMPT}\n").replace("login", "logon");
        fs::write(projects.file("s"), rewritten).unwrap();
        let file = File::options()
            .write(true)
            .open(projects.file("s"))
            .unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            scan_one(&mut scanner, &projects, "s").title,
            "fix the logon bug"
        );
    }

    #[test]
    fn persisted_index_serves_unchanged_files_without_reading_and_orders_by_fresh_mtime() {
        let projects = Projects::new();
        projects.append("old", &format!("{PROMPT}\n"));
        projects.append("new", &format!("{PROMPT}\n"));
        let index_path = projects.0.join("index/index-v1.json");
        let mut first = Scanner::with_index(IndexCache::load(index_path.clone()));
        for file in projects.files() {
            first.scan(&file);
        }
        first.persist();

        let touched = File::options()
            .write(true)
            .open(projects.file("old"))
            .unwrap();
        touched
            .set_modified(SystemTime::now() + Duration::from_secs(60))
            .unwrap();
        projects.append("other", &format!("{PROMPT}\n"));

        let mut second = Scanner::with_index(IndexCache::load(index_path));
        assert_eq!(second.cached_entries().len(), 2);
        let files = projects.files();
        assert!(
            second.is_fresh(
                CLAUDE_SOURCE,
                files
                    .iter()
                    .find(|f| f.path == projects.file("new"))
                    .unwrap()
            )
        );
        let mut catalog = Catalog::default();
        catalog.merge(second.cached_entries());
        catalog.merge(files.iter().map(|file| second.scan(file)).collect());

        let order: Vec<&str> = catalog
            .entries()
            .iter()
            .map(|e| e.session_id.as_str())
            .collect();
        assert_eq!(order[0], "old");
        assert_eq!(order.len(), 3);
        let before = second.bytes_read;
        scan_one(&mut second, &projects, "new");
        assert_eq!(second.bytes_read, before, "reread an unchanged file");
    }

    #[test]
    fn deleted_files_leave_the_catalog_and_the_index() {
        let projects = Projects::new();
        projects.append("a", &format!("{PROMPT}\n"));
        projects.append("b", &format!("{PROMPT}\n"));
        let index_path = projects.0.join("index/index-v1.json");
        let mut scanner = Scanner::with_index(IndexCache::load(index_path.clone()));
        let mut catalog = Catalog::default();
        catalog.merge(
            projects
                .files()
                .iter()
                .map(|file| scanner.scan(file))
                .collect(),
        );
        scanner.persist();

        fs::remove_file(projects.file("a")).unwrap();
        let removed = scanner.forget_missing(CLAUDE_SOURCE, &projects.files());
        catalog.remove(&removed);
        scanner.persist();

        assert_eq!(catalog.entries().len(), 1);
        assert_eq!(
            Scanner::with_index(IndexCache::load(index_path))
                .cached_entries()
                .len(),
            1
        );
    }

    fn source_entry(harness: HarnessId, id: &str, modified_secs: u64) -> SessionEntry {
        let handle = match harness {
            HarnessId::OpenCode => TranscriptHandle::Database {
                path: PathBuf::from("/data/opencode.db"),
                session_id: id.to_owned(),
            },
            _ => TranscriptHandle::Rollout(PathBuf::from(format!("/data/{id}.jsonl"))),
        };
        SourceEntry {
            harness,
            session_id: id.to_owned(),
            cwd: "/w".to_owned(),
            title: id.to_owned(),
            title_source: TitleSource::Prompt,
            last_used: UNIX_EPOCH + Duration::from_secs(modified_secs),
            handle,
        }
        .into()
    }

    #[test]
    fn entries_of_every_harness_share_one_list_ordered_by_last_used() {
        let mut catalog = Catalog::default();
        catalog.merge(vec![source_entry(HarnessId::Claude, "claude-old", 10)]);
        catalog.merge(vec![
            source_entry(HarnessId::OpenCode, "opencode-newest", 30),
            source_entry(HarnessId::Codex, "codex-mid", 20),
        ]);
        catalog.merge(vec![source_entry(HarnessId::Claude, "claude-old", 40)]);

        let order: Vec<(&str, HarnessId)> = catalog
            .entries()
            .iter()
            .map(|entry| (entry.session_id.as_str(), entry.harness))
            .collect();
        assert_eq!(
            order,
            [
                ("claude-old", HarnessId::Claude),
                ("opencode-newest", HarnessId::OpenCode),
                ("codex-mid", HarnessId::Codex),
            ]
        );
    }

    #[test]
    fn opencode_sessions_in_one_database_have_distinct_entry_keys() {
        let first = source_entry(HarnessId::OpenCode, "ses_a", 1);
        let second = source_entry(HarnessId::OpenCode, "ses_b", 1);
        assert_ne!(first.path, second.path);
        let mut catalog = Catalog::default();
        catalog.merge(vec![first, second]);
        assert_eq!(catalog.entries().len(), 2);
    }

    #[test]
    fn codex_rollouts_come_from_the_index_until_they_change_and_names_apply_live() {
        let home = std::env::temp_dir().join(format!("ccview-codex-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/01/01");
        fs::create_dir_all(&day).unwrap();
        let rollout = day.join("rollout-2026-01-01T10-00-00-sess-1.jsonl");
        fs::write(
            &rollout,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"sess-1","cwd":"/work/app"}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the build"}]}}"#, "\n",
            ),
        )
        .unwrap();
        let index_path = home.join("index/index-v1.json");

        let mut first = Scanner::with_index(IndexCache::load(index_path.clone()));
        let file = list_codex_files(&home).remove(0);
        let entry = first.scan_codex(&file, &HashMap::new()).unwrap();
        assert_eq!(
            (entry.harness, entry.session_id.as_str(), entry.cwd.as_str()),
            (HarnessId::Codex, "sess-1", "/work/app")
        );
        assert_eq!(entry.title, "fix the build");
        first.persist();

        fs::remove_file(&rollout).unwrap();
        let second = Scanner::with_index(IndexCache::load(index_path));
        let names = HashMap::from([("sess-1".to_owned(), "renamed".to_owned())]);
        let cached = second.cached_codex_entries(&names);
        assert_eq!(cached.len(), 1);
        assert_eq!(
            (cached[0].title.as_str(), cached[0].title_source),
            ("renamed", TitleSource::Agent)
        );
        assert!(second.is_fresh(CODEX_SOURCE, &file));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_rescans_resend_only_changed_rollouts_or_all_when_names_change() {
        let home = std::env::temp_dir().join(format!("ccview-codex-resend-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/01/01");
        fs::create_dir_all(&day).unwrap();
        for id in ["s1", "s2"] {
            fs::write(
                day.join(format!("rollout-2026-01-01T10-00-00-{id}.jsonl")),
                format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"/work/app"}}}}"#)
                    + "\n",
            )
            .unwrap();
        }
        let (sender, receiver) = mpsc::channel();
        let feed = Feed {
            sender,
            wake: Arc::new(|| {}),
        };
        let mut watch = CodexWatch::default();
        let mut scanner = Scanner::default();
        let sent = |receiver: &Receiver<Update>| -> usize {
            receiver
                .try_iter()
                .map(|update| match update {
                    Update::Scanned(entries) => entries.len(),
                    _ => 0,
                })
                .sum()
        };
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 2);
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 0);
        fs::write(
            day.join("rollout-2026-01-01T10-00-00-s1.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"s1","cwd":"/work/other"}}"#.to_owned()
                + "\n",
        )
        .unwrap();
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 1);
        fs::write(
            home.join("session_index.jsonl"),
            r#"{"id":"s2","thread_name":"renamed"}"#.to_owned() + "\n",
        )
        .unwrap();
        assert!(scan_codex(&mut scanner, &feed, &home, &mut watch));
        assert_eq!(sent(&receiver), 2);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn merging_thousands_of_entries_again_stays_fast() {
        let entries = |n: usize| -> Vec<SessionEntry> {
            (0..n)
                .map(|i| source_entry(HarnessId::Codex, &format!("s{i}"), i as u64))
                .collect()
        };
        let mut catalog = Catalog::default();
        catalog.merge(entries(5000));
        let started = Instant::now();
        catalog.merge(entries(5000));
        assert_eq!(catalog.entries().len(), 5000);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn cwd_falls_back_to_decoded_directory_name() {
        let path = Path::new("/p/-Users-me-app/abc.jsonl");
        assert_eq!(cwd_from_project_dir(path), "/Users/me/app");
    }
}

#[cfg(test)]
mod lineage_tests;
