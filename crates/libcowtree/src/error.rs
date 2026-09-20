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
            Self::Cancelled => "cancelled",
            Self::InvalidSource { .. }
            | Self::InvalidTarget { .. }
            | Self::InvalidTrackedPath { .. }
            | Self::PathConflict { .. }
            | Self::InvalidPolicy { .. }
            | Self::PolicyOverlap
            | Self::TargetContainsData { .. } => "invalid_arguments",
            Self::ModeMismatch { .. } | Self::SourceChanged { .. } => "dirty_source",
            Self::Hardlink { .. } | Self::UnsupportedFile { .. } => "unsupported_mode",
            Self::UnsupportedPlatform => "cow_unavailable",
            Self::Cleanup { .. } => "cleanup_failed",
            Self::Io { source, .. } => match source.kind() {
                io::ErrorKind::Unsupported | io::ErrorKind::CrossesDevices => "cow_unavailable",
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
