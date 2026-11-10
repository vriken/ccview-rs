use super::{App, Confirm, Focus, sessions::Launched, tabs::TabKey};
use crate::{
    external,
    harness::{CodexCli, Harness, Launch, LaunchKind, OpenCodeCli},
    sources::HarnessId,
};
use crossterm::event::{KeyCode, KeyEvent};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Embedded,
    ExternalTerminal,
}

pub struct PendingLaunch {
    harness: HarnessId,
    target: Target,
    launch: Launch,
    cwd: PathBuf,
    title: String,
    session_id: Option<String>,
}

impl App {
    pub(super) fn request_launch(&mut self, kind: LaunchKind, target: Target) {
        let harness = self.selected_harness_id();
        if !self.harness_for(harness).supports(kind) {
            self.notice = Some(format!("{} cannot do that", self.harness_for(harness).display_name()));
            return;
        }
        match kind {
            LaunchKind::Resume | LaunchKind::Fork => {
                self.request_launch_of_selected(kind, target)
            }
            LaunchKind::Attach => self.request_attach(target),
            LaunchKind::New | LaunchKind::ContinueLatest => {
                let Some(cwd) = self.launch_cwd() else {
                    self.notice = Some("no working directory to start in".to_owned());
                    return;
                };
                let (launch, label) = match kind {
                    LaunchKind::New => (Launch::New, "new"),
                    _ => (Launch::ContinueLatest, "continue"),
                };
                self.launch(PendingLaunch {
                    harness,
                    target,
                    launch,
                    title: format!("{label} · {}", directory_label(&cwd)),
                    cwd,
                    session_id: None,
                });
            }
        }
    }

    fn request_attach(&mut self, target: Target) {
        let Some(entry) = self.selected_entry() else {
            self.notice = Some("select a session first".to_owned());
            return;
        };
        let session_id = entry.session_id.clone();
        let Some(activity) = self.library.activity().get(&session_id).filter(|a| a.background) else {
            self.notice = Some("not a background session".to_owned());
            return;
        };
        let attach_id = activity.attach_id.clone().unwrap_or_else(|| session_id.clone());
        let cwd = PathBuf::from(&entry.cwd);
        let title = format!("attach · {}", entry.title);
        if target == Target::Embedded
            && let Some(id) = self.sessions.for_session_id(&session_id)
            && self.sessions.get(id).is_some_and(|session| session.is_running())
        {
            self.show_embedded(id);
            return;
        }
        self.launch(PendingLaunch {
            harness: HarnessId::Claude,
            target,
            launch: Launch::Attach(attach_id),
            cwd,
            title,
            session_id: Some(session_id),
        });
    }

    fn request_launch_of_selected(&mut self, kind: LaunchKind, target: Target) {
        let Some(entry) = self.selected_entry() else {
            self.notice = Some("select a session first".to_owned());
            return;
        };
        let harness = entry.harness;
        let session_id = entry.session_id.clone();
        let cwd = PathBuf::from(&entry.cwd);
        let title = entry.title.clone();
        let live_elsewhere = entry.live.as_ref().is_some_and(|live| {
            target == Target::ExternalTerminal || !self.sessions.owns_pid(live.pid)
        });

        if kind == LaunchKind::Resume
            && target == Target::Embedded
            && let Some(id) = self.sessions.for_session_id(&session_id)
            && self.sessions.get(id).is_some_and(|session| session.is_running())
        {
            self.show_embedded(id);
            return;
        }
        let pending = match kind {
            LaunchKind::Resume => PendingLaunch {
                harness,
                target,
                launch: Launch::Resume(session_id.clone()),
                cwd,
                title,
                session_id: Some(session_id),
            },
            _ => PendingLaunch {
                harness,
                target,
                launch: Launch::Fork(session_id),
                cwd,
                title: format!("fork · {title}"),
                session_id: None,
            },
        };
        if live_elsewhere {
            self.confirm = Some(Confirm::Duplicate(pending));
        } else {
            self.launch(pending);
        }
    }

    pub(super) fn launch(&mut self, pending: PendingLaunch) {
        if !pending.cwd.is_dir() {
            self.notice = Some(format!("{} does not exist", pending.cwd.display()));
            return;
        }
        let harness = self.harness_for(pending.harness);
        let Some(spec) = harness.command(&pending.launch, &pending.cwd) else {
            self.notice = Some(format!("{} cannot do that", harness.display_name()));
            return;
        };
        if pending.target == Target::ExternalTerminal {
            self.notice = Some(external::open(&spec).unwrap_or_else(|err| err));
            return;
        }
        if let Some(stale) = pending
            .session_id
            .as_deref()
            .and_then(|session_id| self.sessions.for_session_id(session_id))
        {
            self.sessions.remove(stale);
        }
        let launched = Launched {
            title: pending.title,
            session_id: pending.session_id,
            harness: pending.harness,
        };
        match self.sessions.launch(&spec, launched, self.pane_size()) {
            Ok(id) => self.show_embedded(id),
            Err(err) => self.notice = Some(format!("could not start {}: {err}", spec.program)),
        }
    }

    pub(super) fn show_embedded(&mut self, id: u64) {
        self.shown = Some(id);
        self.focus = Focus::Embedded;
        self.prefix.reset();
    }

    pub(super) fn harness_for(&self, id: HarnessId) -> &dyn Harness {
        match id {
            HarnessId::Claude => &*self.harness,
            HarnessId::Codex => &CodexCli,
            HarnessId::OpenCode => &OpenCodeCli,
        }
    }

    fn selected_harness_id(&self) -> HarnessId {
        self.selected_entry().map_or(HarnessId::Claude, |entry| entry.harness)
    }

    pub(super) fn attach_available(&self) -> bool {
        self.selected_entry().is_some_and(|entry| {
            entry.harness == HarnessId::Claude
                && self
                    .library
                    .activity()
                    .get(&entry.session_id)
                    .is_some_and(|activity| activity.background)
        })
    }

    fn launch_cwd(&self) -> Option<PathBuf> {
        if let Some(entry) = self.selected_entry() {
            return Some(PathBuf::from(&entry.cwd));
        }
        match &self.active {
            TabKey::Dir(cwd) => Some(PathBuf::from(cwd)),
            _ => std::env::current_dir().ok(),
        }
    }

    pub(super) fn answer_confirm(&mut self, confirm: Confirm, key: KeyEvent) -> bool {
        let accepted = matches!(key.code, KeyCode::Char('y' | 'Y'));
        if !accepted {
            return true;
        }
        match confirm {
            Confirm::Duplicate(pending) => self.launch(pending),
            Confirm::Close(id) => {
                self.sessions.remove(id);
                if self.shown == Some(id) {
                    self.shown = None;
                    self.focus = Focus::List;
                }
            }
            Confirm::Quit => self.should_quit = true,
        }
        true
    }

    pub(super) fn confirm_prompt(&self, confirm: &Confirm) -> String {
        match confirm {
            Confirm::Duplicate(pending) => {
                let harness = pending.harness.as_str();
                match pending.launch {
                    Launch::Fork(_) => format!("live elsewhere — fork {harness} session anyway?"),
                    _ => format!("live elsewhere — resume {harness} session anyway?"),
                }
            }
            Confirm::Close(id) => match self.sessions.get(*id) {
                Some(session) => format!("close and kill this {} session?", session.harness.as_str()),
                None => "close and kill this session?".to_owned(),
            },
            Confirm::Quit => format!(
                "{} session(s) running — quit and kill?",
                self.sessions.running_count()
            ),
        }
    }

    pub(super) fn request_quit(&mut self) {
        if self.sessions.running_count() > 0 {
            self.confirm = Some(Confirm::Quit);
        } else {
            self.should_quit = true;
        }
    }
}

fn directory_label(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| cwd.to_str().unwrap_or(""))
        .to_owned()
}
