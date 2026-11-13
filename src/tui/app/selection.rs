use crate::{
    session::entry::SessionEntry,
    tui::{
        app::{App, Focus},
        widgets::tabs::{self, Marks, TabKey},
    },
};
use std::{path::PathBuf, time::Instant};

impl App {
    pub(crate) fn selected_entry(&self) -> Option<&SessionEntry> {
        let path = self.selected_path.as_ref()?;
        self.catalog
            .entries()
            .iter()
            .find(|entry| &entry.path == path)
    }

    pub(in crate::tui) fn rebuild_view(&mut self) {
        let previous_row = self.selected_row();
        let previous_tab = self.active.clone();
        self.tabs = tabs::build_tabs(
            self.catalog.entries(),
            self.catalog.activity(),
            self.catalog.dismissed(),
            &self.favorite_dirs,
            &|entry| self.is_listed(entry),
        );
        if !self.tabs.iter().any(|tab| tab.key == self.active) {
            self.active = TabKey::All;
        }
        self.refresh_rows();
        self.keep_selection(previous_row, &previous_tab);
        self.mark_search_stale();
    }

    pub(in crate::tui) fn keep_selection(
        &mut self,
        previous_row: Option<usize>,
        previous_tab: &TabKey,
    ) {
        let still_present = self.selected_path.as_ref().is_some_and(|path| {
            self.rows
                .iter()
                .any(|&i| &self.catalog.entries()[i].path == path)
        });
        if !still_present {
            let neighbour = previous_row.filter(|_| &self.active == previous_tab);
            let target = neighbour
                .unwrap_or(0)
                .min(self.rows.len().saturating_sub(1));
            self.select_row(target);
        }
    }

    pub(in crate::tui) fn refresh_rows(&mut self) {
        self.rows = self.search_row_indices().unwrap_or_else(|| {
            tabs::session_indices(
                &self.active,
                self.catalog.entries(),
                &Marks {
                    favorites: &self.favorites,
                    important: &self.important,
                    on_hold: &self.on_hold,
                    order: &self.order,
                },
                self.catalog.activity(),
                self.catalog.dismissed(),
                &|entry| self.is_listed(entry),
            )
        });
        self.refresh_preload();
    }

    pub(in crate::tui) fn selected_row(&self) -> Option<usize> {
        let path = self.selected_path.as_ref()?;
        self.rows
            .iter()
            .position(|&i| &self.catalog.entries()[i].path == path)
    }

    pub(in crate::tui) fn select_row(&mut self, row: usize) {
        let path = self
            .rows
            .get(row)
            .map(|&i| self.catalog.entries()[i].path.clone());
        self.select_path(path);
    }

    pub(in crate::tui) fn select_path(&mut self, path: Option<PathBuf>) {
        if self.selected_path == path {
            return;
        }
        self.selected_path = path.clone();
        self.selection = None;
        if self.focus != Focus::Embedded {
            self.shown = path
                .as_ref()
                .and_then(|path| {
                    self.catalog
                        .entries()
                        .iter()
                        .find(|entry| &entry.path == path)
                })
                .and_then(|entry| self.sessions.for_session_id(&entry.session_id));
            if self.shown.is_some() && self.focus == Focus::Conversation {
                self.focus = Focus::Embedded;
            }
        }
        self.transcript.select(path, Instant::now());
        self.follow_content_switch();
    }
}
