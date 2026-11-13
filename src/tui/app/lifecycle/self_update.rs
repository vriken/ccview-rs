use crate::{
    store::ui_state::{self, FocusZone, UiState},
    sys::executable::{ExecutableStamp, stamp_of_executable},
    tui::{App, app::Focus, reader::selection::View},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) const DISABLE_ENV: &str = "CCVIEW_NO_SELF_UPDATE";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const STABLE_FOR: Duration = Duration::from_secs(1);
const LOOP_GUARD: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Notice {
    LeaveSession,
    RestartCcv,
}

impl Notice {
    pub(in crate::tui) fn text(self) -> &'static str {
        match self {
            Notice::LeaveSession => "update ready: leave the session to restart",
            Notice::RestartCcv => "update ready: restart ccv",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Restart,
    Wait,
    Notice(Notice),
}

#[derive(Debug, Clone, Copy)]
struct Observation {
    startup: ExecutableStamp,
    current: ExecutableStamp,
    stable_for: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Situation {
    auto_restart: bool,
    session_focused: bool,
    interaction_pending: bool,
    restarted_recently: bool,
}

fn decide(observation: Observation, situation: Situation) -> Decision {
    let replaced = observation.current != observation.startup && observation.current.runnable;
    if !replaced || observation.stable_for < STABLE_FOR {
        return Decision::Wait;
    }
    if situation.restarted_recently || !situation.auto_restart {
        return Decision::Notice(Notice::RestartCcv);
    }
    if situation.session_focused {
        return Decision::Notice(Notice::LeaveSession);
    }
    if situation.interaction_pending {
        return Decision::Wait;
    }
    Decision::Restart
}

pub(in crate::tui) struct Watcher {
    path: PathBuf,
    arg0: OsString,
    args: Vec<OsString>,
    startup: ExecutableStamp,
    started: Instant,
    seen: ExecutableStamp,
    seen_since: Instant,
    confirmed_at: Instant,
}

pub(in crate::tui) struct Relaunch {
    pub(in crate::tui) path: PathBuf,
    pub(in crate::tui) arg0: OsString,
    pub(in crate::tui) args: Vec<OsString>,
}

impl Watcher {
    pub(in crate::tui) fn start(now: Instant) -> Option<Self> {
        if std::env::var_os(DISABLE_ENV).is_some_and(|value| !value.is_empty()) {
            return None;
        }
        let path = std::env::current_exe().ok()?.canonicalize().ok()?;
        let startup = stamp_of_executable(&path)?;
        let mut argv = std::env::args_os();
        let arg0 = argv.next().unwrap_or_else(|| path.clone().into_os_string());
        Some(Self {
            path,
            arg0,
            args: argv.collect(),
            startup,
            started: now,
            seen: startup,
            seen_since: now,
            confirmed_at: now,
        })
    }

    fn poll(&mut self, now: Instant) {
        if now.duration_since(self.confirmed_at) < POLL_INTERVAL {
            return;
        }
        self.confirmed_at = now;
        let current = stamp_of_executable(&self.path).unwrap_or(self.seen);
        if current != self.seen {
            self.seen = current;
            self.seen_since = now;
        }
    }

    fn observation(&self) -> Observation {
        Observation {
            startup: self.startup,
            current: self.seen,
            stable_for: self.confirmed_at.duration_since(self.seen_since),
        }
    }

    pub(in crate::tui) fn relaunch(&self) -> Relaunch {
        Relaunch {
            path: self.path.clone(),
            arg0: self.arg0.clone(),
            args: self.args.clone(),
        }
    }
}

impl App {
    pub(in crate::tui) fn with_self_update(mut self, watcher: Option<Watcher>) -> Self {
        self.self_update = watcher;
        self
    }

    pub(crate) fn with_restored_state(mut self, state: Option<UiState>) -> Self {
        self.restarted = state.is_some();
        self.pending_restore = state;
        self
    }

    pub(in crate::tui) fn take_relaunch(&mut self) -> Option<Relaunch> {
        if !self.restart_requested {
            return None;
        }
        self.self_update.as_ref().map(Watcher::relaunch)
    }

    fn update_situation(&self, now: Instant) -> Situation {
        let started = self.self_update.as_ref().map(|watcher| watcher.started);
        Situation {
            auto_restart: self.split.settings().auto_restart_on_update,
            session_focused: self.focus == Focus::Embedded,
            interaction_pending: self.confirm.is_some()
                || self.overlay.is_some()
                || self.drag.is_some()
                || self.search.is_some(),
            restarted_recently: self.restarted
                && started.is_some_and(|started| now.duration_since(started) < LOOP_GUARD),
        }
    }

    pub(in crate::tui) fn check_self_update(&mut self, state_path: &Path) -> bool {
        let now = Instant::now();
        let situation = self.update_situation(now);
        let Some(watcher) = self.self_update.as_mut() else {
            return false;
        };
        watcher.poll(now);
        let wanted = match decide(watcher.observation(), situation) {
            Decision::Wait => None,
            Decision::Notice(notice) => Some(notice),
            Decision::Restart => {
                return self.begin_restart(state_path);
            }
        };
        std::mem::replace(&mut self.update_notice, wanted) != wanted
    }

    fn begin_restart(&mut self, state_path: &Path) -> bool {
        match self.capture_ui_state().save(state_path) {
            Ok(()) => {
                self.restart_requested = true;
                self.should_quit = true;
            }
            Err(err) => {
                self.self_update = None;
                self.update_notice = Some(Notice::RestartCcv);
                self.notice = Some(format!("could not save state for the restart: {err}"));
            }
        }
        true
    }

    pub(in crate::tui) fn capture_ui_state(&self) -> UiState {
        UiState {
            tab: self.active.storage_key(),
            session_id: self.selected_entry().map(|entry| entry.session_id.clone()),
            focus: if self.focus == Focus::Conversation {
                FocusZone::Transcript
            } else {
                FocusZone::List
            },
            scroll: self.current_scroll(),
            follow_bottom: self.follow_bottom,
            show_hidden: self.show_hidden,
            expanded: self.catalog.expanded(),
            saved_ms: ui_state::now_ms(),
        }
    }

    pub(in crate::tui) fn apply_pending_restore(&mut self) -> bool {
        let Some(state) = self.pending_restore.as_ref() else {
            return false;
        };
        let session_known = state.session_id.as_ref().is_some_and(|id| {
            self.catalog
                .entries()
                .iter()
                .any(|entry| &entry.session_id == id)
        });
        if self.scanning && !session_known {
            return false;
        }
        let Some(state) = self.pending_restore.take() else {
            return false;
        };
        self.apply_ui_state(&state);
        true
    }

    pub(in crate::tui) fn apply_ui_state(&mut self, state: &UiState) {
        self.show_hidden = state.show_hidden;
        self.catalog.set_expanded(state.expanded);
        self.rebuild_view();
        let tab = self
            .tabs
            .iter()
            .map(|tab| &tab.key)
            .find(|key| key.storage_key() == state.tab)
            .cloned();
        if let Some(tab) = tab {
            self.active = tab;
            self.refresh_rows();
            self.rebuild_view();
        }
        let wanted = state.session_id.as_ref().and_then(|id| {
            self.rows
                .iter()
                .map(|&row| &self.catalog.entries()[row])
                .find(|entry| &entry.session_id == id)
                .map(|entry| entry.path.clone())
        });
        let Some(path) = wanted else {
            return;
        };
        let reading =
            state.focus == FocusZone::Transcript && self.reading_focus() == Focus::Conversation;
        if reading {
            self.pending_scroll = Some((path.clone(), state.scroll, state.follow_bottom));
        }
        self.select_path(Some(path));
        if reading {
            self.focus = Focus::Conversation;
            self.restore_scroll_when_ready();
        }
    }

    pub(in crate::tui) fn restore_scroll_when_ready(&mut self) {
        let Some((path, scroll, follow_bottom)) = self.pending_scroll.clone() else {
            return;
        };
        if self.selected_path.as_ref() != Some(&path) {
            self.pending_scroll = None;
            return;
        }
        if self.transcript.is_stale() || !matches!(self.transcript.view(), View::Content(_)) {
            return;
        }
        self.pending_scroll = None;
        self.scroll = scroll;
        self.follow_bottom = follow_bottom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn stamp_of_file(name: &str, content: &[u8], mode: u32) -> ExecutableStamp {
        let dir =
            std::env::temp_dir().join(format!("ccview-self-update-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bin");
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let stamp = stamp_of_executable(&path).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        stamp
    }

    fn replaced_binary() -> ExecutableStamp {
        stamp_of_file("good", b"new build", 0o755)
    }

    fn at_startup() -> ExecutableStamp {
        stamp_of_file("startup", b"old", 0o755)
    }

    fn observed(
        startup: ExecutableStamp,
        current: ExecutableStamp,
        stable_secs: u64,
    ) -> Observation {
        Observation {
            startup,
            current,
            stable_for: Duration::from_secs(stable_secs),
        }
    }

    const FREE: Situation = Situation {
        auto_restart: true,
        session_focused: false,
        interaction_pending: false,
        restarted_recently: false,
    };

    #[test]
    fn a_replaced_binary_restarts_only_when_stable_runnable_and_the_user_is_not_busy() {
        let startup = at_startup();
        let cases = [
            (
                "unchanged binary",
                observed(startup, startup, 60),
                FREE,
                Decision::Wait,
            ),
            (
                "still being written",
                observed(startup, replaced_binary(), 0),
                FREE,
                Decision::Wait,
            ),
            (
                "empty file from a half-written install",
                observed(startup, stamp_of_file("empty", b"", 0o755), 60),
                FREE,
                Decision::Wait,
            ),
            (
                "not executable",
                observed(startup, stamp_of_file("plain", b"new build", 0o644), 60),
                FREE,
                Decision::Wait,
            ),
            (
                "stable and free",
                observed(startup, replaced_binary(), 2),
                FREE,
                Decision::Restart,
            ),
            (
                "typing into a session",
                observed(startup, replaced_binary(), 2),
                Situation {
                    session_focused: true,
                    ..FREE
                },
                Decision::Notice(Notice::LeaveSession),
            ),
            (
                "confirm or overlay pending",
                observed(startup, replaced_binary(), 2),
                Situation {
                    interaction_pending: true,
                    ..FREE
                },
                Decision::Wait,
            ),
            (
                "setting off",
                observed(startup, replaced_binary(), 2),
                Situation {
                    auto_restart: false,
                    ..FREE
                },
                Decision::Notice(Notice::RestartCcv),
            ),
            (
                "changed again right after a restart",
                observed(startup, replaced_binary(), 2),
                Situation {
                    restarted_recently: true,
                    ..FREE
                },
                Decision::Notice(Notice::RestartCcv),
            ),
        ];
        for (name, observation, situation, expected) in cases {
            assert_eq!(decide(observation, situation), expected, "{name}");
        }
    }
}
