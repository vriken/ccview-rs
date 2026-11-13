use super::live::Status;
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

const SHELL_LABEL: &str = "shell";
const WAITING_FOR_USER_LABEL: &str = "waiting";
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActivityState {
    Busy,
    Idle,
    Blocked,
    Shell,
    Other(String),
    Unknown,
}

impl ActivityState {
    pub(crate) fn label(&self) -> &str {
        match self {
            ActivityState::Busy => "busy",
            ActivityState::Idle => "idle",
            ActivityState::Blocked => "blocked",
            ActivityState::Shell => "shell",
            ActivityState::Other(label) => label,
            ActivityState::Unknown => "?",
        }
    }

    pub(crate) fn precedence(&self) -> u8 {
        match self {
            ActivityState::Blocked => 5,
            ActivityState::Shell => 4,
            ActivityState::Busy => 3,
            ActivityState::Other(_) => 2,
            ActivityState::Idle => 1,
            ActivityState::Unknown => 0,
        }
    }

    pub(crate) fn from_label(label: &str) -> Self {
        match label {
            "busy" => ActivityState::Busy,
            "idle" => ActivityState::Idle,
            "blocked" | WAITING_FOR_USER_LABEL => ActivityState::Blocked,
            SHELL_LABEL => ActivityState::Shell,
            other => ActivityState::Other(other.to_owned()),
        }
    }

    pub(crate) fn from_registry(status: &Status) -> Self {
        match status {
            Status::Busy => ActivityState::Busy,
            Status::Idle => ActivityState::Idle,
            Status::Other(label) => ActivityState::from_label(label),
            Status::Unknown => ActivityState::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activity {
    pub(crate) state: ActivityState,
    pub(crate) background: bool,
    pub(crate) attach_id: Option<String>,
    pub(crate) agents: Vec<RunningAgent>,
    pub(crate) last_activity: Option<SystemTime>,
}

impl Activity {
    pub(crate) fn new(state: ActivityState) -> Self {
        Self {
            state,
            background: false,
            attach_id: None,
            agents: Vec::new(),
            last_activity: None,
        }
    }

    pub(crate) fn needs_you(&self) -> bool {
        self.state == ActivityState::Blocked
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self.state, ActivityState::Busy | ActivityState::Blocked)
            || !self.agents.is_empty()
    }

    pub(crate) fn absorb(&mut self, state: ActivityState) {
        if state.precedence() > self.state.precedence() {
            self.state = state;
        }
    }
}

pub(crate) type ActivityMap = HashMap<String, Activity>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Summary {
    pub(crate) busy: usize,
    pub(crate) needs_you: usize,
    pub(crate) agents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) text: String,
    pub(crate) attention: bool,
}

pub(crate) fn summarize<'a>(activities: impl IntoIterator<Item = &'a Activity>) -> Summary {
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
    pub(crate) fn segments(&self) -> Vec<Segment> {
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
pub(crate) enum Badge {
    NeedsYou,
    Agents(usize),
    Background,
    Shell,
}

impl Badge {
    pub(crate) fn text(&self) -> String {
        match self {
            Badge::NeedsYou => "needs you".to_owned(),
            Badge::Agents(count) => format!("⛭ {count}"),
            Badge::Background => "bg".to_owned(),
            Badge::Shell => "sh".to_owned(),
        }
    }
}

pub(crate) fn badges(activity: &Activity) -> Vec<Badge> {
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

pub(crate) fn quiet_for(activity: &Activity, on_hold: bool, now: SystemTime) -> Option<Duration> {
    if activity.state != ActivityState::Busy || !activity.agents.is_empty() || on_hold {
        return None;
    }
    let quiet = now.duration_since(activity.last_activity?).ok()?;
    (quiet >= STALE_AFTER).then_some(quiet)
}

fn idle_without_agents(activity: Option<&Activity>) -> bool {
    activity
        .is_some_and(|activity| activity.state == ActivityState::Idle && activity.agents.is_empty())
}

pub(crate) fn asks_you(asks_user: bool, activity: Option<&Activity>) -> bool {
    asks_user && idle_without_agents(activity)
}

pub(crate) fn owes_you(owed: usize, activity: Option<&Activity>) -> bool {
    owed > 0 && idle_without_agents(activity)
}

pub(crate) fn attention_rank(asks_user: bool, owed: usize, activity: Option<&Activity>) -> u8 {
    match activity {
        Some(activity) if activity.needs_you() => 0,
        _ if asks_you(asks_user, activity) || owes_you(owed, activity) => 1,
        Some(activity) if activity.is_active() => 2,
        _ => 3,
    }
}

pub(crate) const ASKS_YOU_LABEL: &str = "asks you";

pub(crate) fn for_you_label(owed: usize) -> String {
    format!("{owed} for you")
}

pub(crate) fn is_session_active(live_pid_alive: bool, activity: Option<&Activity>) -> bool {
    live_pid_alive || activity.is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningAgent {
    pub(crate) agent_type: String,
    pub(crate) description: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    const NOW_SECS: u64 = 100_000;

    fn busy_since(quiet: Duration) -> Activity {
        let mut activity = Activity::new(ActivityState::Busy);
        activity.last_activity = Some(UNIX_EPOCH + Duration::from_secs(NOW_SECS) - quiet);
        activity
    }

    fn quiet(activity: &Activity, on_hold: bool) -> Option<Duration> {
        quiet_for(
            activity,
            on_hold,
            UNIX_EPOCH + Duration::from_secs(NOW_SECS),
        )
    }

    #[test]
    fn only_a_busy_session_quiet_for_half_an_hour_and_not_held_is_stale() {
        let minute = Duration::from_secs(60);
        assert_eq!(quiet(&busy_since(STALE_AFTER), false), Some(STALE_AFTER));
        assert_eq!(quiet(&busy_since(STALE_AFTER - minute), false), None);
        assert_eq!(quiet(&busy_since(STALE_AFTER * 2), true), None);
        let mut idle = busy_since(STALE_AFTER * 2);
        idle.state = ActivityState::Idle;
        assert_eq!(quiet(&idle, false), None);
        let mut blocked = busy_since(STALE_AFTER * 2);
        blocked.state = ActivityState::Blocked;
        assert_eq!(quiet(&blocked, false), None);
        let mut with_agent = busy_since(STALE_AFTER * 2);
        with_agent.agents.push(RunningAgent {
            agent_type: "x".to_owned(),
            description: "y".to_owned(),
        });
        assert_eq!(quiet(&with_agent, false), None);
    }
}
