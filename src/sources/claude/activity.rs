use crate::{
    session::{
        activity::{Activity, ActivityMap, ActivityState},
        live::LiveSession,
    },
    sources::claude::{agents_cli::CliSession, registry, subagents},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const BACKGROUND_KINDS: [&str; 2] = ["background", "bg"];
pub(crate) fn merge(registry: &[LiveSession], cli: &[CliSession]) -> ActivityMap {
    let mut map = ActivityMap::new();
    let registry_ids: HashSet<&str> = registry
        .iter()
        .filter_map(|session| session.session_id.as_deref())
        .collect();
    for session in registry {
        let Some(session_id) = &session.session_id else {
            continue;
        };
        let activity = entry_for(
            &mut map,
            session_id,
            ActivityState::from_registry(&session.status),
        );
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

fn entry_for<'a>(
    map: &'a mut ActivityMap,
    session_id: &str,
    initial: ActivityState,
) -> &'a mut Activity {
    map.entry(session_id.to_owned())
        .or_insert_with(|| Activity::new(initial))
}

fn is_background(kind: &str) -> bool {
    BACKGROUND_KINDS.contains(&kind)
}

pub(crate) struct Tracker {
    projects_dir: PathBuf,
    transcripts: HashMap<String, PathBuf>,
}

impl Tracker {
    pub(crate) fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            transcripts: HashMap::new(),
        }
    }

    pub(crate) fn compute(
        &mut self,
        live: &[LiveSession],
        cli: &[CliSession],
        now: SystemTime,
    ) -> ActivityMap {
        let mut map = merge(live, cli);
        for (session_id, activity) in &mut map {
            if let Some(transcript) = self.transcript_of(live, session_id) {
                activity.agents =
                    subagents::running_in(&subagents_dir(&transcript), Some(&transcript), now);
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
        self.transcripts
            .insert(session_id.to_owned(), found.clone());
        Some(found)
    }
}

pub(crate) fn subagents_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::live::Status;

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

    fn cli(
        session_id: &str,
        kind: &str,
        state: Option<&str>,
        status: Option<&str>,
        id: Option<&str>,
    ) -> CliSession {
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
        let cli = [cli(
            "s1",
            "background",
            Some("blocked"),
            Some("idle"),
            Some("s1id"),
        )];
        let merged = merge(&registry, &cli);
        let activity = &merged["s1"];
        assert_eq!(activity.state, ActivityState::Blocked);
        assert!(activity.background);
        assert_eq!(activity.attach_id.as_deref(), Some("s1id"));
        assert_eq!(
            activity.last_activity,
            Some(UNIX_EPOCH + Duration::from_millis(5_000))
        );
    }

    #[test]
    fn registry_shell_survives_a_cli_that_calls_it_busy_and_sessions_missing_from_registry_still_appear()
     {
        let registry = [live("s1", Status::Other("shell".into()), "interactive")];
        let cli = [
            cli("s1", "interactive", None, Some("busy"), None),
            cli(
                "daemon-only",
                "background",
                Some("blocked"),
                None,
                Some("d1"),
            ),
        ];
        let merged = merge(&registry, &cli);
        assert_eq!(merged["s1"].state, ActivityState::Shell);
        assert!(!merged["s1"].background);
        assert_eq!(merged["daemon-only"].state, ActivityState::Blocked);
    }

    #[test]
    fn a_session_waiting_on_the_user_needs_you_whichever_source_reports_it() {
        let registry = [live(
            "interactive",
            Status::Other("waiting".into()),
            "interactive",
        )];
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
        let registry = [
            live("s1", Status::Idle, "interactive"),
            live("s1", Status::Busy, "interactive"),
        ];
        assert_eq!(merge(&registry, &[])["s1"].state, ActivityState::Busy);
    }
}
