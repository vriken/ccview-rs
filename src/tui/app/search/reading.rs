use super::Searching;
use crate::{
    catalog::search::{
        handle::{Locate, Which},
        query::Query,
    },
    session::entry::TranscriptHandle,
    tui::{
        app::{App, Focus},
        bindings::{NEXT_MATCH_KEY, PREVIOUS_MATCH_KEY},
        reader::{conversation::Highlight, follow::Follower, selection::View},
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{path::PathBuf, sync::Arc};

const MATCH_MOVED: &str = "match moved";
const NO_NEXT_MATCH: &str = "no later match";
const NO_PREVIOUS_MATCH: &str = "no earlier match";

pub(super) struct Reading {
    pub(in crate::tui) highlight: Arc<Highlight>,
    text: String,
    path: PathBuf,
    current: Option<u64>,
    token: u64,
    waiting: Option<Which>,
}

impl App {
    pub(super) fn focus_result_transcript(&mut self) {
        if self.selected_path.is_none() {
            return;
        }
        self.focus = self.reading_focus();
        if self.focus == Focus::Conversation {
            self.start_reading();
        }
    }

    fn start_reading(&mut self) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        let Some(row) = self.selected_row() else {
            return;
        };
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let Some(sent) = search.sent.as_ref() else {
            return;
        };
        let query = Query::parse(&sent.text);
        let rollout = self
            .catalog
            .entries()
            .iter()
            .find(|entry| entry.path == path)
            .is_some_and(|entry| matches!(entry.handle, TranscriptHandle::Rollout(_)));
        if query.fts().is_none() || !rollout {
            return;
        }
        let offset = search
            .row_results
            .get(row)
            .and_then(|&at| search.results.as_ref()?.get(at))
            .and_then(|(_, _, offset)| *offset);
        search.reading = Some(Reading {
            highlight: Arc::new(Highlight::of(&query.terms)),
            text: sent.text.clone(),
            path,
            current: None,
            token: 0,
            waiting: None,
        });
        self.pending_reveal = None;
        if let Some(offset) = offset {
            self.send_locate(Which::At, Some(offset));
        }
    }

    pub(in crate::tui) fn reading_key(&mut self, key: KeyEvent) -> bool {
        if self.focus != Focus::Conversation
            || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        let Some(reading) = search.reading.as_ref() else {
            return false;
        };
        match key.code {
            KeyCode::Esc => {
                search.reading = None;
                self.pending_reveal = None;
            }
            KeyCode::Char(NEXT_MATCH_KEY) if reading.waiting.is_none() => {
                let from = reading.current;
                self.send_locate(Which::Next, from);
            }
            KeyCode::Char(PREVIOUS_MATCH_KEY) if reading.waiting.is_none() => {
                let from = reading.current;
                self.send_locate(Which::Previous, from);
            }
            KeyCode::Char(NEXT_MATCH_KEY | PREVIOUS_MATCH_KEY) => {}
            _ => return false,
        }
        true
    }

    fn send_locate(&mut self, which: Which, from: Option<u64>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let Searching {
            reading,
            locate_token,
            ..
        } = search;
        let (Some(reading), Some(handle)) = (reading.as_mut(), self.search_handle.as_ref()) else {
            return;
        };
        *locate_token += 1;
        reading.token = *locate_token;
        reading.waiting = Some(which);
        handle.locate(Locate {
            token: reading.token,
            text: reading.text.clone(),
            path: reading.path.clone(),
            from,
            which,
        });
    }

    pub(super) fn take_located(&mut self) -> bool {
        let Some(located) = self
            .search_handle
            .as_ref()
            .and_then(|handle| handle.located())
        else {
            return false;
        };
        let Some(reading) = self
            .search
            .as_mut()
            .and_then(|search| search.reading.as_mut())
        else {
            return false;
        };
        if located.token != reading.token {
            return false;
        }
        let Some(asked) = reading.waiting.take() else {
            return false;
        };
        match (located.intact, located.offset) {
            (false, _) => {
                self.notice = Some(MATCH_MOVED.to_owned());
                self.reload_tail();
            }
            (true, None) => {
                self.notice = Some(
                    match asked {
                        Which::Previous => NO_PREVIOUS_MATCH,
                        _ => NO_NEXT_MATCH,
                    }
                    .to_owned(),
                );
            }
            (true, Some(offset)) => {
                reading.current = Some(offset);
                self.jump_to(offset);
            }
        }
        true
    }

    fn jump_to(&mut self, offset: u64) {
        self.pending_reveal = Some(offset);
        let contained = !self.transcript.is_stale()
            && matches!(
                self.transcript.view(),
                View::Content(conversation) if conversation.contains_offset(offset)
            );
        if contained {
            return;
        }
        if let Some(entry) = self.selected_entry() {
            let follower = Follower::start_at(entry.harness, entry.handle.clone(), offset);
            self.transcript.jump(follower);
        }
    }

    fn reload_tail(&mut self) {
        self.pending_reveal = None;
        if let Some(entry) = self.selected_entry() {
            let follower = Follower::start(entry.harness, entry.handle.clone());
            self.transcript.jump(follower);
        }
    }

    pub(in crate::tui) fn settle_jump(&mut self) {
        if self.transcript.take_landed() == Some(false) {
            self.pending_reveal = None;
            self.notice = Some(MATCH_MOVED.to_owned());
        }
    }

    pub(in crate::tui) fn search_highlight(&self) -> Option<Arc<Highlight>> {
        let reading = self.search.as_ref()?.reading.as_ref()?;
        Some(Arc::clone(&reading.highlight))
    }

    pub(in crate::tui) fn search_reading(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| search.reading.is_some())
    }
}
