use crate::{
    activity::{self, ActivityMap},
    parse::{self, Block, Role},
    registry::{self, LiveSession},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CUSTOM_TITLE_PREFIX: &[u8] = br#"{"type":"custom-title""#;
const AGENT_NAME_PREFIX: &[u8] = br#"{"type":"agent-name""#;
const AI_TITLE_PREFIX: &[u8] = br#"{"type":"ai-title""#;
const PROMPT_SEARCH_LINE_LIMIT: usize = 300;
const PROMPT_MAX_CHARS: usize = 160;
const SHORT_ID_CHARS: usize = 8;
const SCAN_BATCH: usize = 16;
const LIVE_REFRESH: Duration = Duration::from_secs(2);
const RESCAN_REFRESH: Duration = Duration::from_secs(5);
const USER_TYPE_MARKER: &[u8] = br#""type":"user""#;
const TOOL_RESULT_MARKER: &[u8] = b"\"tool_result\"";
const IGNORED_PROMPT_PREFIXES: [&str; 3] = ["<", "[Request interrupted", "Caveat:"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSource {
    Custom,
    Agent,
    Derived,
    Prompt,
    SessionId,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub title_source: TitleSource,
    pub last_prompt: Option<String>,
    pub last_used: SystemTime,
    pub live: Option<LiveSession>,
}

pub enum Update {
    Scanned(Vec<SessionEntry>),
    InitialScanDone,
    Live(Vec<LiveSession>),
    Activity(ActivityMap),
}

#[derive(Default)]
pub struct Library {
    entries: Vec<SessionEntry>,
    live: Vec<LiveSession>,
    activity: ActivityMap,
}

impl Library {
    pub fn activity(&self) -> &ActivityMap {
        &self.activity
    }

    pub fn set_activity(&mut self, activity: ActivityMap) {
        self.activity = activity;
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub fn live(&self) -> &[LiveSession] {
        &self.live
    }

    pub fn merge(&mut self, scanned: Vec<SessionEntry>) {
        for mut entry in scanned {
            entry.live = live_for(&self.live, &entry.session_id);
            match self.entries.iter_mut().find(|e| e.path == entry.path) {
                Some(existing) => *existing = entry,
                None => self.entries.push(entry),
            }
        }
        self.entries
            .sort_by(|a, b| b.last_used.cmp(&a.last_used).then_with(|| a.path.cmp(&b.path)));
    }

    pub fn set_live(&mut self, live: Vec<LiveSession>) {
        for entry in &mut self.entries {
            entry.live = live_for(&live, &entry.session_id);
        }
        self.live = live;
    }
}

fn live_for(live: &[LiveSession], session_id: &str) -> Option<LiveSession> {
    live.iter()
        .find(|session| session.session_id.as_deref() == Some(session_id))
        .cloned()
}

#[derive(Default, Clone)]
struct FileMeta {
    cwd: Option<String>,
    custom_title: Option<String>,
    agent_name: Option<String>,
    derived_title: Option<String>,
    first_prompt: Option<String>,
    last_prompt: Option<String>,
}

#[derive(Deserialize)]
struct CwdOnly {
    cwd: Option<String>,
}

#[derive(Default)]
pub struct Scanner {
    cache: HashMap<PathBuf, (SystemTime, u64, FileMeta)>,
}

impl Scanner {
    pub fn scan(&mut self, file: &SessionFile) -> SessionEntry {
        let cached = self
            .cache
            .get(&file.path)
            .filter(|(modified, len, _)| *modified == file.modified && *len == file.len);
        let meta = match cached {
            Some((_, _, meta)) => meta.clone(),
            None => {
                let meta = read_meta(&file.path);
                self.cache
                    .insert(file.path.clone(), (file.modified, file.len, meta.clone()));
                meta
            }
        };
        entry_from(file, meta)
    }

    pub fn is_fresh(&self, file: &SessionFile) -> bool {
        self.cache
            .get(&file.path)
            .is_some_and(|(modified, len, _)| *modified == file.modified && *len == file.len)
    }
}

#[derive(Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub len: u64,
}

pub fn list_files(projects_dir: &Path) -> Vec<SessionFile> {
    let mut files = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return files;
    };
    for project in projects.flatten() {
        let Ok(children) = fs::read_dir(project.path()) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(metadata) = child.metadata() else {
                continue;
            };
            files.push(SessionFile {
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                len: metadata.len(),
                path,
            });
        }
    }
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    files
}

pub fn scan_all(projects_dir: &Path) -> Vec<SessionEntry> {
    let mut scanner = Scanner::default();
    list_files(projects_dir)
        .iter()
        .map(|file| scanner.scan(file))
        .collect()
}

pub fn spawn(projects_dir: PathBuf) -> Receiver<Update> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut scanner = Scanner::default();
        let mut first_pass = true;
        if !send_live(&sender, &projects_dir) {
            return;
        }
        loop {
            let files = list_files(&projects_dir);
            let stale: Vec<&SessionFile> =
                files.iter().filter(|file| !scanner.is_fresh(file)).collect();
            for chunk in stale.chunks(SCAN_BATCH) {
                let entries = chunk.iter().map(|file| scanner.scan(file)).collect();
                if sender.send(Update::Scanned(entries)).is_err() {
                    return;
                }
            }
            if first_pass {
                first_pass = false;
                if sender.send(Update::InitialScanDone).is_err() {
                    return;
                }
            }
            let rescan_at = Instant::now() + RESCAN_REFRESH;
            while Instant::now() < rescan_at {
                thread::sleep(LIVE_REFRESH);
                if !send_live(&sender, &projects_dir) {
                    return;
                }
            }
        }
    });
    receiver
}

fn send_live(sender: &mpsc::Sender<Update>, projects_dir: &Path) -> bool {
    let live = registry::live_sessions();
    let activity_input = live.clone();
    if sender.send(Update::Live(live)).is_err() {
        return false;
    }
    sender
        .send(Update::Activity(activity::poll(projects_dir, &activity_input)))
        .is_ok()
}

fn entry_from(file: &SessionFile, meta: FileMeta) -> SessionEntry {
    let session_id = file
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    let (title, title_source) = choose_title(&meta, &session_id);
    let cwd = meta
        .cwd
        .clone()
        .unwrap_or_else(|| cwd_from_project_dir(&file.path));
    SessionEntry {
        path: file.path.clone(),
        session_id,
        cwd,
        title,
        title_source,
        last_prompt: meta.last_prompt.clone(),
        last_used: file.modified,
        live: None,
    }
}

fn choose_title(meta: &FileMeta, session_id: &str) -> (String, TitleSource) {
    let candidates = [
        (&meta.custom_title, TitleSource::Custom),
        (&meta.agent_name, TitleSource::Agent),
        (&meta.derived_title, TitleSource::Derived),
        (&meta.first_prompt, TitleSource::Prompt),
    ];
    for (candidate, source) in candidates {
        if let Some(title) = candidate
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty() && !is_date_only(t))
        {
            return (title.to_owned(), source);
        }
    }
    let short_id: String = session_id.chars().take(SHORT_ID_CHARS).collect();
    (short_id, TitleSource::SessionId)
}

fn is_date_only(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn cwd_from_project_dir(path: &Path) -> String {
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    dir_name.replace('-', "/")
}

fn read_meta(path: &Path) -> FileMeta {
    let mut meta = FileMeta::default();
    let Ok(file) = File::open(path) else {
        return meta;
    };
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::new();
    let mut line_number = 0usize;
    let mut last_user_line: Option<Vec<u8>> = None;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        line_number += 1;
        if line.starts_with(CUSTOM_TITLE_PREFIX) {
            meta.custom_title = string_field(&line, "customTitle").or(meta.custom_title);
        } else if line.starts_with(AGENT_NAME_PREFIX) {
            meta.agent_name = string_field(&line, "agentName").or(meta.agent_name);
        } else if line.starts_with(AI_TITLE_PREFIX) {
            meta.derived_title = string_field(&line, "aiTitle").or(meta.derived_title);
        } else if line_number <= PROMPT_SEARCH_LINE_LIMIT
            && (meta.first_prompt.is_none() || meta.cwd.is_none())
        {
            read_user_line(&line, &mut meta);
        }
        if contains_bytes(&line, USER_TYPE_MARKER) && !contains_bytes(&line, TOOL_RESULT_MARKER) {
            last_user_line = Some(line.clone());
        }
    }
    meta.last_prompt = last_user_line
        .and_then(|line| String::from_utf8(line).ok())
        .and_then(|text| genuine_prompt(&text));
    meta
}

fn string_field(line: &[u8], field: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    value.get(field)?.as_str().map(str::to_owned)
}

fn read_user_line(line: &[u8], meta: &mut FileMeta) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    if !text.contains(r#""type":"user""#) {
        return;
    }
    if meta.cwd.is_none() {
        meta.cwd = serde_json::from_str::<CwdOnly>(text).ok().and_then(|c| c.cwd);
    }
    if meta.first_prompt.is_none() {
        meta.first_prompt = genuine_prompt(text);
    }
}

fn genuine_prompt(line: &str) -> Option<String> {
    let message = parse::parse_line(line).ok()??;
    if message.role != Role::User || message.is_meta {
        return None;
    }
    let text = message.blocks.into_iter().find_map(|block| match block {
        Block::Text(text) if is_genuine_prompt(&text) => Some(text),
        _ => None,
    })?;
    Some(single_line(&text, PROMPT_MAX_CHARS))
}

fn is_genuine_prompt(text: &str) -> bool {
    let trimmed = text.trim_start();
    !trimmed.is_empty()
        && !IGNORED_PROMPT_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
}

fn single_line(text: &str, max_chars: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let kept: String = joined.chars().take(max_chars).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn meta_of(lines: &[&str]) -> FileMeta {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ccview-lib-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, lines.join("\n")).unwrap();
        let meta = read_meta(&path);
        fs::remove_file(&path).unwrap();
        meta
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
        assert!(is_date_only("2026-11-10") && !is_date_only("2026-11-10 notes") && !is_date_only("2026-1-10"));
    }

    #[test]
    fn last_prompt_is_the_final_genuine_user_message_not_a_tool_result() {
        let later = r#"{"type":"user","message":{"content":"now add tests"}}"#;
        let tool = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        let meta = meta_of(&[PROMPT, later, tool]);
        assert_eq!(meta.last_prompt.as_deref(), Some("now add tests"));
    }

    #[test]
    fn cwd_falls_back_to_decoded_directory_name() {
        let path = Path::new("/p/-Users-me-app/abc.jsonl");
        assert_eq!(cwd_from_project_dir(path), "/Users/me/app");
    }
}
