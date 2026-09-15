// Copyright (c) 2026 Windsor Nguyen

//! Failures that callers can distinguish without parsing diagnostics.

use crate::objects::ObjectError;
use std::{io, path::PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Object(#[from] ObjectError),
    #[error("metadata JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid resource path: {0:?}")]
    InvalidPath(String),
    #[error("identifier is outside SQLite's nonnegative integer domain: {0}")]
    InvalidId(u64),
    #[error("metadata schema or application identity is unsupported")]
    Schema,
    #[error("SQLite {0} lacks the required WAL fix")]
    SQLiteVersion(String),
    #[error("required SQLite durability settings were not applied")]
    Durability,
    #[error("leaf {0} is not active")]
    LeafInactive(i64),
    #[error("resource {0} is leased by another leaf")]
    LeaseConflict(String),
    #[error("lease token is no longer current for {0}")]
    StaleToken(String),
    #[error("resource {0} has not been activated")]
    NotActivated(String),
    #[error("resource {0} contains retained dirty data")]
    DirtyPath(String),
    #[error("upload is not ready or not owned by this leaf")]
    UploadNotReady,
    #[error("request sequence {0} is expired or already retired")]
    RequestExpired(i64),
    #[error("request sequence must be {expected}, received {actual}")]
    RequestSequence { expected: i64, actual: i64 },
    #[error("request identity was reused with different inputs")]
    RequestConflict,
    #[error("proposal has no dirty paths")]
    EmptyProposal,
    #[error("proposal is aborted")]
    Aborted,
    #[error("candidate identity is not current")]
    CandidateMismatch,
    #[error("candidate content has not been durably prepared")]
    CandidateNotReady,
    #[error("workspace tip changed from {expected} to {actual}")]
    TipChanged { expected: i64, actual: i64 },
    #[error("proposal origin is stale for {0}")]
    StaleOrigin(String),
    #[error("snapshot {0} is no longer retained")]
    SnapshotExpired(i64),
    #[error("snapshot path {0} has a non-directory ancestor")]
    NamespaceConflict(String),
    #[error("invalid symlink contents for {0}")]
    InvalidSymlink(String),
    #[error("configured limit exceeded: {0}")]
    Limit(LimitKind),
    #[error("metadata counter exhausted")]
    CounterExhausted,
    #[error("WAL checkpoint is blocked by an active reader or writer")]
    CheckpointBusy,
}

/// Capacity category, allowing callers to distinguish maintenance from input limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LimitKind {
    #[error("active leaves")]
    ActiveLeaves,
    #[error("pending uploads")]
    PendingUploads,
    #[error("pending proposals")]
    PendingProposals,
    #[error("retained paths")]
    RetainedPaths,
    #[error("snapshot paths")]
    SnapshotPaths,
    #[error("object bytes")]
    ObjectBytes,
    #[error("metadata history; run maintenance")]
    MetadataHistory,
    #[error("manual snapshot pins")]
    ManualPins,
    #[error("WAL bytes; run maintenance")]
    WalBytes,
    #[error("positive capacity settings required")]
    Configuration,
}
