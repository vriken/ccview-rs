use crate::{
    session::{
        entry::{SessionEntry, TitleSource, TranscriptHandle},
        harness_id::HarnessId,
        lineage::Fingerprint,
        transcript::{Block, Role},
    },
    sources::{SessionFile, claude::transcript},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    time::UNIX_EPOCH,
};

const CUSTOM_TITLE_PREFIX: &[u8] = br#"{"type":"custom-title""#;
const AGENT_NAME_PREFIX: &[u8] = br#"{"type":"agent-name""#;
const AI_TITLE_PREFIX: &[u8] = br#"{"type":"ai-title""#;
const PROMPT_SEARCH_LINE_LIMIT: usize = 300;
const PROMPT_MAX_CHARS: usize = 160;
const TAIL_WINDOW_BYTES: u64 = 4096;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const SHORT_ID_CHARS: usize = 8;
const USER_TYPE_MARKER: &[u8] = br#""type":"user""#;
const TOOL_RESULT_MARKER: &[u8] = b"\"tool_result\"";
const RECENT_USER_LINES: usize = 8;
const IGNORED_PROMPT_PREFIXES: [&str; 3] = ["<", "[Request interrupted", "Caveat:"];

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    pub size: u64,
    pub mtime_ns: u64,
    pub scanned_bytes: u64,
    pub lines_scanned: usize,
    pub tail_len: u64,
    pub tail_hash: u64,
    pub meta: FileMeta,
    #[serde(default)]
    pub fingerprint: Fingerprint,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub cwd: Option<String>,
    pub custom_title: Option<String>,
    pub agent_name: Option<String>,
    pub derived_title: Option<String>,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Deserialize)]
struct CwdOnly {
    cwd: Option<String>,
}

pub(crate) fn tail_window(path: &Path, end: u64) -> Option<(u64, u64)> {
    let length = end.min(TAIL_WINDOW_BYTES);
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(end - length)).ok()?;
    let mut window = vec![0; length as usize];
    file.read_exact(&mut window).ok()?;
    let hash = window.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    });
    Some((length, hash))
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
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));
    files
}

pub(crate) fn entry_from(file: &SessionFile, meta: FileMeta) -> SessionEntry {
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
        harness: HarnessId::Claude,
        handle: TranscriptHandle::Rollout(file.path.clone()),
        path: file.path.clone(),
        session_id,
        cwd,
        title,
        title_source,
        last_prompt: meta.last_prompt.clone(),
        last_used: file.modified,
        live: None,
        fingerprint: None,
        copy_of: None,
        copies: Vec::new(),
    }
}

pub(crate) fn choose_title(meta: &FileMeta, session_id: &str) -> (String, TitleSource) {
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

pub(crate) fn is_date_only(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

pub(crate) fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(crate) fn cwd_from_project_dir(path: &Path) -> String {
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    dir_name.replace('-', "/")
}

pub(crate) fn extend_record(path: &Path, record: &mut FileRecord) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    if file.seek(SeekFrom::Start(record.scanned_bytes)).is_err() {
        return 0;
    }
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::new();
    let mut bytes_read = 0u64;
    let mut recent_user_lines: VecDeque<Vec<u8>> = VecDeque::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes_read += read as u64,
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        record.scanned_bytes += line.len() as u64;
        record.lines_scanned += 1;
        record.fingerprint.add_line(&line);
        let meta = &mut record.meta;
        if line.starts_with(CUSTOM_TITLE_PREFIX) {
            meta.custom_title = string_field(&line, "customTitle").or(meta.custom_title.take());
        } else if line.starts_with(AGENT_NAME_PREFIX) {
            meta.agent_name = string_field(&line, "agentName").or(meta.agent_name.take());
        } else if line.starts_with(AI_TITLE_PREFIX) {
            meta.derived_title = string_field(&line, "aiTitle").or(meta.derived_title.take());
        } else if record.lines_scanned <= PROMPT_SEARCH_LINE_LIMIT
            && (meta.first_prompt.is_none() || meta.cwd.is_none())
        {
            read_user_line(&line, meta);
        }
        if contains_bytes(&line, USER_TYPE_MARKER) && !contains_bytes(&line, TOOL_RESULT_MARKER) {
            if recent_user_lines.len() == RECENT_USER_LINES {
                recent_user_lines.pop_front();
            }
            recent_user_lines.push_back(line.clone());
        }
    }
    let latest_prompt = recent_user_lines.into_iter().rev().find_map(|user_line| {
        String::from_utf8(user_line)
            .ok()
            .and_then(|text| genuine_prompt(&text))
    });
    if latest_prompt.is_some() {
        record.meta.last_prompt = latest_prompt;
    }
    let (tail_len, tail_hash) = tail_window(path, record.scanned_bytes).unwrap_or_default();
    record.tail_len = tail_len;
    record.tail_hash = tail_hash;
    bytes_read + tail_len
}

pub(crate) fn string_field(line: &[u8], field: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    value.get(field)?.as_str().map(str::to_owned)
}

pub(crate) fn read_user_line(line: &[u8], meta: &mut FileMeta) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    if !text.contains(r#""type":"user""#) {
        return;
    }
    if meta.cwd.is_none() {
        meta.cwd = serde_json::from_str::<CwdOnly>(text)
            .ok()
            .and_then(|c| c.cwd);
    }
    if meta.first_prompt.is_none() {
        meta.first_prompt = genuine_prompt(text);
    }
}

pub(crate) fn genuine_prompt(line: &str) -> Option<String> {
    let message = transcript::parse_line(line).ok()??;
    if message.role != Role::User || message.is_meta {
        return None;
    }
    let text = message.blocks.into_iter().find_map(|block| match block {
        Block::Text(text) if is_genuine_prompt(&text) => Some(text),
        _ => None,
    })?;
    Some(single_line(&text, PROMPT_MAX_CHARS))
}

pub(crate) fn is_genuine_prompt(text: &str) -> bool {
    let trimmed = text.trim_start();
    !trimmed.is_empty()
        && !IGNORED_PROMPT_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
}

pub(crate) fn single_line(text: &str, max_chars: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let kept: String = joined.chars().take(max_chars).collect();
    format!("{kept}…")
}
