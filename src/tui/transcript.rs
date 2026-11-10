use super::{
    conversation::Conversation,
    follow::{Batch, Follower},
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

pub const SETTLE_DELAY: Duration = Duration::from_millis(120);
const CACHE_MAX_SESSIONS: usize = 6;
const CACHE_MAX_MESSAGES: usize = 6000;

struct Open {
    key: PathBuf,
    conversation: Conversation,
    messages: usize,
    follower: Option<Follower>,
    loaded: bool,
    error: Option<String>,
}

impl Open {
    fn waiting(key: PathBuf, follower: Follower) -> Self {
        Self {
            key,
            conversation: Conversation::default(),
            messages: 0,
            follower: Some(follower),
            loaded: false,
            error: None,
        }
    }

    fn drain(&mut self) -> bool {
        let Some(follower) = &self.follower else {
            return false;
        };
        let batches: Vec<Batch> = follower.batches.try_iter().collect();
        let changed = !batches.is_empty();
        for batch in batches {
            match batch {
                Batch::Replaced { messages, hidden_earlier } => {
                    self.conversation.replace_messages(&messages, hidden_earlier);
                    self.messages = messages.len();
                    self.loaded = true;
                    self.error = None;
                }
                Batch::Appended(messages) => {
                    self.conversation.push_messages(&messages);
                    self.messages += messages.len();
                }
                Batch::Failed(message) => self.error = Some(message),
            }
        }
        changed
    }

    fn ready(&self) -> bool {
        self.loaded || self.error.is_some()
    }
}

struct Cached {
    key: PathBuf,
    conversation: Conversation,
    messages: usize,
}

#[derive(Default)]
struct Lru {
    entries: VecDeque<Cached>,
}

impl Lru {
    fn put(&mut self, cached: Cached) {
        self.entries.retain(|entry| entry.key != cached.key);
        self.entries.push_front(cached);
        self.entries.truncate(CACHE_MAX_SESSIONS);
        while self.entries.len() > 1
            && self.entries.iter().map(|entry| entry.messages).sum::<usize>() > CACHE_MAX_MESSAGES
        {
            self.entries.pop_back();
        }
    }

    fn take(&mut self, key: &PathBuf) -> Option<Cached> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        self.entries.remove(index)
    }
}

pub enum View<'a> {
    Idle,
    Loading,
    Error(&'a str),
    Content(&'a mut Conversation),
}

#[derive(Default)]
pub struct Transcript {
    target: Option<PathBuf>,
    shown: Option<Open>,
    loading: Option<Open>,
    settling_since: Option<Instant>,
    content_switched: bool,
    cache: Lru,
}

impl Transcript {
    pub fn select(&mut self, target: Option<PathBuf>, now: Instant) {
        if self.target == target {
            return;
        }
        self.loading = None;
        self.settling_since = None;
        self.target = target.clone();
        let Some(key) = target else {
            self.shown = None;
            return;
        };
        if let Some(shown) = self.shown.as_mut() {
            shown.follower = None;
        }
        if self.shown.as_ref().is_none_or(|shown| shown.key != key)
            && let Some(cached) = self.cache.take(&key)
        {
            self.stash_shown();
            self.shown = Some(Open {
                key,
                conversation: cached.conversation,
                messages: cached.messages,
                follower: None,
                loaded: true,
                error: None,
            });
            self.content_switched = true;
        }
        self.settling_since = Some(now);
    }

    pub fn tick(
        &mut self,
        now: Instant,
        start: &mut dyn FnMut(&PathBuf) -> Option<Follower>,
    ) -> bool {
        let mut changed = false;
        if let Some(since) = self.settling_since
            && now.duration_since(since) >= SETTLE_DELAY
            && let Some(key) = self.target.clone()
        {
            self.settling_since = None;
            match (start(&key), self.shown.as_mut().filter(|shown| shown.key == key)) {
                (Some(follower), Some(shown)) => shown.follower = Some(follower),
                (Some(follower), None) => self.loading = Some(Open::waiting(key, follower)),
                (None, _) => self.shown = None,
            }
            changed = true;
        }
        changed | self.drain()
    }

    pub fn drain(&mut self) -> bool {
        let mut changed = self.shown.as_mut().is_some_and(Open::drain);
        if let Some(loading) = self.loading.as_mut()
            && loading.drain()
            && loading.ready()
        {
            self.stash_shown();
            self.shown = self.loading.take();
            self.content_switched = true;
            changed = true;
        }
        changed
    }

    pub fn take_content_switched(&mut self) -> bool {
        std::mem::take(&mut self.content_switched)
    }

    pub fn is_stale(&self) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|shown| Some(&shown.key) != self.target.as_ref())
    }

    pub fn view(&mut self) -> View<'_> {
        let stale = self.is_stale();
        match (&mut self.shown, &self.target) {
            (Some(shown), _) => match &shown.error {
                Some(error) if !stale => View::Error(error),
                _ => View::Content(&mut shown.conversation),
            },
            (None, Some(_)) => View::Loading,
            (None, None) => View::Idle,
        }
    }

    fn stash_shown(&mut self) {
        if let Some(shown) = self.shown.take().filter(|shown| shown.loaded && shown.error.is_none()) {
            self.cache.put(Cached {
                key: shown.key,
                conversation: shown.conversation,
                messages: shown.messages,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Block, Message, Role};
    use std::sync::mpsc::{self, Sender};

    struct Harness {
        transcript: Transcript,
        started: Vec<PathBuf>,
        senders: Vec<(PathBuf, Sender<Batch>)>,
        clock: Instant,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                transcript: Transcript::default(),
                started: Vec::new(),
                senders: Vec::new(),
                clock: Instant::now(),
            }
        }

        fn select(&mut self, name: &str) {
            self.transcript.select(Some(PathBuf::from(name)), self.clock);
        }

        fn advance(&mut self, millis: u64) -> bool {
            self.clock += Duration::from_millis(millis);
            let (started, senders) = (&mut self.started, &mut self.senders);
            self.transcript.tick(self.clock, &mut |key| {
                started.push(key.clone());
                let (sender, receiver) = mpsc::channel();
                senders.push((key.clone(), sender));
                Some(Follower::detached(receiver))
            })
        }

        fn deliver(&mut self, name: &str, text: &str) {
            let (_, sender) = self
                .senders
                .iter()
                .rev()
                .find(|(key, _)| key == &PathBuf::from(name))
                .unwrap();
            let _ = sender.send(Batch::Replaced { messages: vec![message(text)], hidden_earlier: 0 });
            self.advance(0);
        }

        fn shown_text(&mut self) -> Option<String> {
            match self.transcript.view() {
                View::Content(conversation) => Some(
                    conversation
                        .lines(80)
                        .iter()
                        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            }
        }

        fn is_stale(&mut self) -> bool {
            self.transcript.is_stale()
        }
    }

    fn message(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            timestamp: None,
            is_meta: false,
            blocks: vec![Block::Text(text.to_owned())],
        }
    }

    #[test]
    fn nothing_is_loaded_while_the_selection_keeps_moving() {
        let mut h = Harness::new();
        for row in 0..20 {
            h.select(&format!("row{row}"));
            h.advance(40);
        }
        assert!(h.started.is_empty());
        h.advance(SETTLE_DELAY.as_millis() as u64);
        assert_eq!(h.started, [PathBuf::from("row19")]);
    }

    #[test]
    fn the_previous_transcript_stays_until_the_new_one_has_content() {
        let mut h = Harness::new();
        assert!(matches!(h.transcript.view(), View::Idle));
        h.select("a");
        assert!(matches!(h.transcript.view(), View::Loading));
        h.advance(200);
        h.deliver("a", "alpha body");
        assert!(h.shown_text().unwrap().contains("alpha body"));
        assert!(!h.is_stale());

        h.select("b");
        h.advance(200);
        assert!(h.is_stale());
        assert!(h.shown_text().unwrap().contains("alpha body"));

        h.deliver("b", "beta body");
        assert!(!h.is_stale());
        assert!(h.shown_text().unwrap().contains("beta body"));
        assert!(h.transcript.take_content_switched());
    }

    #[test]
    fn a_superseded_load_is_cancelled_and_never_shown() {
        let mut h = Harness::new();
        h.select("a");
        h.advance(200);
        h.deliver("a", "alpha body");
        h.select("slow");
        h.advance(200);
        h.select("b");
        h.advance(200);
        h.deliver("slow", "slow body");
        assert!(h.shown_text().unwrap().contains("alpha body"));
        h.deliver("b", "beta body");
        assert!(h.shown_text().unwrap().contains("beta body"));
    }

    #[test]
    fn returning_to_a_cached_row_shows_it_at_once_then_revalidates_in_place() {
        let mut h = Harness::new();
        for (name, body) in [("a", "alpha body"), ("b", "beta body")] {
            h.select(name);
            h.advance(200);
            h.deliver(name, body);
        }
        h.transcript.take_content_switched();

        h.select("a");
        assert!(!h.is_stale());
        assert!(h.shown_text().unwrap().contains("alpha body"));
        assert!(h.transcript.take_content_switched());

        h.advance(200);
        assert_eq!(h.started.last(), Some(&PathBuf::from("a")));
        h.deliver("a", "alpha body updated");
        let text = h.shown_text().unwrap();
        assert!(text.contains("alpha body updated") && !text.contains("beta"));
        assert!(!h.transcript.take_content_switched());
    }

    #[test]
    fn the_cache_keeps_the_latest_sessions_within_the_session_and_message_bounds() {
        let mut lru = Lru::default();
        for index in 0..CACHE_MAX_SESSIONS + 3 {
            lru.put(Cached {
                key: PathBuf::from(format!("s{index}")),
                conversation: Conversation::default(),
                messages: 1,
            });
        }
        assert_eq!(lru.entries.len(), CACHE_MAX_SESSIONS);
        assert!(lru.take(&PathBuf::from("s0")).is_none());
        assert!(lru.take(&PathBuf::from("s8")).is_some());

        lru.put(Cached {
            key: PathBuf::from("big"),
            conversation: Conversation::default(),
            messages: CACHE_MAX_MESSAGES,
        });
        assert_eq!(lru.entries.len(), 1);
    }

    #[test]
    fn moving_the_selection_down_twenty_rows_never_blanks_or_shows_the_loading_placeholder() {
        use crate::{favorites::Favorites, library::{SessionEntry, Update}, sources::{HarnessId, TranscriptHandle}};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};
        use std::{fs, thread, time::UNIX_EPOCH};

        const ROWS: usize = 22;
        let dir = std::env::temp_dir().join(format!("ccview-sweep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let entries: Vec<SessionEntry> = (0..ROWS)
            .map(|row| {
                let path = dir.join(format!("row{row:02}.jsonl"));
                let body: String = (0..6000)
                    .map(|line| {
                        format!(
                            "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"ROW{row:02} line {line}\"}}]}}}}\n"
                        )
                    })
                    .collect();
                fs::write(&path, body).unwrap();
                SessionEntry {
                    harness: HarnessId::Claude,
                    handle: TranscriptHandle::Rollout(path.clone()),
                    path,
                    session_id: format!("row{row:02}"),
                    cwd: "/w".to_owned(),
                    title: format!("row {row:02}"),
                    last_used: UNIX_EPOCH + Duration::from_secs((ROWS - row) as u64),
                    ..SessionEntry::stub()
                }
            })
            .collect();

        let mut app = super::super::App::new(
            Favorites::load(dir.join("favorites.json")),
            None,
            mpsc::channel().0,
        );
        app.apply(Update::Scanned(entries));
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        let mut pane_text = |app: &mut super::super::App| {
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            (2..33)
                .map(|y| (50..118).map(|x| buffer[(x, y)].symbol().to_owned()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let deadline = Instant::now() + Duration::from_secs(30);
        while !pane_text(&mut app).contains("ROW00") {
            app.tick();
            assert!(Instant::now() < deadline, "first transcript never appeared");
            thread::sleep(Duration::from_millis(10));
        }

        let mut previous = pane_text(&mut app);
        let mut content_changes = 0;
        for _ in 1..ROWS {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            for _ in 0..20 {
                app.tick();
                let text = pane_text(&mut app);
                assert!(!text.contains("Loading…"), "placeholder shown:\n{text}");
                assert!(text.chars().any(|c| !c.is_whitespace() && c != '│'), "pane went empty");
                content_changes += usize::from(text != previous);
                previous = text;
                thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(previous.contains("ROW21"), "last row never shown:\n{previous}");
        assert!(content_changes <= ROWS * 2, "content changed {content_changes} times");
        drop(app);
        let deadline = Instant::now() + Duration::from_secs(10);
        while super::super::follow::LIVE_THREADS.load(std::sync::atomic::Ordering::SeqCst) != 0 {
            assert!(Instant::now() < deadline, "follower threads leaked");
            thread::sleep(Duration::from_millis(20));
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
