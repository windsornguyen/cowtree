// Copyright (c) 2026 Windsor Nguyen

//! Standalone worktree requests distinguish branch ownership and source selection.

use serde::Serialize;
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Branch {
    Detached,
    New(OsString),
    Existing(OsString),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    Checkout,
    Committed,
}

/// Standalone submodule handling, independent of managed read-only snapshots.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmodulePolicy {
    Reject,
    #[default]
    LeaveUninitialized,
    MaterializePinned,
}

impl std::fmt::Display for SubmodulePolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Reject => "reject",
            Self::LeaveUninitialized => "leave-uninitialized",
            Self::MaterializePinned => "materialize-pinned",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lock {
    Release,
    Retain { reason: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RequestIssue {
    #[error("path and revision must be nonempty")]
    EmptyInput,
    #[error("lock reason must be nonempty and contain no NUL")]
    InvalidReason,
    #[error("existing branch selects the committed revision")]
    BranchSelectsRevision,
    #[error("destination requires a valid name and an existing ancestor")]
    InvalidDestination,
    #[error("branch must be a literal UTF-8 local name")]
    InvalidBranch,
    #[error("doctor requires an existing directory")]
    ProbeDirectory,
}

#[derive(Debug, Clone)]
pub struct AddRequest {
    pub cancellation: crate::Cancellation,
    pub path: PathBuf,
    pub source: Option<PathBuf>,
    pub branch: Branch,
    pub revision: OsString,
    pub source_mode: SourceMode,
    pub submodules: SubmodulePolicy,
    pub lock: Lock,
}

impl AddRequest {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            cancellation: crate::Cancellation::default(),
            path,
            source: None,
            branch: Branch::Detached,
            revision: "HEAD".into(),
            source_mode: SourceMode::Checkout,
            submodules: SubmodulePolicy::default(),
            lock: Lock::Release,
        }
    }
}

/// Fields and string values match the existing standalone JSON receipt contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Worktree {
    #[serde(serialize_with = "crate::path_json::path")]
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub prunable: bool,
    pub locked: bool,
    pub reason: Option<String>,
}
