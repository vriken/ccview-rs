use super::{App, Focus};
use crossterm::event::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListFocus {
    #[default]
    Rows,
    Tabs,
}

impl App {
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
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Tab => self.open_selected(),
            KeyCode::Char('x') => self.toggle_hidden_selected(),
            KeyCode::Char('H') => self.toggle_show_hidden(),
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
