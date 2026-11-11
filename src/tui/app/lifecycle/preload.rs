use crate::{
    session::entry::TranscriptHandle,
    tui::{
        App,
        reader::preload::Wanted,
        widgets::tabs::{self, Marks, TabKey},
    },
};

const PRELOAD_ROWS: usize = 8;

impl App {
    pub(in crate::tui) fn refresh_preload(&mut self) {
        let entries = self.catalog.entries();
        let active = tabs::session_indices(
            &TabKey::Active,
            entries,
            &Marks {
                favorites: &self.favorites,
                important: &self.important,
                on_hold: &self.on_hold,
                order: &self.order,
            },
            self.catalog.activity(),
            self.catalog.dismissed(),
            &|entry| self.is_listed(entry),
        );
        let top_rows = self.rows.iter().copied().take(PRELOAD_ROWS);
        let wanted = active
            .into_iter()
            .chain(top_rows)
            .filter_map(|index| {
                let entry = &entries[index];
                matches!(entry.handle, TranscriptHandle::Rollout(_)).then(|| Wanted {
                    key: entry.path.clone(),
                    harness: entry.harness,
                    handle: entry.handle.clone(),
                })
            })
            .collect();
        self.transcript.preload(wanted);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        catalog::feed::Update,
        session::{
            entry::{SessionEntry, TranscriptHandle},
            live::{LiveSession, Status},
        },
        tui::app::tests::support::{app_with, screen_lines},
    };
    use std::{
        fs,
        path::PathBuf,
        thread,
        time::{Duration, Instant, UNIX_EPOCH},
    };

    const ROWS: usize = 12;

    fn entries(dir: &std::path::Path) -> Vec<SessionEntry> {
        (0..ROWS)
            .map(|row| {
                let path = dir.join(format!("row{row:02}.jsonl"));
                let body: String = (0..30)
                    .map(|line| {
                        format!(
                            "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"BODY{row:02} line {line}\"}}]}}}}\n"
                        )
                    })
                    .collect();
                fs::write(&path, body).unwrap();
                SessionEntry {
                    handle: TranscriptHandle::Rollout(path.clone()),
                    path,
                    session_id: format!("row{row:02}"),
                    cwd: "/w".to_owned(),
                    title: format!("row {row:02}"),
                    last_used: UNIX_EPOCH + Duration::from_secs((ROWS - row) as u64),
                    ..SessionEntry::stub()
                }
            })
            .collect()
    }

    #[test]
    fn an_active_session_far_down_the_list_is_ready_before_it_is_selected_and_an_idle_one_is_not() {
        let dir = std::env::temp_dir().join(format!("ccview-preload-app-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut app = app_with("preload", entries(&dir));
        app.apply(Update::Live(vec![LiveSession {
            pid: std::process::id(),
            pid_alive: true,
            session_id: Some("row11".to_owned()),
            cwd: None,
            started_at_ms: None,
            updated_at_ms: None,
            status_updated_at_ms: None,
            status: Status::Idle,
            name: None,
            name_source: None,
            version: None,
            kind: None,
            entrypoint: None,
            transcript: None,
        }]));
        let active: PathBuf = dir.join("row11.jsonl");
        let idle: PathBuf = dir.join("row10.jsonl");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.transcript.is_cached(&active) {
            app.tick();
            assert!(Instant::now() < deadline, "active session never preloaded");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.transcript.is_cached(&idle));

        for _ in 0..ROWS - 1 {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        let screen = screen_lines(&mut app, 120, 35).join("\n");
        assert!(screen.contains("BODY11"), "{screen}");
        assert!(!screen.contains("Loading…"));
        fs::remove_dir_all(dir).unwrap();
    }
}
