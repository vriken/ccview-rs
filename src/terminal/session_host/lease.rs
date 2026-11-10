use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const NO_TOKEN: u64 = 0;

#[derive(Default)]
pub(super) struct Lease {
    holder: AtomicU64,
    issued: AtomicU64,
}

impl Lease {
    fn next_token(&self) -> u64 {
        self.issued.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub(super) fn take(&self) -> u64 {
        let token = self.next_token();
        self.holder.store(token, Ordering::Release);
        token
    }

    pub(super) fn take_if_free(&self) -> u64 {
        let token = self.next_token();
        match self
            .holder
            .compare_exchange(NO_TOKEN, token, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => token,
            Err(_) => NO_TOKEN,
        }
    }

    pub(super) fn holds(&self, token: u64) -> bool {
        token != NO_TOKEN && self.holder.load(Ordering::Acquire) == token
    }

    pub(super) fn release(&self, token: u64) {
        let _ = self
            .holder
            .compare_exchange(token, NO_TOKEN, Ordering::AcqRel, Ordering::Acquire);
    }
}
