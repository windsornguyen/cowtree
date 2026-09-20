// Copyright (c) 2026 Windsor Nguyen

//! Callers request cancellation while the operation retains rollback ownership.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub struct Cancellation {
    requested: Arc<AtomicBool>,
    external: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl std::fmt::Debug for Cancellation {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("Cancellation")
            .field("requested", &self.requested.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Cancellation {
    /// Check the embedding runtime at transaction boundaries without owning its signals.
    #[must_use]
    pub fn with_check(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self { requested: Arc::default(), external: Some(Arc::new(check)) }
    }

    pub fn cancel(&self) {
        self.requested.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }

    pub(crate) fn check(&self) -> crate::Result<()> {
        if !self.is_cancelled() && self.external.as_ref().is_some_and(|check| check()) {
            self.cancel();
        }
        if self.is_cancelled() {
            return Err(crate::Error::Cancelled);
        }
        Ok(())
    }
}
