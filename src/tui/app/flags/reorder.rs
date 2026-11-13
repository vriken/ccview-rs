use crate::{
    store::order::ManualOrder,
    tui::{
        app::App,
        input::mouse::{self, Zone},
        widgets::list::DragView,
    },
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

pub(in crate::tui) struct RowDrag {
    session_id: String,
    press_row: u16,
    offset: usize,
    slot: usize,
    moved: bool,
}

impl App {
    pub(crate) fn with_order(mut self, order: ManualOrder) -> Self {
        self.order = order;
        self.refresh_rows();
        self
    }

    pub(in crate::tui) fn drag_view(&self) -> Option<DragView> {
        let drag = self.drag.as_ref().filter(|drag| drag.moved)?;
        Some(DragView {
            row: self.row_of(&drag.session_id)?,
            slot: drag.slot,
            offset: drag.offset,
        })
    }

    pub(in crate::tui) fn reorder_with_mouse(&mut self, event: MouseEvent) -> Option<bool> {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.begin_drag(event);
                None
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag.is_some() => {
                Some(self.drag_to(event))
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag.is_some() => {
                Some(self.drop_dragged_row())
            }
            _ => None,
        }
    }

    pub(in crate::tui) fn move_selected_row(&mut self, step: isize) {
        let Some(from) = self.selected_row() else {
            return;
        };
        let Some(to) = from
            .checked_add_signed(step)
            .filter(|&to| to < self.rows.len())
        else {
            return;
        };
        let mut ids = self.visible_ids();
        ids.swap(from, to);
        self.save_order(ids);
    }

    pub(in crate::tui) fn reset_order(&mut self) {
        if let Err(err) = self.order.clear(&self.active.storage_key()) {
            self.notice = Some(format!("could not save order: {err}"));
            return;
        }
        self.refresh_rows();
        self.notice = Some("order reset".to_owned());
    }

    fn begin_drag(&mut self, event: MouseEvent) {
        self.drag = None;
        if self.search.is_some()
            || mouse::zone_at(&self.regions, event.column, event.row) != Zone::List
        {
            return;
        }
        let list = self.regions.list;
        let Some(index) = mouse::list_row_at(list, self.selected_row(), self.rows.len(), event.row)
        else {
            return;
        };
        self.drag = Some(RowDrag {
            session_id: self.session_id_at(index),
            press_row: event.row,
            offset: mouse::list_offset(list, Some(index)),
            slot: index,
            moved: false,
        });
    }

    fn drag_to(&mut self, event: MouseEvent) -> bool {
        let Some(drag) = &self.drag else {
            return false;
        };
        let Some(from) = self.row_of(&drag.session_id) else {
            self.drag = None;
            return true;
        };
        let list = self.regions.list;
        let geometry = mouse::list_geometry(list);
        let count = self.rows.len();
        let mut offset = drag.offset;
        if event.row <= geometry.top && offset > 0 {
            offset -= 1;
        } else if event.row + 1 >= geometry.top + geometry.height
            && offset + geometry.visible_rows < count
        {
            offset += 1;
        }
        let last_visible = (offset + geometry.visible_rows)
            .min(count)
            .saturating_sub(1);
        let index = if event.row < geometry.top {
            offset
        } else {
            mouse::row_at_offset(list, offset, count, event.row).unwrap_or(last_visible)
        };
        let slot = match index.cmp(&from) {
            std::cmp::Ordering::Less => index,
            std::cmp::Ordering::Equal => from,
            std::cmp::Ordering::Greater => index + 1,
        };
        if let Some(drag) = &mut self.drag {
            drag.offset = offset;
            drag.slot = slot;
            drag.moved |= event.row != drag.press_row;
        }
        true
    }

    fn drop_dragged_row(&mut self) -> bool {
        let Some(drag) = self.drag.take().filter(|drag| drag.moved) else {
            return false;
        };
        let Some(from) = self.row_of(&drag.session_id) else {
            return true;
        };
        if drag.slot == from || drag.slot == from + 1 {
            return true;
        }
        let mut ids = self.visible_ids();
        let id = ids.remove(from);
        let target = if drag.slot > from {
            drag.slot - 1
        } else {
            drag.slot
        };
        ids.insert(target, id);
        self.save_order(ids);
        true
    }

    fn save_order(&mut self, mut ids: Vec<String>) {
        let key = self.active.storage_key();
        let absent: Vec<String> = self
            .order
            .ids(&key)
            .iter()
            .filter(|id| !ids.contains(id))
            .cloned()
            .collect();
        ids.extend(absent);
        if let Err(err) = self.order.set(&key, ids) {
            self.notice = Some(format!("could not save order: {err}"));
        }
        self.refresh_rows();
    }

    fn visible_ids(&self) -> Vec<String> {
        (0..self.rows.len())
            .map(|row| self.session_id_at(row))
            .collect()
    }

    fn session_id_at(&self, row: usize) -> String {
        self.catalog.entries()[self.rows[row]].session_id.clone()
    }

    fn row_of(&self, session_id: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|&i| self.catalog.entries()[i].session_id == session_id)
    }
}

#[cfg(test)]
mod tests;
