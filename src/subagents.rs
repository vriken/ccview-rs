use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, SystemTime},
};

const RUNNING_WINDOW: Duration = Duration::from_secs(10 * 60);
const TAIL_BYTES: u64 = 16 * 1024;
const MAX_TAIL_BYTES: u64 = 1024 * 1024;
const PARENT_TAIL_BYTES: u64 = 1024 * 1024;
const NOTIFICATION_RECORD_TYPE: &str = "queue-operation";
const NOTIFICATION_TOOL_USE_ID: (&str, &str) = ("<tool-use-id>", "</tool-use-id>");
const NOTIFICATION_STATUS: (&str, &str) = ("<status>", "</status>");
const ENDED_STATUSES: [&str; 3] = ["completed", "failed", "killed"];
const TRANSCRIPT_PREFIX: &str = "agent-";
const TRANSCRIPT_SUFFIX: &str = ".jsonl";
const META_SUFFIX: &str = ".meta.json";
const UNKNOWN_TYPE: &str = "agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningAgent {
    pub agent_type: String,
    pub description: String,
}

#[derive(Deserialize, Default)]
struct Meta {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
    description: Option<String>,
    #[serde(rename = "toolUseId")]
    tool_use_id: Option<String>,
    #[serde(default)]
    finished: bool,
    #[serde(rename = "stoppedByUser", default)]
    stopped_by_user: bool,
}

#[derive(Deserialize)]
struct LastRecord {
    message: Option<LastMessage>,
}

#[derive(Deserialize)]
struct LastMessage {
    stop_reason: Option<String>,
}

pub fn running_in(subagents_dir: &Path, parent_transcript: Option<&Path>, now: SystemTime) -> Vec<RunningAgent> {
    let Ok(entries) = fs::read_dir(subagents_dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<(Option<String>, RunningAgent)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name
                .strip_prefix(TRANSCRIPT_PREFIX)?
                .strip_suffix(TRANSCRIPT_SUFFIX)?;
            let modified = entry.metadata().ok()?.modified().ok()?;
            if now.duration_since(modified).unwrap_or_default() > RUNNING_WINDOW {
                return None;
            }
            let meta = read_meta(&subagents_dir.join(format!("{TRANSCRIPT_PREFIX}{stem}{META_SUFFIX}")));
            if meta.finished || meta.stopped_by_user || ended_its_turn(&entry.path()) {
                return None;
            }
            let agent = RunningAgent {
                agent_type: meta.agent_type.unwrap_or_else(|| UNKNOWN_TYPE.to_owned()),
                description: meta.description.unwrap_or_default(),
            };
            Some((meta.tool_use_id, agent))
        })
        .collect();
    if let Some(parent) = parent_transcript.filter(|_| candidates.iter().any(|(id, _)| id.is_some())) {
        let ended = ended_tool_uses(parent);
        candidates.retain(|(id, _)| !id.as_ref().is_some_and(|id| ended.contains(id)));
    }
    let mut running: Vec<RunningAgent> = candidates.into_iter().map(|(_, agent)| agent).collect();
    running.sort_by(|a, b| a.description.cmp(&b.description));
    running
}

fn ended_tool_uses(parent_transcript: &Path) -> HashSet<String> {
    let Some(tail) = read_tail(parent_transcript, PARENT_TAIL_BYTES) else {
        return HashSet::new();
    };
    tail.lines()
        .filter(|line| line.contains(NOTIFICATION_RECORD_TYPE))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record.get("type").and_then(Value::as_str) == Some(NOTIFICATION_RECORD_TYPE))
        .filter_map(|record| {
            let content = record.get("content")?.as_str()?;
            let status = between(content, NOTIFICATION_STATUS)?;
            let tool_use_id = between(content, NOTIFICATION_TOOL_USE_ID)?;
            ENDED_STATUSES.contains(&status).then(|| tool_use_id.to_owned())
        })
        .collect()
}

fn between<'a>(text: &'a str, (open, close): (&str, &str)) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find(close)?;
    Some(text[start..end].trim())
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if start == 0 {
        return Some(text);
    }
    Some(text.split_once('\n')?.1.to_owned())
}

fn read_meta(path: &Path) -> Meta {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn ended_its_turn(transcript: &Path) -> bool {
    last_stop_reason(transcript).as_deref() == Some("end_turn")
}

fn last_stop_reason(transcript: &Path) -> Option<String> {
    let mut file = File::open(transcript).ok()?;
    let len = file.metadata().ok()?.len();
    let mut window = TAIL_BYTES;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::new();
        file.by_ref().take(window).read_to_end(&mut tail).ok()?;
        let text = String::from_utf8_lossy(&tail);
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let last = lines.next_back()?;
        if start > 0 && lines.next().is_none() {
            if window >= MAX_TAIL_BYTES {
                return None;
            }
            window *= 2;
            continue;
        }
        let record: LastRecord = serde_json::from_str(last).ok()?;
        return record.message?.stop_reason;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccview-sub-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn agent(dir: &Path, id: &str, meta: &str, last_stop_reason: &str) {
        fs::write(dir.join(format!("agent-{id}.meta.json")), meta).unwrap();
        let record = format!(r#"{{"type":"assistant","message":{{"stop_reason":"{last_stop_reason}"}}}}"#);
        fs::write(dir.join(format!("agent-{id}.jsonl")), format!("{{\"type\":\"user\"}}\n{record}\n")).unwrap();
    }

    #[test]
    fn only_unfinished_agents_still_working_count_as_running() {
        let dir = scratch("running");
        agent(&dir, "working", r#"{"agentType":"explore","description":"b scan"}"#, "tool_use");
        agent(&dir, "flagged", r#"{"agentType":"explore","description":"done","finished":true}"#, "tool_use");
        agent(&dir, "stopped", r#"{"stoppedByUser":true}"#, "tool_use");
        agent(&dir, "ended", r#"{"agentType":"plan","description":"wrote its answer"}"#, "end_turn");
        fs::write(dir.join("agent-nometa.jsonl"), "{\"type\":\"user\"}\n").unwrap();
        agent(&dir, "second", r#"{"agentType":"plan","description":"a plan"}"#, "tool_use");

        let running = running_in(&dir, None, SystemTime::now());
        let described: Vec<(&str, &str)> = running
            .iter()
            .map(|agent| (agent.agent_type.as_str(), agent.description.as_str()))
            .collect();
        assert_eq!(described, [("agent", ""), ("plan", "a plan"), ("explore", "b scan")]);
        fs::remove_dir_all(dir).unwrap();
    }

    fn notification(tool_use_id: &str, status: &str) -> String {
        let content = format!("<task-notification>\\n<tool-use-id>{tool_use_id}</tool-use-id>\\n<status>{status}</status>\\n</task-notification>");
        format!("{{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"content\":\"{content}\"}}\n")
    }

    #[test]
    fn an_agent_the_parent_was_notified_about_stops_running_even_if_its_meta_says_otherwise() {
        let dir = scratch("notified");
        for (id, tool_use_id) in [("a", "tu-a"), ("b", "tu-b"), ("c", "tu-c"), ("d", "tu-d")] {
            agent(&dir, id, &format!(r#"{{"agentType":"explore","description":"{id}","toolUseId":"{tool_use_id}"}}"#), "tool_use");
        }
        let parent = dir.join("parent.jsonl");
        let quoted = r#"{"type":"user","message":{"content":"<tool-use-id>tu-d</tool-use-id><status>completed</status>"}}"#;
        fs::write(
            &parent,
            format!("{quoted}\n{}{}{}", notification("tu-a", "completed"), notification("tu-b", "killed"), notification("tu-c", "running")),
        )
        .unwrap();

        let running = running_in(&dir, Some(&parent), SystemTime::now());
        let descriptions: Vec<&str> = running.iter().map(|agent| agent.description.as_str()).collect();
        assert_eq!(descriptions, ["c", "d"]);
        assert_eq!(running_in(&dir, None, SystemTime::now()).len(), 4);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_last_record_larger_than_the_first_tail_read_is_still_judged() {
        let dir = scratch("big-last");
        let big = "x".repeat(40 * 1024);
        let record = format!(r#"{{"type":"assistant","message":{{"stop_reason":"end_turn","text":"{big}"}}}}"#);
        fs::write(dir.join("agent-big.meta.json"), r#"{"agentType":"explore","description":"d"}"#).unwrap();
        fs::write(dir.join("agent-big.jsonl"), format!("{{\"type\":\"user\"}}\n{record}\n")).unwrap();
        assert!(running_in(&dir, None, SystemTime::now()).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_transcript_untouched_for_longer_than_the_window_is_an_orphan() {
        let dir = scratch("orphan");
        agent(&dir, "old", r#"{"agentType":"explore","description":"x"}"#, "tool_use");
        let later = SystemTime::now() + RUNNING_WINDOW + Duration::from_secs(60);
        assert!(running_in(&dir, None, later).is_empty());
        assert_eq!(running_in(&dir, None, SystemTime::now()).len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
