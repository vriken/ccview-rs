pub(super) mod age;
pub(super) mod colors;
pub(super) mod dismiss;
mod draw;
pub(super) mod hide;
pub(super) mod important;
mod keys;
pub(super) mod launch;
pub(super) mod nav;
mod selection;
pub(super) mod session_host;

#[cfg(test)]
mod tests;

use crate::{
    catalog::{Catalog, feed::Update},
    harness::{self, Harness},
    store::{
        favorite_dirs::FavoriteDirs, favorites::Favorites, hidden::HiddenSessions,
        important::ImportantSessions, settings::UiSettingsStore,
    },
    tui::{
        app::{launch::PendingLaunch, nav::ListFocus},
        embedded::{manager::SessionManager, prefix::Prefix},
        input::{mouse, split::SplitState},
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
}

pub struct App {
    pub(in crate::tui) catalog: Catalog,
    pub(in crate::tui) favorites: Favorites,
    pub(in crate::tui) favorite_dirs: FavoriteDirs,
    pub(in crate::tui) hidden: HiddenSessions,
    pub(in crate::tui) important: ImportantSessions,
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
    pub(in crate::tui) should_quit: bool,
    pub(in crate::tui) harness: Box<dyn Harness>,
    pub(in crate::tui) sessions: SessionManager,
    pub(in crate::tui) shown: Option<u64>,
    pub(in crate::tui) prefix: Prefix,
    pub(in crate::tui) confirm: Option<Confirm>,
    pub(in crate::tui) pane_inner: Rect,
    pub(in crate::tui) view: ViewSetting,
    pub(in crate::tui) scroll_anchor: Option<usize>,
    pub(in crate::tui) split: SplitState,
    pub(in crate::tui) regions: mouse::Regions,
    pub(in crate::tui) terminal_focused: bool,
    pub(in crate::tui) focused_child: Option<u64>,
    pub(in crate::tui) outer_output: Vec<u8>,
    pub(in crate::tui) alert: Option<(String, Instant)>,
    pub(in crate::tui) ignore_ctrl_c_until: Option<Instant>,
    pub(in crate::tui) forwarded_presses: Vec<KeyCode>,
    pub(in crate::tui) overlay: Option<overlay::Overlay>,
}

impl App {
    pub fn new(
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
            should_quit: false,
            harness: harness::default_harness(),
            sessions: SessionManager::new(wake),
            shown: None,
            prefix: Prefix::default(),
            confirm: None,
            pane_inner: Rect::default(),
            view: ViewSetting::default(),
            scroll_anchor: None,
            split: SplitState::new(UiSettingsStore::default()),
            regions: mouse::Regions::default(),
            terminal_focused: true,
            focused_child: None,
            outer_output: Vec::new(),
            alert: None,
            ignore_ctrl_c_until: None,
            forwarded_presses: Vec::new(),
            overlay: None,
        };
        app.drain_updates();
        app.rebuild_view();
        app
    }

    pub fn with_ui_settings(mut self, store: UiSettingsStore) -> Self {
        self.split = SplitState::new(store);
        self.apply_loaded_settings();
        self
    }

    pub fn with_hidden(mut self, hidden: HiddenSessions) -> Self {
        self.hidden = hidden;
        self.rebuild_view();
        self
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn output_ready(&mut self, id: u64) -> bool {
        let newly_exited = self.sessions.acknowledge_output(id);
        let events_changed = self.drain_terminal_events(id);
        let reaped = self.reap_clean_exits();
        reaped || newly_exited || events_changed || self.shown == Some(id)
    }

    pub fn handle_event(&mut self, event: Event) -> bool {
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
    pub fn apply(&mut self, update: Update) {
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
            Update::InitialScanDone => self.scanning = false,
        }
    }

    pub fn tick(&mut self) -> bool {
        let mut changed = self.reap_clean_exits();
        changed |= self.drain_updates();
        changed |= self.drain_conversation();
        changed |= self.expire_alert();
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
        changed
    }

    pub(in crate::tui) fn follow_content_switch(&mut self) {
        if self.transcript.take_content_switched() {
            self.scroll = 0;
            self.follow_bottom = true;
        }
    }
}
