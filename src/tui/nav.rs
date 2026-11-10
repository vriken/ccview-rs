use super::{App, Focus, tabs::TabKey};
use crate::favorite_dirs::FavoriteDirs;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListFocus {
    #[default]
    Rows,
    Tabs,
}

pub(super) fn shift_arrow_step(key: &KeyEvent) -> Option<isize> {
    if key.modifiers != KeyModifiers::SHIFT {
        return None;
    }
    match key.code {
        KeyCode::Left => Some(-1),
        KeyCode::Right => Some(1),
        _ => None,
    }
}

impl App {
    pub(super) fn with_favorite_dirs(mut self, favorite_dirs: FavoriteDirs) -> Self {
        self.favorite_dirs = favorite_dirs;
        self.rebuild_view();
        self
    }

    pub(super) fn toggle_star_active_dir(&mut self) {
        if let TabKey::Dir(cwd) = self.active.clone() {
            self.toggle_dir_star(&cwd);
        }
    }

    fn toggle_star_selected_dir(&mut self) {
        if let Some(cwd) = self.selected_entry().map(|entry| entry.cwd.clone()) {
            self.toggle_dir_star(&cwd);
        }
    }

    fn toggle_dir_star(&mut self, cwd: &str) {
        if let Err(err) = self.favorite_dirs.toggle(cwd) {
            self.notice = Some(format!("could not save favorite directories: {err}"));
        }
        self.rebuild_view();
    }

    fn toggle_expand_copies(&mut self) {
        self.library.set_expanded(!self.library.expanded());
        self.rebuild_view();
    }

    pub(super) fn tab_bar_has_focus(&self) -> bool {
        self.focus == Focus::List && self.list_focus == ListFocus::Tabs
    }

    pub(super) fn list_down(&mut self) {
        match self.list_focus {
            ListFocus::Tabs => self.list_focus = ListFocus::Rows,
            ListFocus::Rows => {
                let next = self.selected_row().map_or(0, |row| row + 1);
                if next < self.rows.len() {
                    self.select_row(next);
                }
            }
        }
    }

    pub(super) fn list_up(&mut self) {
        if self.list_focus == ListFocus::Tabs {
            return;
        }
        match self.selected_row().filter(|&row| row > 0) {
            Some(row) => self.select_row(row - 1),
            None => self.list_focus = ListFocus::Tabs,
        }
    }

    pub(super) fn handle_list_only_key(&mut self, code: KeyCode) {
        match self.list_focus {
            ListFocus::Tabs => self.handle_tab_bar_key(code),
            ListFocus::Rows => self.handle_rows_key(code),
        }
    }

    fn handle_tab_bar_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h' | '[') => self.switch_tab(-1),
            KeyCode::Right | KeyCode::Char('l' | ']') => self.switch_tab(1),
            KeyCode::Enter | KeyCode::Tab | KeyCode::Esc => self.list_focus = ListFocus::Rows,
            _ => {}
        }
    }

    fn handle_rows_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Tab => {
                self.open_selected()
            }
            KeyCode::Char('x') => self.toggle_hidden_selected(),
            KeyCode::Char('H') => self.toggle_show_hidden(),
            KeyCode::Char('S') => self.toggle_star_selected_dir(),
            KeyCode::Char('E') => self.toggle_expand_copies(),
            _ => {}
        }
    }

    fn open_selected(&mut self) {
        self.focus = self.reading_focus();
        self.list_focus = ListFocus::Rows;
    }
}

#[cfg(test)]
mod tests;
