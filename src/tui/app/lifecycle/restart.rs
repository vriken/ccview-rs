use crate::{
    harness::Launch,
    session::{
        activity::{Activity, ActivityState},
        harness_id::HarnessId,
    },
    sys::{process::pid_alive, time::now_ms},
    tui::app::{
        App, Confirm, Focus,
        flags::stop::{StopTarget, short_text},
        lifecycle::launch::{ATTACH_TITLE_PREFIX, PendingLaunch, Target},
        nav::ListFocus,
    },
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

const EXIT_WAIT: Duration = Duration::from_secs(10);
const AUTO_DELAY: Duration = Duration::from_secs(60);
const AUTO_DELAY_ENV: &str = "CCVIEW_AUTO_RESTART_DELAY_MS";
const NO_DIRECTORY: &str = "restart: directory not found";
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

pub(in crate::tui) struct AutoRestart {
    tried: HashSet<u64>,
    quiet_since: HashMap<u64, Instant>,
    delay: Duration,
}

impl Default for AutoRestart {
    fn default() -> Self {
        let delay = std::env::var(AUTO_DELAY_ENV)
            .ok()
            .and_then(|millis| millis.parse().ok())
            .map_or(AUTO_DELAY, Duration::from_millis);
        Self {
            tried: HashSet::new(),
            quiet_since: HashMap::new(),
            delay,
        }
    }
}

impl AutoRestart {
    fn outdated(&self, pane: u64, launched_at_ms: u64, changed_ms: u64) -> bool {
        let delay_ms = u64::try_from(self.delay.as_millis()).unwrap_or(u64::MAX);
        launched_at_ms.saturating_add(delay_ms) < changed_ms && !self.tried.contains(&pane)
    }

    fn settled(&mut self, pane: u64, quiet: bool, now: Instant) -> bool {
        if !quiet {
            self.quiet_since.remove(&pane);
            return false;
        }
        let since = *self.quiet_since.entry(pane).or_insert(now);
        now.duration_since(since) >= self.delay
    }
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
        let from_pane = self.focus == Focus::Embedded && self.shown == Some(pane);
        match self.restart_candidate(pane, from_pane) {
            Ok(restart) => self.confirm = Some(Confirm::Restart(restart)),
            Err(reason) => self.say(reason),
        }
    }

    pub(in crate::tui) fn auto_restart_outdated(&mut self) -> bool {
        if !self.auto_restart_allowed() {
            self.auto_restart.quiet_since.clear();
            return false;
        }
        let now_ms = now_ms();
        let Some(changed_ms) = self.config_changed_ms.filter(|&changed| changed <= now_ms) else {
            return false;
        };
        let outdated: Vec<(u64, u64)> = self
            .sessions
            .all()
            .iter()
            .filter(|session| {
                self.auto_restart
                    .outdated(session.id, session.launched_at_ms, changed_ms)
            })
            .map(|session| (session.id, session.launched_at_ms))
            .collect();
        let now = Instant::now();
        let mut due = None;
        for (pane, launched_at_ms) in outdated {
            let quiet = self
                .restart_candidate(pane, false)
                .ok()
                .filter(|restart| self.is_quiet(&restart.session_id, launched_at_ms, now_ms));
            let settled = self.auto_restart.settled(pane, quiet.is_some(), now);
            if settled && due.is_none() {
                due = quiet;
            }
        }
        let Some(restart) = due else {
            return false;
        };
        self.auto_restart.quiet_since.remove(&restart.pane);
        self.auto_restart.tried.insert(restart.pane);
        self.begin_session_restart(restart);
        true
    }

    fn auto_restart_allowed(&self) -> bool {
        self.split.settings().auto_restart_sessions
            && !self.scanning
            && self.focus == Focus::List
            && self.restarting.is_none()
            && self.confirm.is_none()
            && self.overlay.is_none()
            && self.search.is_none()
            && self.drag.is_none()
    }

    fn is_quiet(&self, session_id: &str, launched_at_ms: u64, now_ms: u64) -> bool {
        let scanned = self
            .catalog
            .entries()
            .iter()
            .any(|entry| entry.session_id == session_id);
        scanned && !self.has_pending_work(session_id, launched_at_ms, now_ms)
    }

    fn has_pending_work(&self, session_id: &str, launched_at_ms: u64, now_ms: u64) -> bool {
        let lineage = self.catalog.lineage_ids(session_id);
        self.catalog
            .entries()
            .iter()
            .filter(|entry| lineage.contains(&entry.session_id))
            .any(|entry| entry.pending_work.blocks_restart(launched_at_ms, now_ms))
    }

    fn restart_candidate(
        &self,
        pane: u64,
        from_pane: bool,
    ) -> Result<PendingRestart, &'static str> {
        let session = self
            .sessions
            .get(pane)
            .filter(|session| session.is_running())
            .ok_or(NOT_HOSTED)?;
        if session.harness != HarnessId::Claude {
            return Err(CLAUDE_ONLY);
        }
        let session_id = session.session_id.clone().ok_or(NO_SESSION_ID)?;
        if session.title.starts_with(ATTACH_TITLE_PREFIX) {
            return Err(BACKGROUND);
        }
        let cwd = session
            .cwd
            .clone()
            .or_else(|| self.cwd_of(&session_id))
            .ok_or(NOT_HOSTED)?;
        if let Some(reason) = self.refusal(&session_id) {
            return Err(reason);
        }
        Ok(PendingRestart {
            pane,
            session_id,
            cwd,
            from_pane,
            title: session.title.clone(),
        })
    }

    pub(in crate::tui) fn begin_session_restart(&mut self, restart: PendingRestart) {
        if let Some(reason) = self.refusal(&restart.session_id) {
            return self.say(reason);
        }
        if !restart.cwd.is_dir() {
            return self.say(NO_DIRECTORY);
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
