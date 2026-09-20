// Copyright (c) 2026 Windsor Nguyen

//! Failures that callers can distinguish without parsing diagnostics.

use crate::objects::ObjectError;
use serde::Serialize;
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
    #[error("input must be an absolute path to a regular file: {0}")]
    InvalidInputFile(PathBuf),
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
    #[error("resource {path:?} overlaps read-only namespace {scope:?}")]
    ReadOnlyPath { path: crate::ResourcePath, scope: crate::ResourcePath },
    #[error("read-only scope {scope:?} does not contain {path:?}")]
    InvalidReadOnlyScope { path: crate::ResourcePath, scope: crate::ResourcePath },
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
    #[error("batch contains conflicting changes: {0}")]
    BatchConflict(String),
    #[error("resolution conflicts with current state: {0}")]
    ResolutionConflict(String),
    #[error("initial import conflicts with authority state: {0}")]
    ImportConflict(String),
    #[error("initial import has unpublished objects")]
    ImportNotReady,
    #[error("initial import source differs from its manifest at {0}")]
    ImportSourceChanged(String),
    #[error("configured limit exceeded: {0}")]
    Limit(LimitKind),
    #[error("metadata counter exhausted")]
    CounterExhausted,
    #[error("WAL checkpoint is blocked by an active reader or writer")]
    CheckpointBusy,
}

/// Capacity category, allowing callers to distinguish maintenance from input limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
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

/// Stable protocol codes. Diagnostic messages are not part of the wire contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Sqlite,
    DatabaseBusy,
    ObjectIdentifier,
    ObjectMissing,
    ObjectCorrupt,
    ObjectUnexpectedEntry,
    ObjectIo,
    MetadataJson,
    Io,
    InvalidPath,
    InvalidId,
    InvalidInputFile,
    Schema,
    SqliteVersion,
    Durability,
    LeafInactive,
    LeaseConflict,
    StaleToken,
    NotActivated,
    DirtyPath,
    ReadOnlyPath,
    InvalidReadOnlyScope,
    UploadNotReady,
    RequestExpired,
    RequestSequence,
    RequestConflict,
    EmptyProposal,
    Aborted,
    CandidateMismatch,
    CandidateNotReady,
    TipChanged,
    StaleOrigin,
    SnapshotExpired,
    NamespaceConflict,
    InvalidSymlink,
    LimitExceeded,
    CounterExhausted,
    CheckpointBusy,
    BatchConflict,
    ResolutionConflict,
    ImportConflict,
    ImportNotReady,
    ImportSourceChanged,
}

/// Required next step, never an instruction to retry indefinitely.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAction {
    None,
    RetrySameRequest,
    Reprepare,
    RunMaintenance,
    ResolveConflict,
}

/// Typed context carried alongside a stable error code.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErrorDetails {
    None,
    Conflict { reason: String },
    Path { path: String },
    ReadOnlyScope { path: crate::ResourcePath, scope: crate::ResourcePath },
    Identifier { value: u64 },
    Leaf { leaf: i64 },
    Sequence { sequence: i64 },
    SequenceMismatch { expected: i64, actual: i64 },
    Tip { expected: i64, actual: i64 },
    Version { version: i64 },
    Sqlite { extended_code: Option<i32> },
    RequiredSqlite { actual: String },
    Limit { resource: LimitKind },
    ObjectIdentifier { value: String },
    ObjectMissing { path: PathBuf, object: String },
    ObjectCorrupt { path: PathBuf, expected: String, actual: String },
    Io { path: PathBuf, os_code: Option<i32> },
    InvalidRequest { line: usize, column: usize },
    RequestTooLarge { max_bytes: usize },
}

/// Serializable failure shared by native callers and the JSON command interface.
#[derive(Debug, Serialize)]
pub struct WireError {
    /// Human-readable diagnostic; callers must not parse its wording.
    pub message: String,
    /// Stable failure category.
    pub code: ErrorCode,
    /// Required caller action before another attempt.
    pub retry_action: RetryAction,
    /// Category-specific context without string parsing.
    pub details: ErrorDetails,
}

impl Error {
    /// Preserve structured context without exposing SQLite or OS message parsing.
    pub fn wire(&self) -> WireError {
        WireError {
            message: self.to_string(),
            code: self.code(),
            retry_action: self.retry_action(),
            details: self.details(),
        }
    }

    /// Classify failures independently of human-readable message wording.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Sqlite(error) if database_busy(error) => ErrorCode::DatabaseBusy,
            Self::Sqlite(_) => ErrorCode::Sqlite,
            Self::Object(error) => object_code(error),
            Self::Json(_) => ErrorCode::MetadataJson,
            Self::Io { .. } => ErrorCode::Io,
            Self::InvalidPath(_) => ErrorCode::InvalidPath,
            Self::InvalidId(_) => ErrorCode::InvalidId,
            Self::InvalidInputFile(_) => ErrorCode::InvalidInputFile,
            Self::Schema => ErrorCode::Schema,
            Self::SQLiteVersion(_) => ErrorCode::SqliteVersion,
            Self::Durability => ErrorCode::Durability,
            Self::LeafInactive(_) => ErrorCode::LeafInactive,
            Self::LeaseConflict(_) => ErrorCode::LeaseConflict,
            Self::StaleToken(_) => ErrorCode::StaleToken,
            Self::NotActivated(_) => ErrorCode::NotActivated,
            Self::DirtyPath(_) => ErrorCode::DirtyPath,
            Self::ReadOnlyPath { .. } => ErrorCode::ReadOnlyPath,
            Self::InvalidReadOnlyScope { .. } => ErrorCode::InvalidReadOnlyScope,
            Self::UploadNotReady => ErrorCode::UploadNotReady,
            Self::RequestExpired(_) => ErrorCode::RequestExpired,
            Self::RequestSequence { .. } => ErrorCode::RequestSequence,
            Self::RequestConflict => ErrorCode::RequestConflict,
            Self::EmptyProposal => ErrorCode::EmptyProposal,
            Self::Aborted => ErrorCode::Aborted,
            Self::CandidateMismatch => ErrorCode::CandidateMismatch,
            Self::CandidateNotReady => ErrorCode::CandidateNotReady,
            Self::TipChanged { .. } => ErrorCode::TipChanged,
            Self::StaleOrigin(_) => ErrorCode::StaleOrigin,
            Self::SnapshotExpired(_) => ErrorCode::SnapshotExpired,
            Self::NamespaceConflict(_) => ErrorCode::NamespaceConflict,
            Self::InvalidSymlink(_) => ErrorCode::InvalidSymlink,
            Self::BatchConflict(_) => ErrorCode::BatchConflict,
            Self::ResolutionConflict(_) => ErrorCode::ResolutionConflict,
            Self::ImportConflict(_) => ErrorCode::ImportConflict,
            Self::ImportNotReady => ErrorCode::ImportNotReady,
            Self::ImportSourceChanged(_) => ErrorCode::ImportSourceChanged,
            Self::Limit(_) => ErrorCode::LimitExceeded,
            Self::CounterExhausted => ErrorCode::CounterExhausted,
            Self::CheckpointBusy => ErrorCode::CheckpointBusy,
        }
    }

    /// Identify the required next step without hiding input or durability failures.
    pub fn retry_action(&self) -> RetryAction {
        match self {
            Self::Sqlite(error) if database_busy(error) => RetryAction::RetrySameRequest,
            Self::CheckpointBusy => RetryAction::RetrySameRequest,
            Self::TipChanged { .. } => RetryAction::Reprepare,
            Self::StaleOrigin(_) => RetryAction::ResolveConflict,
            Self::Limit(LimitKind::MetadataHistory | LimitKind::WalBytes) => {
                RetryAction::RunMaintenance
            }
            _ => RetryAction::None,
        }
    }

    /// Return typed context for the classified failure.
    pub fn details(&self) -> ErrorDetails {
        match self {
            Self::Sqlite(error) => ErrorDetails::Sqlite {
                extended_code: error.sqlite_error().map(|error| error.extended_code),
            },
            Self::Object(error) => object_details(error),
            Self::ReadOnlyPath { path, scope } | Self::InvalidReadOnlyScope { path, scope } => {
                ErrorDetails::ReadOnlyScope { path: path.clone(), scope: scope.clone() }
            }
            Self::Io { path, source } => {
                ErrorDetails::Io { path: path.clone(), os_code: source.raw_os_error() }
            }
            Self::InvalidPath(path)
            | Self::LeaseConflict(path)
            | Self::StaleToken(path)
            | Self::NotActivated(path)
            | Self::DirtyPath(path)
            | Self::StaleOrigin(path)
            | Self::NamespaceConflict(path)
            | Self::InvalidSymlink(path)
            | Self::ImportSourceChanged(path) => ErrorDetails::Path { path: path.clone() },
            Self::BatchConflict(reason)
            | Self::ResolutionConflict(reason)
            | Self::ImportConflict(reason) => ErrorDetails::Conflict { reason: reason.clone() },
            Self::InvalidInputFile(path) => {
                ErrorDetails::Path { path: path.to_string_lossy().into_owned() }
            }
            Self::InvalidId(value) => ErrorDetails::Identifier { value: *value },
            Self::SQLiteVersion(actual) => ErrorDetails::RequiredSqlite { actual: actual.clone() },
            Self::LeafInactive(leaf) => ErrorDetails::Leaf { leaf: *leaf },
            Self::RequestExpired(sequence) => ErrorDetails::Sequence { sequence: *sequence },
            Self::RequestSequence { expected, actual } => {
                ErrorDetails::SequenceMismatch { expected: *expected, actual: *actual }
            }
            Self::TipChanged { expected, actual } => {
                ErrorDetails::Tip { expected: *expected, actual: *actual }
            }
            Self::SnapshotExpired(version) => ErrorDetails::Version { version: *version },
            Self::Limit(resource) => ErrorDetails::Limit { resource: *resource },
            Self::Json(_)
            | Self::Schema
            | Self::Durability
            | Self::UploadNotReady
            | Self::ImportNotReady
            | Self::RequestConflict
            | Self::EmptyProposal
            | Self::Aborted
            | Self::CandidateMismatch
            | Self::CandidateNotReady
            | Self::CounterExhausted
            | Self::CheckpointBusy => ErrorDetails::None,
        }
    }
}

fn database_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

fn object_code(error: &ObjectError) -> ErrorCode {
    match error {
        ObjectError::InvalidIdentifier { .. } => ErrorCode::ObjectIdentifier,
        ObjectError::Missing { .. } => ErrorCode::ObjectMissing,
        ObjectError::Corrupt { .. } => ErrorCode::ObjectCorrupt,
        ObjectError::UnexpectedEntry { .. } => ErrorCode::ObjectUnexpectedEntry,
        ObjectError::Io { .. } => ErrorCode::ObjectIo,
    }
}

fn object_details(error: &ObjectError) -> ErrorDetails {
    match error {
        ObjectError::InvalidIdentifier { value } => {
            ErrorDetails::ObjectIdentifier { value: value.clone() }
        }
        ObjectError::Missing { id, path } => {
            ErrorDetails::ObjectMissing { path: path.clone(), object: id.to_string() }
        }
        ObjectError::Corrupt { path, expected, actual } => ErrorDetails::ObjectCorrupt {
            path: path.clone(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        },
        ObjectError::UnexpectedEntry { path } => {
            ErrorDetails::Path { path: path.to_string_lossy().into_owned() }
        }
        ObjectError::Io { path, source } => {
            ErrorDetails::Io { path: path.clone(), os_code: source.raw_os_error() }
        }
    }
}
