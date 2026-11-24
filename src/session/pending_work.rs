use serde::{Deserialize, Serialize};

const MAX_OPEN_TASKS: usize = 256;
const WAKEUP_GRACE_MS: u64 = 120_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingWork {
    open_tasks: Vec<(String, u64)>,
    crons_created_ms: Vec<u64>,
    crons_deleted_ms: Vec<u64>,
    wakeup_due_ms: Option<u64>,
}

impl PendingWork {
    pub(crate) fn open_task(&mut self, tool_use_id: &str, at_ms: u64) {
        if self.open_tasks.iter().any(|(id, _)| id == tool_use_id) {
            return;
        }
        if self.open_tasks.len() == MAX_OPEN_TASKS {
            self.open_tasks.remove(0);
        }
        self.open_tasks.push((tool_use_id.to_owned(), at_ms));
    }

    pub(crate) fn close_task(&mut self, tool_use_id: &str) {
        self.open_tasks.retain(|(id, _)| id != tool_use_id);
    }

    pub(crate) fn cron_created(&mut self, at_ms: u64) {
        self.crons_created_ms.push(at_ms);
    }

    pub(crate) fn cron_deleted(&mut self, at_ms: u64) {
        self.crons_deleted_ms.push(at_ms);
    }

    pub(crate) fn set_wakeup(&mut self, due_ms: Option<u64>) {
        self.wakeup_due_ms = due_ms;
    }

    pub(crate) fn blocks_restart(&self, launched_at_ms: u64, now_ms: u64) -> bool {
        let since_launch = |times: &[u64]| times.iter().filter(|&&at| at >= launched_at_ms).count();
        self.open_tasks.iter().any(|&(_, at)| at >= launched_at_ms)
            || since_launch(&self.crons_created_ms) > since_launch(&self.crons_deleted_ms)
            || self.wakeup_due_ms.is_some_and(|due| {
                due >= launched_at_ms && due.saturating_add(WAKEUP_GRACE_MS) > now_ms
            })
    }
}
