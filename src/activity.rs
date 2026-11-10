use crate::{
    agents_cli::CliSession,
    registry::{self, LiveSession, Status},
    subagents::{self, RunningAgent},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const BACKGROUND_KINDS: [&str; 2] = ["background", "bg"];
const SHELL_LABEL: &str = "shell";
const WAITING_FOR_USER_LABEL: &str = "waiting";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityState {
    Busy,
    Idle,
    Blocked,
    Shell,
    Other(String),
    Unknown,
}

impl ActivityState {
    pub fn label(&self) -> &str {
        match self {
            ActivityState::Busy => "busy",
            ActivityState::Idle => "idle",
            ActivityState::Blocked => "blocked",
            ActivityState::Shell => "shell",
            ActivityState::Other(label) => label,
            ActivityState::Unknown => "?",
        }
    }

    fn precedence(&self) -> u8 {
        match self {
            ActivityState::Blocked => 5,
            ActivityState::Shell => 4,
            ActivityState::Busy => 3,
            ActivityState::Other(_) => 2,
            ActivityState::Idle => 1,
            ActivityState::Unknown => 0,
        }
    }

    fn from_label(label: &str) -> Self {
        match label {
            "busy" => ActivityState::Busy,
            "idle" => ActivityState::Idle,
            "blocked" | WAITING_FOR_USER_LABEL => ActivityState::Blocked,
            SHELL_LABEL => ActivityState::Shell,
            other => ActivityState::Other(other.to_owned()),
        }
    }

    fn from_registry(status: &Status) -> Self {
        match status {
            Status::Busy => ActivityState::Busy,
            Status::Idle => ActivityState::Idle,
            Status::Other(label) => ActivityState::from_label(label),
            Status::Unknown => ActivityState::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub state: ActivityState,
    pub background: bool,
    pub attach_id: Option<String>,
    pub agents: Vec<RunningAgent>,
    pub last_activity: Option<SystemTime>,
}

impl Activity {
    fn new(state: ActivityState) -> Self {
        Self {
            state,
            background: false,
            attach_id: None,
            agents: Vec::new(),
            last_activity: None,
        }
    }

    pub fn needs_you(&self) -> bool {
        self.state == ActivityState::Blocked
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, ActivityState::Busy | ActivityState::Blocked) || !self.agents.is_empty()
    }

    fn absorb(&mut self, state: ActivityState) {
        if state.precedence() > self.state.precedence() {
            self.state = state;
        }
    }
}

pub type ActivityMap = HashMap<String, Activity>;

pub fn merge(registry: &[LiveSession], cli: &[CliSession]) -> ActivityMap {
    let mut map = ActivityMap::new();
    let registry_ids: HashSet<&str> = registry
        .iter()
        .filter_map(|session| session.session_id.as_deref())
        .collect();
    for session in registry {
        let Some(session_id) = &session.session_id else {
            continue;
        };
        let activity = entry_for(&mut map, session_id, ActivityState::from_registry(&session.status));
        activity.absorb(ActivityState::from_registry(&session.status));
        activity.background |= session.kind.as_deref().is_some_and(is_background);
        let seen = session
            .status_updated_at_ms
            .or(session.updated_at_ms)
            .map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
        activity.last_activity = activity.last_activity.max(seen);
    }
    for session in cli {
        let reported = session
            .state
            .as_deref()
            .or(session.status.as_deref())
            .map_or(ActivityState::Unknown, ActivityState::from_label);
        let background = session.kind.as_deref().is_some_and(is_background);
        let activity = entry_for(&mut map, &session.session_id, reported.clone());
        if background || !registry_ids.contains(session.session_id.as_str()) {
            activity.absorb(reported);
        }
        activity.background |= background;
        if activity.attach_id.is_none() {
            activity.attach_id = session.id.clone();
        }
    }
    map
}

fn entry_for<'a>(map: &'a mut ActivityMap, session_id: &str, initial: ActivityState) -> &'a mut Activity {
    map.entry(session_id.to_owned())
        .or_insert_with(|| Activity::new(initial))
}

fn is_background(kind: &str) -> bool {
    BACKGROUND_KINDS.contains(&kind)
}

pub struct Tracker {
    projects_dir: PathBuf,
    transcripts: HashMap<String, PathBuf>,
}

impl Tracker {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            transcripts: HashMap::new(),
        }
    }

    pub fn compute(&mut self, live: &[LiveSession], cli: &[CliSession], now: SystemTime) -> ActivityMap {
        let mut map = merge(live, cli);
        for (session_id, activity) in &mut map {
            if let Some(transcript) = self.transcript_of(live, session_id) {
                activity.agents = subagents::running_in(&subagents_dir(&transcript), Some(&transcript), now);
            }
        }
        map
    }

    fn transcript_of(&mut self, live: &[LiveSession], session_id: &str) -> Option<PathBuf> {
        let known = live
            .iter()
            .find(|session| session.session_id.as_deref() == Some(session_id))
            .and_then(|session| session.transcript.clone())
            .or_else(|| self.transcripts.get(session_id).cloned());
        if known.is_some() {
            return known;
        }
        let found = registry::find_transcript(&self.projects_dir, session_id)?;
        self.transcripts.insert(session_id.to_owned(), found.clone());
        Some(found)
    }
}

pub fn subagents_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub busy: usize,
    pub needs_you: usize,
    pub agents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub attention: bool,
}

pub fn summarize<'a>(activities: impl IntoIterator<Item = &'a Activity>) -> Summary {
    let mut summary = Summary::default();
    for activity in activities {
        match activity.state {
            ActivityState::Busy => summary.busy += 1,
            ActivityState::Blocked => summary.needs_you += 1,
            _ => {}
        }
        summary.agents += activity.agents.len();
    }
    summary
}

impl Summary {
    pub fn segments(&self) -> Vec<Segment> {
        let segment = |count: usize, noun: &str, attention: bool| {
            (count > 0).then(|| Segment {
                text: format!("{count} {noun}"),
                attention,
            })
        };
        [
            segment(self.busy, "busy", false),
            segment(self.needs_you, "needs you", true),
            segment(self.agents, "agents running", false),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Badge {
    NeedsYou,
    Agents(usize),
    Background,
    Shell,
}

impl Badge {
    pub fn text(&self) -> String {
        match self {
            Badge::NeedsYou => "needs you".to_owned(),
            Badge::Agents(count) => format!("⛭ {count}"),
            Badge::Background => "bg".to_owned(),
            Badge::Shell => "sh".to_owned(),
        }
    }
}

pub fn badges(activity: &Activity) -> Vec<Badge> {
    let mut badges = Vec::new();
    if activity.needs_you() {
        badges.push(Badge::NeedsYou);
    }
    if !activity.agents.is_empty() {
        badges.push(Badge::Agents(activity.agents.len()));
    }
    if activity.background {
        badges.push(Badge::Background);
    }
    if activity.state == ActivityState::Shell {
        badges.push(Badge::Shell);
    }
    badges
}

pub fn attention_rank(activity: Option<&Activity>) -> u8 {
    u8::from(!activity.is_some_and(Activity::needs_you))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(session_id: &str, status: Status, kind: &str) -> LiveSession {
        LiveSession {
            pid: 1,
            pid_alive: true,
            session_id: Some(session_id.to_owned()),
            cwd: None,
            started_at_ms: None,
            updated_at_ms: Some(5_000),
            status_updated_at_ms: None,
            status,
            name: None,
            name_source: None,
            version: None,
            kind: Some(kind.to_owned()),
            entrypoint: None,
            transcript: None,
        }
    }

    fn cli(session_id: &str, kind: &str, state: Option<&str>, status: Option<&str>, id: Option<&str>) -> CliSession {
        CliSession {
            session_id: session_id.to_owned(),
            id: id.map(str::to_owned),
            kind: Some(kind.to_owned()),
            state: state.map(str::to_owned),
            status: status.map(str::to_owned),
        }
    }

    #[test]
    fn cli_blocked_overrides_registry_idle_and_marks_the_session_background() {
        let registry = [live("s1", Status::Idle, "bg")];
        let cli = [cli("s1", "background", Some("blocked"), Some("idle"), Some("s1id"))];
        let merged = merge(&registry, &cli);
        let activity = &merged["s1"];
        assert_eq!(activity.state, ActivityState::Blocked);
        assert!(activity.background);
        assert_eq!(activity.attach_id.as_deref(), Some("s1id"));
        assert_eq!(activity.last_activity, Some(UNIX_EPOCH + Duration::from_millis(5_000)));
    }

    #[test]
    fn registry_shell_survives_a_cli_that_calls_it_busy_and_sessions_missing_from_registry_still_appear() {
        let registry = [live("s1", Status::Other("shell".into()), "interactive")];
        let cli = [
            cli("s1", "interactive", None, Some("busy"), None),
            cli("daemon-only", "background", Some("blocked"), None, Some("d1")),
        ];
        let merged = merge(&registry, &cli);
        assert_eq!(merged["s1"].state, ActivityState::Shell);
        assert!(!merged["s1"].background);
        assert_eq!(merged["daemon-only"].state, ActivityState::Blocked);
    }

    #[test]
    fn a_session_waiting_on_the_user_needs_you_whichever_source_reports_it() {
        let registry = [live("interactive", Status::Other("waiting".into()), "interactive")];
        let cli = [cli("cli-only", "background", None, Some("waiting"), None)];
        let merged = merge(&registry, &cli);
        assert!(merged["interactive"].needs_you());
        assert!(merged["cli-only"].needs_you());
    }

    #[test]
    fn a_stale_cli_row_cannot_hold_an_interactive_session_busy_once_its_registry_file_says_idle() {
        let registry = [live("s1", Status::Idle, "interactive")];
        let cli = [cli("s1", "interactive", None, Some("busy"), Some("s1id"))];
        let merged = merge(&registry, &cli);
        assert_eq!(merged["s1"].state, ActivityState::Idle);
        assert_eq!(merged["s1"].attach_id.as_deref(), Some("s1id"));
    }

    #[test]
    fn two_processes_on_one_session_take_the_most_attention_demanding_state() {
        let registry = [live("s1", Status::Idle, "interactive"), live("s1", Status::Busy, "interactive")];
        assert_eq!(merge(&registry, &[])["s1"].state, ActivityState::Busy);
    }

    #[test]
    fn badges_and_summary_follow_the_activity() {
        let mut blocked = Activity::new(ActivityState::Blocked);
        blocked.background = true;
        let mut busy = Activity::new(ActivityState::Busy);
        busy.agents = vec![
            RunningAgent { agent_type: "explore".into(), description: "a".into() },
            RunningAgent { agent_type: "plan".into(), description: "b".into() },
        ];
        let idle = Activity::new(ActivityState::Idle);
        assert_eq!(badges(&blocked), [Badge::NeedsYou, Badge::Background]);
        assert_eq!(badges(&busy), [Badge::Agents(2)]);
        assert!(badges(&idle).is_empty());

        let summary = summarize([&blocked, &busy, &idle]);
        assert_eq!(summary, Summary { busy: 1, needs_you: 1, agents: 2 });
        let texts: Vec<String> = summary.segments().into_iter().map(|s| s.text).collect();
        assert_eq!(texts, ["1 busy", "1 needs you", "2 agents running"]);
        assert!(summarize([&idle]).segments().is_empty());
        assert!(blocked.is_active() && busy.is_active() && !idle.is_active());
    }
}
