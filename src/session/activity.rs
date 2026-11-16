use super::{entry::SessionEntry, live::Status};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

const SHELL_LABEL: &str = "shell";
const WAITING_FOR_USER_LABEL: &str = "waiting";
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
pub(crate) const AGENT_STUCK_AFTER: Duration = Duration::from_secs(10 * 60);
pub(crate) const QUIET_MARK: &str = "quiet";

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
    pub(crate) quiet_agents: usize,
    pub(crate) longest_agent_quiet: Option<Duration>,
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
        let stuck = activity.agents.iter().filter_map(RunningAgent::stuck_for);
        summary.quiet_agents += stuck.clone().count();
        summary.longest_agent_quiet = summary.longest_agent_quiet.max(stuck.max());
        summary.agents += activity.agents.len();
    }
    summary.agents -= summary.quiet_agents;
    summary
}

impl Summary {
    pub(crate) fn segments(&self) -> Vec<Segment> {
        let segment = |count: usize, label: String, attention: bool| {
            (count > 0).then(|| Segment {
                text: format!("{count} {label}"),
                attention,
            })
        };
        let quiet = self
            .longest_agent_quiet
            .map(|quiet| format!("{} {}", agent_noun(self.quiet_agents), quiet_text(quiet)))
            .unwrap_or_default();
        [
            segment(self.busy, "busy".to_owned(), false),
            segment(self.needs_you, "needs you".to_owned(), true),
            segment(
                self.agents,
                format!("{} running", agent_noun(self.agents)),
                false,
            ),
            segment(self.quiet_agents, quiet, false),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Badge {
    NeedsYou,
    Agents {
        count: usize,
        quiet: Option<Duration>,
    },
    Background,
    Shell,
}

impl Badge {
    pub(crate) fn text(&self) -> String {
        match self {
            Badge::NeedsYou => "needs you".to_owned(),
            Badge::Agents { count, quiet: None } => format!("⛭ {count}"),
            Badge::Agents {
                count,
                quiet: Some(quiet),
            } => format!("⛭ {count} {}", quiet_text(*quiet)),
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
        badges.push(Badge::Agents {
            count: activity.agents.len(),
            quiet: longest_agent_quiet(&activity.agents),
        });
    }
    if activity.background {
        badges.push(Badge::Background);
    }
    if activity.state == ActivityState::Shell {
        badges.push(Badge::Shell);
    }
    badges
}

fn agent_noun(count: usize) -> &'static str {
    if count == 1 { "agent" } else { "agents" }
}

pub(crate) fn quiet_text(quiet: Duration) -> String {
    let span = match quiet.as_secs() {
        seconds @ 0..3600 => format!("{}m", seconds / 60),
        seconds @ 3600..86_400 => format!("{}h", seconds / 3600),
        seconds => format!("{}d", seconds / 86_400),
    };
    format!("{QUIET_MARK} {span}")
}

fn longest_agent_quiet(agents: &[RunningAgent]) -> Option<Duration> {
    agents.iter().filter_map(RunningAgent::stuck_for).max()
}

pub(crate) fn quiet_for(activity: &Activity, on_hold: bool, now: SystemTime) -> Option<Duration> {
    if activity.state != ActivityState::Busy || on_hold {
        return None;
    }
    let parent = now.duration_since(activity.last_activity?).ok()?;
    if parent < STALE_AFTER {
        return None;
    }
    let agents = activity
        .agents
        .iter()
        .map(RunningAgent::stuck_for)
        .collect::<Option<Vec<_>>>()?;
    Some(agents.into_iter().fold(parent, Duration::min))
}

fn idle_without_agents(activity: Option<&Activity>) -> bool {
    activity
        .is_some_and(|activity| activity.state == ActivityState::Idle && activity.agents.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleState {
    AsksYou,
    ForYou,
    Waiting,
    Stalled,
    Done,
    Idle,
}

pub(crate) fn idle_state(entry: &SessionEntry, activity: Option<&Activity>) -> Option<IdleState> {
    if !idle_without_agents(activity) {
        return None;
    }
    let state = if entry.asks_user {
        IdleState::AsksYou
    } else if entry.owed > 0 {
        IdleState::ForYou
    } else {
        match &entry.dod {
            Some(dod) if dod.waits > 0 => IdleState::Waiting,
            Some(dod) if dod.open > 0 => IdleState::Stalled,
            Some(dod) if dod.file_open == 0 => IdleState::Done,
            _ => IdleState::Idle,
        }
    };
    Some(state)
}

pub(crate) fn attention_rank(entry: &SessionEntry, activity: Option<&Activity>) -> u8 {
    match (activity, idle_state(entry, activity)) {
        (Some(activity), _) if activity.needs_you() => 0,
        (_, Some(IdleState::AsksYou | IdleState::ForYou | IdleState::Stalled)) => 1,
        (Some(activity), _) if activity.is_active() => 2,
        (_, Some(IdleState::Waiting)) => 4,
        (_, Some(IdleState::Done)) => 5,
        _ => 3,
    }
}

pub(crate) const ASKS_YOU_LABEL: &str = "asks you";
pub(crate) const STALLED_LABEL: &str = "stalled";
pub(crate) const DONE_LABEL: &str = "done";

pub(crate) fn for_you_label(owed: usize) -> String {
    format!("{owed} for you")
}

pub(crate) fn waiting_label(label: Option<&str>) -> String {
    format!("waiting on {}", label.unwrap_or_default())
        .trim_end()
        .to_owned()
}

pub(crate) fn open_label(open: usize) -> String {
    format!("{open} open")
}

pub(crate) fn is_session_active(live_pid_alive: bool, activity: Option<&Activity>) -> bool {
    live_pid_alive || activity.is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningAgent {
    pub(crate) agent_type: String,
    pub(crate) description: String,
    pub(crate) quiet: Option<Duration>,
}

impl RunningAgent {
    pub(crate) fn stuck_for(&self) -> Option<Duration> {
        self.quiet.filter(|quiet| *quiet >= AGENT_STUCK_AFTER)
    }
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
        with_agent
            .agents
            .push(agent(Some(AGENT_STUCK_AFTER - minute)));
        assert_eq!(quiet(&with_agent, false), None);
        with_agent.agents.push(agent(Some(AGENT_STUCK_AFTER * 2)));
        assert_eq!(quiet(&with_agent, false), None);
        let mut all_quiet = busy_since(STALE_AFTER * 2);
        all_quiet.agents.push(agent(Some(AGENT_STUCK_AFTER * 3)));
        all_quiet.agents.push(agent(Some(AGENT_STUCK_AFTER)));
        assert_eq!(quiet(&all_quiet, false), Some(AGENT_STUCK_AFTER));
        let mut parent_active = busy_since(minute);
        parent_active
            .agents
            .push(agent(Some(AGENT_STUCK_AFTER * 3)));
        assert_eq!(quiet(&parent_active, false), None);
    }

    fn agent(quiet: Option<Duration>) -> RunningAgent {
        RunningAgent {
            agent_type: "x".to_owned(),
            description: "y".to_owned(),
            quiet,
        }
    }

    #[test]
    fn a_quiet_agent_is_marked_on_the_row_and_in_the_footer_and_a_busy_one_is_not() {
        let minute = Duration::from_secs(60);
        let mut one_quiet = Activity::new(ActivityState::Busy);
        one_quiet.agents.push(agent(Some(17 * minute)));
        let mut one_working = Activity::new(ActivityState::Busy);
        one_working.agents.push(agent(Some(minute)));
        let row = |activity: &Activity| badges(activity)[0].text();
        let footer = |activities: &[&Activity]| -> Vec<String> {
            summarize(activities.iter().copied())
                .segments()
                .into_iter()
                .map(|segment| segment.text)
                .collect()
        };

        assert_eq!(row(&one_quiet), "⛭ 1 quiet 17m");
        assert_eq!(row(&one_working), "⛭ 1");
        assert_eq!(footer(&[&one_quiet]), ["1 busy", "1 agent quiet 17m"]);
        assert_eq!(footer(&[&one_working]), ["1 busy", "1 agent running"]);
        assert_eq!(
            footer(&[&one_working, &one_working, &one_quiet]),
            ["3 busy", "2 agents running", "1 agent quiet 17m"]
        );
    }
}
