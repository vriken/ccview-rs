use crate::tui::{
    app::App,
    input::mouse::Detail,
    outer_terminal,
    reader::{selection::View, verbosity::Level},
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Frame, buffer::Buffer, layout::Rect, style::Modifier, text::Line};
use unicode_width::UnicodeWidthChar;

const CLIPBOARD_TARGET: &str = "c";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Transcript { width: usize, level: Level },
    Child,
}

#[derive(Debug, Clone, Copy)]
pub struct Selection {
    target: Target,
    anchor: Cell,
    head: Cell,
    dragging: bool,
    moved: bool,
}

impl Selection {
    fn ordered(&self) -> (Cell, Cell) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

fn relative_cell(area: Rect, column: u16, row: u16) -> (usize, usize) {
    let col = column
        .saturating_sub(area.x)
        .min(area.width.saturating_sub(1));
    let row = row
        .saturating_sub(area.y)
        .min(area.height.saturating_sub(1));
    (usize::from(row), usize::from(col))
}

fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn slice_columns(text: &str, from: usize, to: usize) -> String {
    let mut column = 0;
    let mut slice = String::new();
    for c in text.chars() {
        if column > to {
            break;
        }
        if column >= from {
            slice.push(c);
        }
        column += c.width().unwrap_or(0);
    }
    slice
}

fn join_trimmed(lines: impl Iterator<Item = String>) -> String {
    let joined = lines
        .map(|line| line.trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    joined.trim_end().to_owned()
}

pub fn transcript_text(lines: &[Line], start: Cell, end: Cell) -> String {
    let Some(last) = lines.len().checked_sub(1) else {
        return String::new();
    };
    let (first_row, last_row) = (start.row.min(last), end.row.min(last));
    join_trimmed((first_row..=last_row).map(|row| {
        let from = if row == start.row { start.col } else { 0 };
        let to = if row == end.row { end.col } else { usize::MAX };
        slice_columns(&line_text(&lines[row]), from, to)
    }))
}

fn paint_row(buffer: &mut Buffer, area: Rect, row: usize, from: usize, to: usize) {
    let y = area.y + row as u16;
    let last_filled = (0..area.width).rev().find(|&x| {
        buffer
            .cell((area.x + x, y))
            .is_some_and(|cell| cell.symbol() != " ")
    });
    let Some(last_filled) = last_filled else {
        return;
    };
    let to = to.min(usize::from(last_filled));
    for col in from..=to {
        if let Some(cell) = buffer.cell_mut((area.x + col as u16, y)) {
            cell.modifier.insert(Modifier::REVERSED);
        }
    }
}

fn paint_rows(buffer: &mut Buffer, area: Rect, first_visible: usize, (start, end): (Cell, Cell)) {
    for row in 0..usize::from(area.height) {
        let absolute = first_visible + row;
        if absolute < start.row || absolute > end.row {
            continue;
        }
        let from = if absolute == start.row { start.col } else { 0 };
        let to = if absolute == end.row {
            end.col
        } else {
            usize::MAX
        };
        paint_row(buffer, area, row, from, to);
    }
}

impl App {
    pub(in crate::tui) fn select_with_mouse(&mut self, event: MouseEvent) -> Option<bool> {
        let (target, area) = self.selectable()?;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = None;
                if !contains(area, event.column, event.row) {
                    return None;
                }
                let cell = self.selection_cell(target, area, event);
                self.selection = Some(Selection {
                    target,
                    anchor: cell,
                    head: cell,
                    dragging: true,
                    moved: false,
                });
                self.focus_detail();
                Some(true)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let selection = self.selection.filter(|selection| selection.dragging)?;
                let head = self.selection_cell(selection.target, area, event);
                self.selection = Some(Selection {
                    head,
                    moved: true,
                    ..selection
                });
                Some(true)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let selection = self.selection.filter(|selection| selection.dragging)?;
                if selection.anchor == selection.head {
                    self.selection = None;
                    let released = self.selection_cell(selection.target, area, event);
                    if !selection.moved && released == selection.anchor {
                        match selection.target {
                            Target::Transcript { width, level } => {
                                self.open_link_at(width, level, released);
                            }
                            Target::Child => self.open_child_link_at(released),
                        }
                    }
                } else {
                    self.selection = Some(Selection {
                        dragging: false,
                        ..selection
                    });
                    self.copy_selection(selection);
                }
                Some(true)
            }
            _ => None,
        }
    }

    pub(in crate::tui) fn clear_selection_on_press(&mut self, event: MouseEvent) -> bool {
        let pressed = event.kind == MouseEventKind::Down(MouseButton::Left);
        pressed && self.selection.take().is_some()
    }

    fn selectable(&self) -> Option<(Target, Rect)> {
        match self.detail_kind() {
            Detail::Transcript => Some((
                Target::Transcript {
                    width: self.text_area.width as usize,
                    level: self.view.level,
                },
                self.text_area,
            )),
            Detail::Child {
                reporting: false, ..
            } => Some((Target::Child, self.pane_inner)),
            Detail::Child { .. } => None,
        }
    }

    fn selection_cell(&self, target: Target, area: Rect, event: MouseEvent) -> Cell {
        let (row, col) = relative_cell(area, event.column, event.row);
        let row = match target {
            Target::Transcript { .. } => self.current_scroll() + row,
            Target::Child => row,
        };
        Cell { row, col }
    }

    fn copy_selection(&mut self, selection: Selection) {
        let (start, end) = selection.ordered();
        let text = match selection.target {
            Target::Transcript { width, level } => match self.transcript.view() {
                View::Content(conversation) => {
                    transcript_text(conversation.lines(width, level), start, end)
                }
                _ => String::new(),
            },
            Target::Child => self.child_text(start, end),
        };
        self.copy_to_clipboard(&text);
    }

    fn child_text(&self, start: Cell, end: Cell) -> String {
        let Some(session) = self.running_shown() else {
            return String::new();
        };
        let snapshot = session.host().snapshot();
        let screen = snapshot.screen();
        let (_, columns) = screen.size();
        let end_column = (end.col + 1).min(usize::from(columns.get()));
        let text = screen.contents_between(
            start.row as u16,
            start.col as u16,
            end.row as u16,
            end_column as u16,
        );
        join_trimmed(text.lines().map(str::to_owned))
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match outer_terminal::clipboard_sequence(CLIPBOARD_TARGET, text.as_bytes()) {
            Some(sequence) => {
                self.outer_output.extend(sequence);
                self.show_alert(&format!("copied {} characters", text.chars().count()));
            }
            None => self.show_alert("selection too large to copy"),
        }
    }

    pub(in crate::tui) fn keep_selection_for(&mut self, expected: Option<Target>) {
        if self
            .selection
            .is_some_and(|selection| Some(selection.target) != expected)
        {
            self.selection = None;
        }
    }

    pub(in crate::tui) fn paint_transcript_selection(&self, frame: &mut Frame) {
        if let Some(selection) = self.selection {
            paint_rows(
                frame.buffer_mut(),
                self.text_area,
                self.current_scroll(),
                selection.ordered(),
            );
        }
    }

    pub(in crate::tui) fn paint_child_selection(&self, frame: &mut Frame) {
        if let Some(selection) = self.selection {
            paint_rows(frame.buffer_mut(), self.pane_inner, 0, selection.ordered());
        }
    }
}
