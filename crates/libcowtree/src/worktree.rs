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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lock {
    Release,
    Retain { reason: Option<String> },
}

#[derive(Debug, Clone)]
pub struct AddRequest {
    pub cancellation: crate::Cancellation,
    pub path: PathBuf,
    pub source: Option<PathBuf>,
    pub branch: Branch,
    pub revision: OsString,
    pub source_mode: SourceMode,
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
