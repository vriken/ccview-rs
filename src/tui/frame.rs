use super::App;
use crossterm::{
    execute,
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

const RESTART_CLEAR_WAIT: Duration = Duration::from_millis(300);

pub(in crate::tui) struct FramePainter {
    clear_pending: bool,
    give_up_at: Instant,
}

impl FramePainter {
    pub(in crate::tui) fn new(restarted: bool, now: Instant) -> Self {
        Self {
            clear_pending: restarted,
            give_up_at: now + RESTART_CLEAR_WAIT,
        }
    }

    pub(in crate::tui) fn paint<W: Write>(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<W>>,
        app: &mut App,
        now: Instant,
    ) -> io::Result<bool> {
        if self.clear_pending && app.pending_restore.is_some() && now < self.give_up_at {
            return Ok(false);
        }
        execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
        let painted = (|| {
            if self.clear_pending {
                execute!(terminal.backend_mut(), Clear(ClearType::All))?;
                self.clear_pending = false;
            }
            terminal.draw(|frame| app.draw(frame)).map(|_| true)
        })();
        execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
        painted
    }
}
