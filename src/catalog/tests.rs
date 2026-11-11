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

const WRAPPER: &str =
    r#"{"type":"user","cwd":"/work/app","message":{"content":"<command-name>/x</command-name>"}}"#;
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
    let tool = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
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

fn fingerprint(entry: &SessionEntry) -> (String, String, TitleSource, Option<String>, SystemTime) {
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
        r#"{"type":"session_meta","payload":{"id":"s1","cwd":"/work/other"}}"#.to_owned() + "\n",
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

fn user_line(text: &str, at: &str) -> String {
    format!(r#"{{"type":"user","timestamp":"{at}","message":{{"content":"{text}"}}}}"#)
}

fn touch(path: &Path, secs_from_now: u64) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(secs_from_now))
        .unwrap();
}

fn order_of(catalog: &Catalog) -> Vec<&str> {
    catalog
        .entries()
        .iter()
        .map(|entry| entry.session_id.as_str())
        .collect()
}

fn catalog_of(projects: &Projects, scanner: &mut Scanner) -> Catalog {
    let mut catalog = Catalog::default();
    catalog.merge(projects.files().iter().map(|f| scanner.scan(f)).collect());
    catalog
}

#[test]
fn a_session_the_owner_wrote_to_later_outranks_one_whose_file_was_only_touched_later() {
    let projects = Projects::new();
    projects.append(
        "wrote",
        &format!("{}\n", user_line("hi", "2026-11-10T12:00:00Z")),
    );
    projects.append(
        "touched",
        &format!(
            "{}\n{}\n",
            user_line("hi", "2026-11-10T09:00:00Z"),
            r#"{"type":"ai-title","aiTitle":"renamed later"}"#
        ),
    );
    touch(&projects.file("wrote"), 10);
    touch(&projects.file("touched"), 60);
    let catalog = catalog_of(&projects, &mut Scanner::default());
    assert_eq!(order_of(&catalog), ["wrote", "touched"]);
    assert_eq!(
        catalog.entries()[0].last_used,
        UNIX_EPOCH + Duration::from_secs(1_794_312_000)
    );
}

#[test]
fn only_a_genuine_user_message_sets_the_recency_time() {
    let projects = Projects::new();
    projects.append(
        "s",
        &format!(
            "{}\n{}\n{}\n{}\n{}\n",
            user_line("real", "2026-11-10T09:00:00Z"),
            r#"{"type":"user","timestamp":"2026-11-10T10:00:00Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"user","isMeta":true,"timestamp":"2026-11-10T11:00:00Z","message":{"content":"meta text"}}"#,
            user_line("<command-name>/x</command-name>", "2026-11-10T12:00:00Z"),
            r#"{"type":"user","isSidechain":true,"timestamp":"2026-11-10T13:00:00Z","message":{"content":"sub-agent prompt"}}"#,
        ),
    );
    let entry = scan_one(&mut Scanner::default(), &projects, "s");
    assert_eq!(
        entry.last_used,
        UNIX_EPOCH + Duration::from_secs(1_794_301_200)
    );
}

#[test]
fn without_a_genuine_user_message_the_file_mtime_orders_the_session() {
    let projects = Projects::new();
    projects.append("s", &format!("{WRAPPER}\n"));
    touch(&projects.file("s"), 120);
    let file = projects.files().remove(0);
    let entry = Scanner::default().scan(&file);
    assert_eq!(entry.last_used, file.modified);
}

#[test]
fn appending_a_user_message_lifts_the_session_to_the_top_on_the_next_rescan() {
    let projects = Projects::new();
    projects.append(
        "a",
        &format!("{}\n", user_line("hi", "2026-11-10T09:00:00Z")),
    );
    projects.append(
        "b",
        &format!("{}\n", user_line("hi", "2026-11-10T10:00:00Z")),
    );
    let mut scanner = Scanner::default();
    let mut catalog = catalog_of(&projects, &mut scanner);
    assert_eq!(order_of(&catalog), ["b", "a"]);

    projects.append(
        "a",
        &format!("{}\n", user_line("again", "2026-11-10T11:00:00Z")),
    );
    catalog.merge(projects.files().iter().map(|f| scanner.scan(f)).collect());
    assert_eq!(order_of(&catalog), ["a", "b"]);
}

#[test]
fn a_session_stops_asking_once_a_user_message_is_appended_to_its_transcript() {
    let projects = Projects::new();
    let question = r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Shall I continue?"}]}}"#;
    projects.append(
        "s",
        &format!("{}\n{question}\n", user_line("go", "2026-11-10T09:00:00Z")),
    );
    let mut scanner = Scanner::default();
    assert!(scan_one(&mut scanner, &projects, "s").asks_user);

    projects.append(
        "s",
        &format!("{}\n", user_line("yes", "2026-11-10T10:00:00Z")),
    );
    assert!(!scan_one(&mut scanner, &projects, "s").asks_user);
}

#[test]
fn codex_orders_by_its_last_user_message_not_by_file_mtime() {
    let home = std::env::temp_dir().join(format!("ccview-codex-order-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    let day = home.join("sessions/2026/01/01");
    fs::create_dir_all(&day).unwrap();
    let rollout = |id: &str, user_at: Option<&str>| {
        let user = user_at.map_or(String::new(), |at| {
            format!(
                r#"{{"timestamp":"{at}","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"go"}}]}}}}"#
            ) + "\n"
        });
        let output = r#"{"timestamp":"2026-01-01T23:00:00Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"x"}}"#;
        let path = day.join(format!("rollout-2026-01-01T10-00-00-{id}.jsonl"));
        fs::write(
            &path,
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"/w"}}}}"#)
                + "\n"
                + &user
                + output
                + "\n",
        )
        .unwrap();
        path
    };
    let wrote = rollout("wrote", Some("2026-01-01T12:00:00Z"));
    let touched = rollout("touched", Some("2026-01-01T09:00:00Z"));
    let silent = rollout("silent", None);
    touch(&wrote, 10);
    touch(&touched, 60);
    touch(&silent, 30);
    let mut scanner = Scanner::default();
    let mut catalog = Catalog::default();
    catalog.merge(
        list_codex_files(&home)
            .iter()
            .filter_map(|f| scanner.scan_codex(f, &HashMap::new()))
            .collect(),
    );
    assert_eq!(order_of(&catalog), ["silent", "wrote", "touched"]);
    fs::remove_dir_all(home).unwrap();
}
