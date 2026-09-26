// Copyright (c) 2026 Windsor Nguyen

//! Typed filesystem failures retain the operation, path, and operating-system cause.

use std::{io, path::PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Inspect,
    Open,
    Clone,
    Metadata,
    CreateDirectory,
    ReadLink,
    CreateLink,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot start clone worker: {0}")]
    WorkerStart(#[source] io::Error),
    #[error("operation cancelled")]
    Cancelled,
    #[error("source is not a regular file: {path}")]
    InvalidSource { path: PathBuf },
    #[error("invalid population destination: {path}")]
    InvalidTarget { path: PathBuf },
    #[error("population target contains data: {path}")]
    TargetContainsData { path: PathBuf },
    #[error("invalid tracked path: {path}")]
    InvalidTrackedPath { path: PathBuf },
    #[error("tracked file mode differs at {path}")]
    ModeMismatch { path: PathBuf },
    #[error("tracked paths conflict at {path}")]
    PathConflict { path: PathBuf },
    #[error("invalid policy prefix: {path}")]
    InvalidPolicy { path: PathBuf },
    #[error("policy prefixes overlap")]
    PolicyOverlap,
    #[error("hard-linked file: {path}")]
    Hardlink { path: PathBuf },
    #[error("unsupported file type: {path}")]
    UnsupportedFile { path: PathBuf },
    #[error("derived symlink escapes or cannot resolve within private cache: {path}")]
    DerivedLink { path: PathBuf },
    #[error("source changed during capture: {path}")]
    SourceChanged { path: PathBuf },
    #[error("native cloning is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("{operation:?} failed at {path}: {source}")]
    Io { operation: Operation, path: PathBuf, source: io::Error },
    #[error("failed clone remains at {path}: {source} (original failure: {original})")]
    Cleanup { path: PathBuf, original: Box<Self>, source: io::Error },
}

impl Error {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::WorkerStart(_) => "command_failed",
            Self::Cancelled => "cancelled",
            Self::InvalidSource { .. }
            | Self::InvalidTarget { .. }
            | Self::InvalidTrackedPath { .. }
            | Self::PathConflict { .. }
            | Self::InvalidPolicy { .. }
            | Self::PolicyOverlap
            | Self::TargetContainsData { .. } => "invalid_arguments",
            Self::ModeMismatch { .. } | Self::SourceChanged { .. } => "dirty_source",
            Self::Hardlink { .. } | Self::UnsupportedFile { .. } | Self::DerivedLink { .. } => {
                "unsupported_mode"
            }
            Self::UnsupportedPlatform => "cow_unavailable",
            Self::Cleanup { .. } => "cleanup_failed",
            Self::Io { source, .. } if clone_unavailable(source) => "cow_unavailable",
            Self::Io { source, .. } => match source.kind() {
                io::ErrorKind::NotFound
                | io::ErrorKind::AlreadyExists
                | io::ErrorKind::InvalidInput => "invalid_arguments",
                _ => "command_failed",
            },
        }
    }

    pub(crate) fn io(operation: Operation, path: &std::path::Path, source: io::Error) -> Self {
        Self::Io { operation, path: path.to_path_buf(), source }
    }
}

#[must_use]
pub(crate) fn clone_unavailable(error: &io::Error) -> bool {
    if matches!(error.kind(), io::ErrorKind::Unsupported | io::ErrorKind::CrossesDevices) {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(rustix::io::Errno::NOTTY.raw_os_error()) {
        return true;
    }
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn rejected_clone_ioctl_is_unavailable_not_an_operational_failure() {
        for errno in [rustix::io::Errno::NOTTY, rustix::io::Errno::OPNOTSUPP] {
            let error = Error::io(Operation::Clone, std::path::Path::new("target"), errno.into());
            assert_eq!(error.code(), "cow_unavailable");
        }
        let denied = Error::io(
            Operation::Clone,
            std::path::Path::new("target"),
            rustix::io::Errno::ACCESS.into(),
        );
        assert_eq!(denied.code(), "command_failed");
    }
}
