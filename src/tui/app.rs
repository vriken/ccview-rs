mod draw;
pub(super) mod flags;
mod keys;
pub(super) mod lifecycle;
pub(super) mod nav;
mod selection;

#[cfg(test)]
mod tests;

use crate::{
    catalog::{Catalog, feed::Update},
    harness::{self, Harness, browser},
    store::{
        favorite_dirs::FavoriteDirs,
        favorites::Favorites,
        hidden::HiddenSessions,
        important::ImportantSessions,
        on_hold::OnHoldSessions,
        order::ManualOrder,
        settings::UiSettingsStore,
        ui_state::{self, UiState},
    },
    terminal::HostUnreachable,
    tui::{
        app::{lifecycle::launch::PendingLaunch, nav::ListFocus},
        embedded::{manager::SessionManager, prefix::Prefix},
        input::{copy, mouse, split::SplitState},
        overlay,
        reader::{follow::Follower, selection::Transcript, verbosity::ViewSetting},
        runtime::LoopEvent,
        style::identity::Identity,
        widgets::tabs::{Tab, TabKey},
    },
};
use crossterm::event::{Event, KeyCode};
use ratatui::layout::Rect;
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
    time::Instant,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Focus {
    List,
    Conversation,
    Embedded,
}

pub(in crate::tui) enum Confirm {
    Duplicate(PendingLaunch),
    Stop(flags::stop::PendingStop),
}

pub(crate) struct App {
    pub(in crate::tui) catalog: Catalog,
    pub(in crate::tui) favorites: Favorites,
    pub(in crate::tui) favorite_dirs: FavoriteDirs,
    pub(in crate::tui) hidden: HiddenSessions,
    pub(in crate::tui) important: ImportantSessions,
    pub(in crate::tui) on_hold: OnHoldSessions,
    pub(in crate::tui) order: ManualOrder,
    pub(in crate::tui) drag: Option<flags::reorder::RowDrag>,
    pub(in crate::tui) show_hidden: bool,
    pub(in crate::tui) list_focus: ListFocus,
    pub(in crate::tui) identity: Identity,
    pub(in crate::tui) updates: Option<Receiver<Update>>,
    pub(in crate::tui) scanning: bool,
    pub(in crate::tui) tabs: Vec<Tab>,
    pub(in crate::tui) active: TabKey,
    pub(in crate::tui) rows: Vec<usize>,
    pub(in crate::tui) selected_path: Option<PathBuf>,
    pub(in crate::tui) focus: Focus,
    pub(in crate::tui) transcript: Transcript,
    pub(in crate::tui) scroll: usize,
    pub(in crate::tui) max_scroll: usize,
    pub(in crate::tui) follow_bottom: bool,
    pub(in crate::tui) viewport_height: usize,
    pub(in crate::tui) notice: Option<String>,
    pub(in crate::tui) host_unreachable: Option<HostUnreachable>,
    pub(in crate::tui) should_quit: bool,
    pub(in crate::tui) harness: Box<dyn Harness>,
    pub(in crate::tui) sessions: SessionManager,
    pub(in crate::tui) shown: Option<u64>,
    pub(in crate::tui) prefix: Prefix,
    pub(in crate::tui) confirm: Option<Confirm>,
    pub(in crate::tui) pane_inner: Rect,
    pub(in crate::tui) view: ViewSetting,
    pub(in crate::tui) scroll_anchor: Option<usize>,
    pub(in crate::tui) near_top: bool,
    pub(in crate::tui) split: SplitState,
    pub(in crate::tui) regions: mouse::Regions,
    pub(in crate::tui) terminal_focused: bool,
    pub(in crate::tui) focused_child: Option<u64>,
    pub(in crate::tui) outer_output: Vec<u8>,
    pub(in crate::tui) alert: Option<(String, Instant)>,
    pub(in crate::tui) ignore_ctrl_c_until: Option<Instant>,
    pub(in crate::tui) forwarded_presses: Vec<KeyCode>,
    pub(in crate::tui) age_link: Option<lifecycle::age::AgeLink>,
    pub(in crate::tui) overlay: Option<overlay::Overlay>,
    pub(in crate::tui) selection: Option<copy::Selection>,
    pub(in crate::tui) text_area: Rect,
    pub(in crate::tui) claude_sessions_dir: Option<PathBuf>,
    pub(in crate::tui) link_opener: Box<browser::Opener>,
    pub(in crate::tui) self_update: Option<lifecycle::self_update::Watcher>,
    pub(in crate::tui) update_notice: Option<lifecycle::self_update::Notice>,
    pub(in crate::tui) restart_requested: bool,
    pub(in crate::tui) restarted: bool,
    pub(in crate::tui) pending_restore: Option<UiState>,
    pub(in crate::tui) pending_scroll: Option<(PathBuf, usize, bool)>,
    pub(in crate::tui) ui_state_path: PathBuf,
}

impl App {
    pub(crate) fn new(
        favorites: Favorites,
        identity: Identity,
        updates: Option<Receiver<Update>>,
        wake: Sender<LoopEvent>,
    ) -> Self {
        let mut app = Self {
            catalog: Catalog::default(),
            favorites,
            favorite_dirs: FavoriteDirs::default(),
            hidden: HiddenSessions::default(),
            important: ImportantSessions::default(),
            on_hold: OnHoldSessions::default(),
            order: ManualOrder::default(),
            drag: None,
            show_hidden: false,
            list_focus: ListFocus::default(),
            identity,
            updates,
            scanning: true,
            tabs: Vec::new(),
            active: TabKey::All,
            rows: Vec::new(),
            selected_path: None,
            focus: Focus::List,
            transcript: Transcript::default(),
            scroll: 0,
            max_scroll: 0,
            follow_bottom: true,
            viewport_height: 0,
            notice: None,
            host_unreachable: None,
            should_quit: false,
            harness: harness::default_harness(),
            sessions: SessionManager::new(wake),
            shown: None,
            prefix: Prefix::default(),
            confirm: None,
            pane_inner: Rect::default(),
            view: ViewSetting::default(),
            scroll_anchor: None,
            near_top: false,
            split: SplitState::new(UiSettingsStore::default()),
            regions: mouse::Regions::default(),
            terminal_focused: true,
            focused_child: None,
            outer_output: Vec::new(),
            alert: None,
            ignore_ctrl_c_until: None,
            forwarded_presses: Vec::new(),
            age_link: None,
            overlay: None,
            selection: None,
            text_area: Rect::default(),
            claude_sessions_dir: crate::sources::claude::registry::claude_sessions_dir(),
            link_opener: Box::new(browser::system_open),
            self_update: None,
            update_notice: None,
            restart_requested: false,
            restarted: false,
            pending_restore: None,
            pending_scroll: None,
            ui_state_path: ui_state::default_path(),
        };
        app.drain_updates();
        app.rebuild_view();
        app
    }

    pub(crate) fn with_ui_settings(mut self, store: UiSettingsStore) -> Self {
        self.split = SplitState::new(store);
        self.apply_loaded_settings();
        self
    }

    pub(crate) fn with_hidden(mut self, hidden: HiddenSessions) -> Self {
        self.hidden = hidden;
        self.rebuild_view();
        self
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub(crate) fn quit(&mut self) {
        self.should_quit = true;
    }

    pub(crate) fn output_ready(&mut self, id: u64) -> bool {
        let newly_exited = self.sessions.acknowledge_output(id);
        let events_changed = self.drain_terminal_events(id);
        let reaped = self.reap_clean_exits();
        reaped || newly_exited || events_changed || self.shown == Some(id)
    }

    pub(crate) fn handle_event(&mut self, event: Event) -> bool {
        let redraw = match event {
            Event::Key(key) => self.handle_key(key),
            Event::Paste(text) => {
                self.handle_paste(&text);
                false
            }
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::FocusGained => {
                self.terminal_focused = true;
                false
            }
            Event::FocusLost => {
                self.terminal_focused = false;
                false
            }
            Event::Resize(..) => true,
        };
        self.sync_child_focus();
        redraw
    }

    #[cfg(test)]
    pub(crate) fn apply(&mut self, update: Update) {
        self.absorb(update);
        self.rebuild_view();
    }

    pub(in crate::tui) fn absorb(&mut self, update: Update) {
        match update {
            Update::Scanned(entries) => self.catalog.merge(entries),
            Update::Removed(paths) => self.catalog.remove(&paths),
            Update::Live(live) => {
                self.sync_identity();
                for (host, session_id) in self.sessions.bind_live(&live) {
                    self.identity.adopt(host, &session_id);
                }
                self.catalog.set_live(live);
            }
            Update::Activity(activity) => self.catalog.set_activity(activity),
            Update::Checkouts(keys) => self.catalog.set_checkouts(keys),
            Update::InitialScanDone => self.scanning = false,
        }
        self.release_answered_holds();
    }

    pub(crate) fn tick(&mut self) -> bool {
        self.publish_age_filter();
        let mut changed = self.reap_clean_exits();
        changed |= self.drain_updates();
        changed |= self.apply_pending_restore();
        changed |= self.drain_conversation();
        changed |= self.expire_alert();
        let state_path = self.ui_state_path.clone();
        changed |= self.check_self_update(&state_path);
        changed
    }

    pub(in crate::tui) fn drain_updates(&mut self) -> bool {
        let Some(receiver) = &self.updates else {
            return false;
        };
        let pending: Vec<Update> = receiver.try_iter().collect();
        if pending.is_empty() {
            return false;
        }
        for update in pending {
            self.absorb(update);
        }
        self.rebuild_view();
        true
    }

    pub(in crate::tui) fn drain_conversation(&mut self) -> bool {
        let entries = self.catalog.entries();
        let changed = self.transcript.tick(Instant::now(), &mut |key| {
            let entry = entries.iter().find(|entry| &entry.path == key)?;
            Some(Follower::start(entry.harness, entry.handle.clone()))
        });
        self.follow_content_switch();
        if self.near_top {
            self.transcript.request_earlier();
        }
        changed
    }

    pub(in crate::tui) fn follow_content_switch(&mut self) {
        if self.transcript.take_content_switched() {
            self.scroll = 0;
            self.follow_bottom = true;
            self.near_top = false;
            self.restore_scroll_when_ready();
        }
    }
}
