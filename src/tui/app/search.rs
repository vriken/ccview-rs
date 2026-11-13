use crate::{
    catalog::search::{
        IndexProgress,
        handle::SearchHandle,
        query::{MAX_INPUT_CHARS, Query},
        rank::{SnapshotEntry, Tier},
        snippet::matched_spans,
    },
    session::{cursor::Cursor, entry::SessionEntry},
    tui::{
        app::{App, Focus, nav::ListFocus},
        widgets::search::{ResultLine, SearchView},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

mod reading;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(80);
const REQUERY_WHILE_BUILDING: Duration = Duration::from_secs(1);

#[derive(Default)]
struct QueryLine {
    text: String,
    cursor: usize,
}

impl QueryLine {
    fn byte_at(&self, cursor: usize) -> usize {
        self.text
            .char_indices()
            .nth(cursor)
            .map_or(self.text.len(), |(index, _)| index)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn insert(&mut self, text: &str) {
        for letter in text.chars() {
            if self.len() >= MAX_INPUT_CHARS {
                return;
            }
            let at = self.byte_at(self.cursor);
            self.text.insert(at, letter);
            self.cursor += 1;
        }
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let at = self.byte_at(self.cursor);
        self.text.remove(at);
    }

    fn delete(&mut self) {
        let at = self.byte_at(self.cursor);
        if at < self.text.len() {
            self.text.remove(at);
        }
    }

    fn delete_word(&mut self) {
        let letters: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor;
        while start > 0 && letters[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !letters[start - 1].is_whitespace() {
            start -= 1;
        }
        let from = self.byte_at(start);
        let to = self.byte_at(self.cursor);
        self.text.replace_range(from..to, "");
        self.cursor = start;
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

struct Saved {
    selected: Option<PathBuf>,
    focus: Focus,
    list_focus: ListFocus,
}

struct Sent {
    text: String,
    snapshot: Arc<[SnapshotEntry]>,
}

pub(in crate::tui) struct Searching {
    line: QueryLine,
    saved: Saved,
    results: Option<Vec<(String, ResultLine, Option<u64>)>>,
    reading: Option<reading::Reading>,
    locate_token: u64,
    row_results: Vec<usize>,
    generation: u64,
    sent: Option<Sent>,
    pending_since: Option<Instant>,
    edited: bool,
    forced: bool,
    reset_for: Option<u64>,
    last_sent_at: Instant,
    was_building: bool,
    shown_progress: Option<(usize, usize)>,
    body_unavailable: bool,
}

impl App {
    pub(crate) fn with_search(
        mut self,
        handle: SearchHandle,
        progress: Arc<IndexProgress>,
    ) -> Self {
        self.search_handle = Some(handle);
        self.index_progress = progress;
        self
    }

    #[cfg(test)]
    pub(in crate::tui) fn search_text(&self) -> &str {
        self.search
            .as_ref()
            .map_or("", |search| search.line.text.as_str())
    }

    pub(in crate::tui) fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some(Searching {
                line: QueryLine::default(),
                saved: Saved {
                    selected: self.selected_path.clone(),
                    focus: self.focus,
                    list_focus: self.list_focus,
                },
                results: None,
                reading: None,
                locate_token: 0,
                row_results: Vec::new(),
                generation: 0,
                sent: None,
                pending_since: None,
                edited: false,
                forced: false,
                reset_for: None,
                last_sent_at: Instant::now(),
                was_building: self.index_progress.building().is_some(),
                shown_progress: None,
                body_unavailable: false,
            });
        }
        self.focus = Focus::List;
        self.list_focus = ListFocus::Rows;
    }

    pub(in crate::tui) fn close_search(&mut self) {
        let Some(search) = self.search.take() else {
            return;
        };
        let Saved {
            selected,
            focus,
            list_focus,
        } = search.saved;
        let previous_row = self.selected_row();
        self.refresh_rows();
        let restorable = selected.filter(|path| {
            self.rows
                .iter()
                .any(|&index| &self.catalog.entries()[index].path == path)
        });
        match restorable {
            Some(path) => self.select_path(Some(path)),
            None => {
                let tab = self.active.clone();
                self.keep_selection(previous_row, &tab);
            }
        }
        self.focus = match focus {
            Focus::Conversation => self.reading_focus(),
            other => other,
        };
        self.list_focus = list_focus;
    }

    pub(in crate::tui) fn search_key(&mut self, key: KeyEvent) -> bool {
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let line = &mut search.line;
        let before = line.text.clone();
        match key.code {
            KeyCode::Esc => self.close_search(),
            KeyCode::Enter | KeyCode::Tab => self.focus_result_transcript(),
            KeyCode::Right if line.cursor >= line.len() => self.focus_result_transcript(),
            KeyCode::Right => line.cursor += 1,
            KeyCode::Left => line.cursor = line.cursor.saturating_sub(1),
            KeyCode::Home => line.cursor = 0,
            KeyCode::End => line.cursor = line.len(),
            KeyCode::Up => self.move_result(-1),
            KeyCode::Down => self.move_result(1),
            KeyCode::Backspace => line.backspace(),
            KeyCode::Delete => line.delete(),
            KeyCode::Char('u') if control => line.clear(),
            KeyCode::Char('w') if control => line.delete_word(),
            KeyCode::Char(letter) if !control && !alt => {
                line.insert(letter.encode_utf8(&mut [0; 4]))
            }
            _ => {}
        }
        self.after_query_edit(&before);
        true
    }

    pub(in crate::tui) fn search_paste(&mut self, text: &str) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let before = search.line.text.clone();
        let flattened: String = text
            .chars()
            .map(|letter| if letter.is_control() { ' ' } else { letter })
            .collect();
        search.line.insert(&flattened);
        self.after_query_edit(&before);
    }

    fn after_query_edit(&mut self, before: &str) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.line.text == before {
            return;
        }
        search.reading = None;
        self.pending_reveal = None;
        if search.line.is_blank() {
            search.results = None;
            search.pending_since = None;
            search.edited = false;
            let previous_row = self.selected_row();
            self.refresh_rows();
            let tab = self.active.clone();
            self.keep_selection(previous_row, &tab);
        } else {
            search.edited = true;
            search.pending_since = Some(Instant::now());
        }
    }

    fn move_result(&mut self, step: isize) {
        let Some(row) = self.selected_row() else {
            if step > 0 {
                self.select_row(0);
            }
            return;
        };
        if let Some(target) = row
            .checked_add_signed(step)
            .filter(|&target| target < self.rows.len())
        {
            self.select_row(target);
        }
    }

    pub(in crate::tui) fn mark_search_stale(&mut self) {
        if let Some(search) = self.search.as_mut()
            && !search.line.is_blank()
        {
            search.pending_since.get_or_insert_with(Instant::now);
        }
    }

    pub(in crate::tui) fn pump_search(&mut self) -> bool {
        if self.search.is_none() {
            return false;
        }
        let mut changed = self.watch_index();
        changed |= self.take_response();
        changed |= self.take_located();
        self.send_due_query();
        changed
    }

    fn watch_index(&mut self) -> bool {
        let building = self.index_progress.building();
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        let finished = search.was_building && building.is_none();
        search.was_building = building.is_some();
        let refreshing =
            building.is_some() && search.last_sent_at.elapsed() >= REQUERY_WHILE_BUILDING;
        if (finished || refreshing) && !search.line.is_blank() {
            search.forced = true;
            search.pending_since.get_or_insert_with(Instant::now);
        }
        let changed = search.shown_progress != building;
        search.shown_progress = building;
        changed
    }

    fn take_response(&mut self) -> bool {
        let Some(response) = self.search_handle.as_ref().and_then(SearchHandle::latest) else {
            return false;
        };
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        let Some(sent) = search.sent.as_ref() else {
            return false;
        };
        if response.generation != search.generation || search.line.is_blank() {
            return false;
        }
        let query = Query::parse(&sent.text);
        let by_id: HashMap<&str, &SessionEntry> = self
            .catalog
            .entries()
            .iter()
            .map(|entry| (entry.session_id.as_str(), entry))
            .collect();
        search.body_unavailable = !response.body_searched && query.fts().is_some();
        search.results = Some(
            response
                .results
                .into_iter()
                .map(|result| {
                    let title_spans = by_id
                        .get(result.session_id.as_str())
                        .filter(|_| result.tier == Tier::Name)
                        .map(|entry| matched_spans(&entry.title, &query.terms))
                        .unwrap_or_default();
                    let line = ResultLine {
                        hits: (result.tier == Tier::Body).then_some(result.matching_messages),
                        snippet: result.snippet,
                        title_spans,
                    };
                    let offset = result
                        .hit
                        .filter(|hit| hit.session_id == result.session_id)
                        .and_then(|hit| match hit.at {
                            Cursor::Offset(offset) => Some(offset),
                            Cursor::Row { .. } => None,
                        });
                    (result.session_id, line, offset)
                })
                .collect(),
        );
        let reset = search.reset_for.take() == Some(response.generation);
        let previous_row = self.selected_row();
        self.refresh_rows();
        if reset {
            self.select_row(0);
        } else {
            let tab = self.active.clone();
            self.keep_selection(previous_row, &tab);
        }
        true
    }

    fn send_due_query(&mut self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let due = search
            .pending_since
            .is_some_and(|since| since.elapsed() >= DEBOUNCE);
        if !due {
            return;
        }
        let text = search.line.text.clone();
        let snapshot = self.search_snapshot();
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.pending_since = None;
        if search.line.is_blank() {
            return;
        }
        let unchanged = search
            .sent
            .as_ref()
            .is_some_and(|sent| sent.text == text && *sent.snapshot == *snapshot);
        if unchanged && !search.edited && !search.forced {
            return;
        }
        search.generation += 1;
        search.reset_for = search.edited.then_some(search.generation);
        search.edited = false;
        search.forced = false;
        search.last_sent_at = Instant::now();
        search.sent = Some(Sent {
            text: text.clone(),
            snapshot: Arc::clone(&snapshot),
        });
        if let Some(handle) = &self.search_handle {
            handle.query(text, snapshot, search.generation);
        }
    }

    fn search_snapshot(&self) -> Arc<[SnapshotEntry]> {
        self.catalog
            .entries()
            .iter()
            .filter(|entry| {
                let group = entry.copy_of.as_deref().unwrap_or(&entry.session_id);
                self.show_hidden || !self.hidden.contains(group)
            })
            .map(SnapshotEntry::from)
            .collect()
    }

    pub(in crate::tui) fn search_row_indices(&mut self) -> Option<Vec<usize>> {
        let search = self.search.as_ref()?;
        let results = search.results.as_ref().filter(|_| !search.line.is_blank());
        let Some(results) = results else {
            if let Some(search) = self.search.as_mut() {
                search.row_results.clear();
            }
            return None;
        };
        let entries = self.catalog.entries();
        let positions: HashMap<&str, usize> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.session_id.as_str(), index))
            .collect();
        let (rows, row_results): (Vec<usize>, Vec<usize>) = results
            .iter()
            .enumerate()
            .filter_map(|(at, (session_id, _, _))| {
                let index = *positions.get(session_id.as_str())?;
                self.is_listed(&entries[index]).then_some((index, at))
            })
            .unzip();
        if let Some(search) = self.search.as_mut() {
            search.row_results = row_results;
        }
        Some(rows)
    }

    pub(in crate::tui) fn search_view(&self) -> Option<SearchView<'_>> {
        let search = self.search.as_ref()?;
        let results = search.results.as_deref().unwrap_or_default();
        let sessions = match self.rows.len() {
            1 => "1 session".to_owned(),
            count => format!("{count} sessions"),
        };
        let status = match self.index_progress.building() {
            Some((_, 0)) => format!("{sessions} · indexing"),
            Some((done, total)) => format!("{sessions} · indexing {done}/{total}"),
            None if search.body_unavailable => format!("{sessions} · index unavailable"),
            None => sessions,
        };
        Some(SearchView {
            text: &search.line.text,
            cursor: search.line.cursor,
            input_focused: self.focus == Focus::List,
            status,
            lines: search
                .row_results
                .iter()
                .map(|&at| &results[at].1)
                .collect(),
        })
    }

    pub(in crate::tui) fn search_empty_message(&self) -> Option<&'static str> {
        let search = self.search.as_ref()?;
        if search.line.is_blank() {
            return None;
        }
        Some(if search.results.is_some() {
            "No matches."
        } else {
            "Searching…"
        })
    }
}
