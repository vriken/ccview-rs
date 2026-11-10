use crate::{
    harness::LaunchKind,
    tui::{
        app::{
            App, Focus,
            launch::Target,
            nav::{self, ListFocus},
        },
        input::split::Resize,
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Instant;

const PAGE_SCROLL_OVERLAP: usize = 2;

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.overlay.is_some() {
            return self.overlay_key(key);
        }
        if key.kind == KeyEventKind::Release {
            return self.forward_release(key);
        }
        if self.focus != Focus::List {
            self.list_focus = ListFocus::Rows;
        }
        if let Some(confirm) = self.confirm.take() {
            return self.answer_confirm(confirm, key);
        }
        let had_selection = self.selection.take();
        if self.focus == Focus::Embedded {
            return self.handle_embedded_key(key) || had_selection.is_some();
        }
        if let Some(selection) = had_selection {
            if key.code == KeyCode::Esc {
                return true;
            }
            self.selection = Some(selection);
        }
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self
                .ignore_ctrl_c_until
                .is_some_and(|until| Instant::now() < until)
            {
                return false;
            }
            self.quit();
            return true;
        }
        if let Some(resize) = Resize::from_key(key) {
            self.apply_resize(resize);
            return true;
        }
        if let Some(step) = nav::shift_arrow_step(&key) {
            self.switch_tab(step);
            return true;
        }
        match key.code {
            KeyCode::Char('q') => self.quit(),
            KeyCode::Char(',') => self.open_settings(),
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('r') => self.request_launch(LaunchKind::Resume, Target::Embedded),
            KeyCode::Char('f') => self.request_launch(LaunchKind::Fork, Target::Embedded),
            KeyCode::Char('n') => self.request_launch(LaunchKind::New, Target::Embedded),
            KeyCode::Char('c') => self.request_launch(LaunchKind::ContinueLatest, Target::Embedded),
            KeyCode::Char('a') => self.request_launch(LaunchKind::Attach, Target::Embedded),
            KeyCode::Char('o') => self.request_launch(LaunchKind::Resume, Target::ExternalTerminal),
            KeyCode::Char('O') => self.request_launch(LaunchKind::New, Target::ExternalTerminal),
            KeyCode::Char('s' | ' ') => self.toggle_star(),
            KeyCode::Char('i') => self.toggle_important(),
            KeyCode::Char('C') => self.cycle_selected_color(),
            KeyCode::Char('v') => self.cycle_view(),
            KeyCode::Char('d') => self.dismiss_selected(),
            KeyCode::Char('D') => self.undismiss_selected(),
            KeyCode::PageDown => self.scroll_down(self.page_size()),
            KeyCode::PageUp => self.scroll_up(self.page_size()),
            KeyCode::Char('g') => {
                self.scroll = 0;
                self.follow_bottom = false;
            }
            KeyCode::Char('G') => self.follow_bottom = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Char(digit @ '1'..='9') if !self.sessions.is_empty() => {
                self.show_strip_session(digit)
            }
            code if self.focus == Focus::List => self.handle_list_only_key(code),
            code => self.handle_transcript_only_key(code),
        }
        true
    }

    pub(in crate::tui) fn cycle_view(&mut self) {
        self.set_view_level(self.view.level.next());
    }

    pub(in crate::tui) fn handle_transcript_only_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc | KeyCode::Tab => {
                self.focus = Focus::List
            }
            _ => {}
        }
    }

    pub(in crate::tui) fn reading_focus(&self) -> Focus {
        if self.shown.is_some() {
            Focus::Embedded
        } else {
            Focus::Conversation
        }
    }

    pub(in crate::tui) fn page_size(&self) -> usize {
        self.viewport_height
            .saturating_sub(PAGE_SCROLL_OVERLAP)
            .max(1)
    }

    pub(in crate::tui) fn current_scroll(&self) -> usize {
        if self.follow_bottom {
            self.max_scroll
        } else {
            self.scroll.min(self.max_scroll)
        }
    }

    pub(in crate::tui) fn scroll_down(&mut self, amount: usize) {
        let target = self.current_scroll().saturating_add(amount);
        self.follow_bottom = target >= self.max_scroll;
        self.scroll = target.min(self.max_scroll);
    }

    pub(in crate::tui) fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.current_scroll().saturating_sub(amount);
        self.follow_bottom = false;
    }

    pub(in crate::tui) fn move_down(&mut self) {
        match self.focus {
            Focus::Conversation | Focus::Embedded => self.scroll_down(1),
            Focus::List => self.list_down(),
        }
    }

    pub(in crate::tui) fn move_up(&mut self) {
        match self.focus {
            Focus::Conversation | Focus::Embedded => self.scroll_up(1),
            Focus::List => self.list_up(),
        }
    }

    pub(in crate::tui) fn apply_resize(&mut self, resize: Resize) {
        if let Err(err) = self.split.resize(resize) {
            self.notice = Some(format!("could not save layout: {err}"));
        }
    }

    pub(in crate::tui) fn switch_tab(&mut self, step: isize) {
        let count = self.tabs.len() as isize;
        let current = self
            .tabs
            .iter()
            .position(|tab| tab.key == self.active)
            .unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(count) as usize;
        self.select_tab(next);
    }

    pub(in crate::tui) fn select_tab(&mut self, next: usize) {
        self.active = self.tabs[next].key.clone();
        self.refresh_rows();
        let first = self
            .rows
            .first()
            .map(|&i| self.catalog.entries()[i].path.clone());
        self.select_path(first);
    }

    pub(in crate::tui) fn toggle_star(&mut self) {
        if self.tab_bar_has_focus() {
            return self.toggle_star_active_dir();
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if let Err(err) = self.favorites.toggle_entry(&entry) {
            self.notice = Some(format!("could not save favorites: {err}"));
        }
        let previous = self.selected_row();
        self.refresh_rows();
        if self.selected_row().is_none() {
            let row = previous.unwrap_or(0).min(self.rows.len().saturating_sub(1));
            self.select_row(row);
        }
    }
}
