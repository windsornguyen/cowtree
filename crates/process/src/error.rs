// Copyright (c) 2026 Windsor Nguyen

//! Process failures distinguish admission, execution, and cleanup.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("validation needs a command and positive timeout")]
    InvalidRequest,
    #[error("validation supervision I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot signal validation process group: {0}")]
    Signal(#[source] rustix::io::Errno),
    #[error("cannot observe validation process: {0}")]
    Wait(#[source] rustix::io::Errno),
    #[error("child exit status is not representable")]
    ExitStatus,
    #[error("{original}; process cleanup failed: {cleanup}")]
    Cleanup {
        /// Failure that initiated process retirement.
        original: Box<Self>,
        /// Failure while terminating or reaping owned processes.
        cleanup: Box<Self>,
    },
}
