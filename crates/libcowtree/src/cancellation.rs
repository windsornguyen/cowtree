// Copyright (c) 2026 Windsor Nguyen

//! Callers request cancellation while the operation retains rollback ownership.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Default)]
pub struct Cancellation {
    /// Monotone cancellation request shared with the native signal handler.
    requested: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.requested.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }

    pub(crate) fn check(&self) -> crate::Result<()> {
        if self.is_cancelled() {
            return Err(crate::Error::Cancelled);
        }
        Ok(())
    }
}
