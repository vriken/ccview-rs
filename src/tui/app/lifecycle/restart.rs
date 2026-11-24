use crate::{
    harness::Launch,
    session::{
        activity::{Activity, ActivityState},
        harness_id::HarnessId,
    },
    sys::process::pid_alive,
    tui::app::{
        App, Confirm, Focus,
        flags::stop::{StopTarget, short_text},
        lifecycle::launch::{ATTACH_TITLE_PREFIX, PendingLaunch, Target},
        nav::ListFocus,
    },
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const EXIT_WAIT: Duration = Duration::from_secs(10);
const NOT_HOSTED: &str = "restart: not running in ccv";
const NO_SESSION_ID: &str = "restart: session id not known yet";
const CLAUDE_ONLY: &str = "restart is Claude only";
const IN_PROGRESS: &str = "a restart is already in progress";
const BACKGROUND: &str = "restart: attached to a background session";
const BUSY: &str = "busy, restart when idle";

pub(in crate::tui) struct PendingRestart {
    pane: u64,
    session_id: String,
    cwd: PathBuf,
    from_pane: bool,
    pub(in crate::tui) title: String,
}

pub(in crate::tui) struct Restarting {
    restart: PendingRestart,
    pid: Option<u32>,
    deadline: Instant,
}

impl App {
    pub(in crate::tui) fn restart_selected(&mut self) {
        if self.focus == Focus::List && self.list_focus == ListFocus::Tabs {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let session_id = entry.session_id.clone();
        let pid = entry
            .live
            .as_ref()
            .filter(|live| live.pid_alive)
            .map(|live| live.pid);
        match self.stop_target(&session_id, pid) {
            StopTarget::Hosted(pane) => self.request_restart(pane),
            StopTarget::Registry(_) | StopTarget::Nothing => self.say(NOT_HOSTED),
        }
    }

    pub(in crate::tui) fn request_restart(&mut self, pane: u64) {
        if self.restarting.is_some() {
            return self.say(IN_PROGRESS);
        }
        let Some(session) = self
            .sessions
            .get(pane)
            .filter(|session| session.is_running())
        else {
            return self.say(NOT_HOSTED);
        };
        if session.harness != HarnessId::Claude {
            return self.say(CLAUDE_ONLY);
        }
        let Some(session_id) = session.session_id.clone() else {
            return self.say(NO_SESSION_ID);
        };
        if session.title.starts_with(ATTACH_TITLE_PREFIX) {
            return self.say(BACKGROUND);
        }
        let title = session.title.clone();
        let Some(cwd) = session.cwd.clone().or_else(|| self.cwd_of(&session_id)) else {
            return self.say(NOT_HOSTED);
        };
        if let Some(reason) = self.refusal(&session_id) {
            return self.say(reason);
        }
        self.confirm = Some(Confirm::Restart(PendingRestart {
            pane,
            session_id,
            cwd,
            from_pane: self.focus == Focus::Embedded && self.shown == Some(pane),
            title,
        }));
    }

    pub(in crate::tui) fn begin_session_restart(&mut self, restart: PendingRestart) {
        if let Some(reason) = self.refusal(&restart.session_id) {
            return self.say(reason);
        }
        let pid = self
            .sessions
            .get(restart.pane)
            .and_then(|session| session.host().process_id());
        if let Err(error) = self.sessions.stop(restart.pane) {
            let text = format!("could not restart {}: {error}", short_text(&restart.title));
            return self.say(&text);
        }
        let text = format!("restarting {}", short_text(&restart.title));
        self.say(&text);
        self.restarting = Some(Restarting {
            restart,
            pid,
            deadline: Instant::now() + EXIT_WAIT,
        });
    }

    pub(in crate::tui) fn advance_restart(&mut self) -> bool {
        let Some(restarting) = self.restarting.take() else {
            return false;
        };
        let exited = match self.sessions.get(restarting.restart.pane) {
            Some(session) => {
                !session.is_running() && restarting.pid.is_none_or(|pid| !pid_alive(pid))
            }
            None => restarting.pid.is_some_and(|pid| !pid_alive(pid)),
        };
        if exited {
            self.resume_restarted(restarting.restart);
            return true;
        }
        if Instant::now() >= restarting.deadline {
            let text = format!(
                "{} did not exit; not restarted",
                short_text(&restarting.restart.title)
            );
            self.say(&text);
            return true;
        }
        self.restarting = Some(restarting);
        false
    }

    fn resume_restarted(&mut self, restart: PendingRestart) {
        let follow = restart.from_pane
            && self.focus == Focus::Embedded
            && self.shown.is_none_or(|id| id == restart.pane);
        let (shown, focus) = (self.shown, self.focus);
        self.notice = None;
        self.launch(PendingLaunch {
            harness: HarnessId::Claude,
            target: Target::Embedded,
            launch: Launch::Resume(restart.session_id.clone()),
            cwd: restart.cwd,
            title: restart.title,
            session_id: Some(restart.session_id),
        });
        if follow {
            return;
        }
        if shown != Some(restart.pane) {
            self.shown = shown;
        }
        self.focus = focus;
    }

    fn refusal(&self, session_id: &str) -> Option<&'static str> {
        let activities = self.catalog.activity();
        self.catalog
            .lineage_ids(session_id)
            .iter()
            .filter_map(|id| activities.get(id))
            .find_map(refusal_for)
    }

    fn say(&mut self, text: &str) {
        if self.focus == Focus::Embedded {
            self.show_alert(text);
        } else {
            self.notice = Some(text.to_owned());
        }
    }

    fn cwd_of(&self, session_id: &str) -> Option<PathBuf> {
        self.catalog
            .entries()
            .iter()
            .find(|entry| entry.session_id == session_id)
            .map(|entry| PathBuf::from(&entry.cwd))
    }
}

fn refusal_for(activity: &Activity) -> Option<&'static str> {
    if activity.background {
        return Some(BACKGROUND);
    }
    let working = matches!(activity.state, ActivityState::Busy | ActivityState::Blocked)
        || !activity.agents.is_empty();
    working.then_some(BUSY)
}
