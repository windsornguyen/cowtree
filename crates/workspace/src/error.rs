// Copyright (c) 2026 Windsor Nguyen

//! Workspace failures preserve the owning operation and its original error source.

use std::{
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("candidate check exited {status}; {log}")]
    Check {
        /// Child termination status returned by the supervisor.
        status: ExitStatus,
        /// Retained diagnostic log.
        log: PathBuf,
    },
    #[error("{original}; cleanup failed: {cleanup}")]
    Cleanup {
        /// Failure that initiated rollback.
        original: Box<Error>,
        /// Failure preserving unresolved owned state.
        cleanup: Box<Error>,
    },
    #[error("{issue}: {path}")]
    State {
        /// Violated workspace contract.
        issue: Issue,
        /// Owned resource at the failed boundary.
        path: PathBuf,
    },
    #[error("filesystem operation at {path}: {source}")]
    Io {
        /// Filesystem entry used by the failed operation.
        path: PathBuf,
        /// Original operating-system failure.
        #[source]
        source: io::Error,
    },
    #[error("invalid record at {path}: {source}")]
    Record {
        /// Durable record that could not be decoded or encoded.
        path: PathBuf,
        /// Exact serialization failure.
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Git(#[from] cowtree_git::Error),
    #[error("invalid Git commit: {value}")]
    InvalidCommit {
        /// Rejected persisted or returned commit identity.
        value: String,
    },
    #[error("{0}")]
    Native(#[from] cowtree::Error),
    #[error("{0}")]
    Worktree(#[from] cowtree::WorktreeError),
    #[error("{0}")]
    Authority(#[from] cowtree_metadata::Error),
    #[error("{0}")]
    Object(#[from] cowtree_metadata::objects::ObjectError),
}

impl Error {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }

    pub(crate) fn record(path: &Path, source: serde_json::Error) -> Self {
        Self::Record { path: path.into(), source }
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum Issue {
    #[error("unrecognized operation blocks collection")]
    UnknownOperation,
    #[error("validation is still running")]
    ValidationBusy,
    #[error("invalid source path")]
    InvalidPath,
    #[error("filesystem aliases")]
    AliasedPath,
    #[error("directory identity changed")]
    ChangedDirectory,
    #[error("invalid workspace root")]
    InvalidRoot,
    #[error("source bytes changed")]
    SourceChanged,
    #[error("managed leaf must remain detached")]
    NotDetached,
    #[error("sparse workspace capture is unsupported")]
    SparseCheckout,
    #[error("source HEAD changed")]
    HeadChanged,
    #[error("reference is outside the cowtree namespace")]
    OutsideNamespace,
    #[error("capture source must be a checkout root")]
    SourceRoot,
    #[error("tracked source cannot be a cache or ephemeral")]
    ConflictingPolicy,
    #[error("unsupported pinned submodule")]
    InvalidSubmodule,
    #[error("read-only dependency changed")]
    DependencyChanged,
    #[error("snapshot source changed")]
    NodeChanged,
    #[error("node identity disagrees with its record")]
    NodeIdentity,
    #[error("snapshot budget exhausted; run collection")]
    SnapshotLimit,
    #[error("durable operation record is incomplete")]
    IncompleteRecord,
    #[error("exact candidate has not passed validation")]
    InvalidCandidate,
    #[error("resume or abort the pending publication")]
    PendingPublication,
    #[error("object content differs from its identity")]
    CorruptObject,
    #[error("workspace paths and symlinks require UTF-8")]
    NonUtf8,
    #[error("source has tracked changes or unsafe index flags")]
    DirtyCheckout,
    #[error("unsupported source entry")]
    UnsupportedEntry,
}

impl Issue {
    pub(crate) fn at(self, path: &Path) -> Error {
        Error::State { issue: self, path: path.into() }
    }
}

impl Error {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Cleanup { .. } => "cleanup_failed",
            Self::Native(error) => error.code(),
            Self::Worktree(error) => error.code(),
            Self::State { issue: Issue::SourceChanged | Issue::DirtyCheckout, .. } => {
                "dirty_source"
            }
            Self::State { issue: Issue::HeadChanged, .. } => "head_mismatch",
            Self::State { issue: Issue::SparseCheckout, .. } => "sparse_checkout",
            Self::State { issue: Issue::InvalidSubmodule, .. } => "submodule_unsupported",
            Self::State { issue: Issue::DependencyChanged, .. } => "dependency_changed",
            Self::State { issue: Issue::UnsupportedEntry, .. } => "unsupported_mode",
            Self::State {
                issue:
                    Issue::UnknownOperation
                    | Issue::NodeChanged
                    | Issue::NodeIdentity
                    | Issue::IncompleteRecord
                    | Issue::CorruptObject,
                ..
            } => "command_failed",
            Self::State { .. }
            | Self::InvalidCommit { .. }
            | Self::Record { .. }
            | Self::Git(cowtree_git::Error::Encoding { .. }) => "invalid_arguments",
            Self::Io { source, .. } | Self::Git(cowtree_git::Error::Io { source, .. })
                if source.kind() == io::ErrorKind::CrossesDevices =>
            {
                "different_filesystem"
            }
            Self::Io { .. } | Self::Git(cowtree_git::Error::Io { .. }) => "io",
            Self::Check { .. }
            | Self::Git(cowtree_git::Error::Command { .. })
            | Self::Authority(_)
            | Self::Object(_) => "command_failed",
        }
    }
}
