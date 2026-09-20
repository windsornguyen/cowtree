// Copyright (c) 2026 Windsor Nguyen

//! Worktree failures identify the violated contract and retain native causes.

use std::{io, path::PathBuf, process::ExitStatus};

pub type WorktreeResult<T> = std::result::Result<T, WorktreeError>;

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("Git failed with {status}: {stderr}")]
    Git { status: ExitStatus, stderr: String },
    #[error("filesystem operation failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid Git response for {field:?}")]
    GitResponse { field: GitField },
    #[error("source has tracked changes or hidden index flags")]
    DirtySource,
    #[error("source HEAD changed or does not match the requested commit")]
    HeadMismatch,
    #[error("sparse checkouts are unsupported")]
    SparseCheckout,
    #[error("submodules are unsupported: {path}")]
    Submodule { path: PathBuf },
    #[error("unsupported Git file mode at {path}")]
    UnsupportedMode { path: PathBuf },
    #[error("destination already exists: {path}")]
    DestinationExists { path: PathBuf },
    #[error("branch already exists: {name:?}")]
    BranchExists { name: std::ffi::OsString },
    #[error("branch does not exist: {name:?}")]
    BranchMissing { name: std::ffi::OsString },
    #[error("invalid request: {reason}")]
    InvalidRequest { reason: crate::RequestIssue },
    #[error("clone probe violated {invariant:?}")]
    ProbeFailed { invariant: ProbeInvariant },
    #[error("created worktree is not registered: {path}")]
    WorktreeMissing { path: PathBuf },
    #[error("private seed cleanup failed at {path}. Destination may already be complete: {source}")]
    SeedCleanup { path: PathBuf, source: Box<Self> },
    #[error(transparent)]
    Native(#[from] crate::Error),
    #[error(
        "cleanup failed at {path}, branch {branch:?}: {cleanup} (original failure: {original})"
    )]
    Cleanup { path: PathBuf, branch: crate::Branch, original: Box<Self>, cleanup: Box<Self> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitField {
    Text,
    PathEncoding,
    TreeEntry,
    TreeMode,
    WorktreeKey,
    WorktreePath,
    WorktreeTerminator,
    Head,
    Branch,
    LockReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeInvariant {
    ClonedContents,
    SourceIsolation,
    TargetIsolation,
}

impl WorktreeError {
    pub(crate) fn io(path: &std::path::Path, source: io::Error) -> Self {
        Self::Io { path: path.to_path_buf(), source }
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DirtySource => "dirty_source",
            Self::Native(crate::Error::Cancelled) => "cancelled",
            Self::HeadMismatch => "head_mismatch",
            Self::SparseCheckout => "sparse_checkout",
            Self::Submodule { .. } => "submodule_unsupported",
            Self::UnsupportedMode { .. } => "unsupported_mode",
            Self::DestinationExists { .. }
            | Self::InvalidRequest { .. }
            | Self::BranchExists { .. }
            | Self::BranchMissing { .. } => "invalid_arguments",
            Self::WorktreeMissing { .. } => "worktree_not_found",
            Self::Cleanup { .. }
            | Self::SeedCleanup { .. }
            | Self::Native(crate::Error::Cleanup { .. }) => "cleanup_failed",
            Self::Native(crate::Error::Io { source, .. })
                if source.kind() == io::ErrorKind::Unsupported =>
            {
                "cow_unavailable"
            }
            Self::Native(crate::Error::Io { source, .. })
                if source.kind() == io::ErrorKind::CrossesDevices =>
            {
                "different_filesystem"
            }
            Self::Native(error) => error.code(),
            _ => "command_failed",
        }
    }
}
