// Copyright (c) 2026 Windsor Nguyen

//! Validated protocol values and owned operation results.

use crate::{Error, Result, objects::ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ResourcePath(String);

impl ResourcePath {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 4096
            || value.contains('\0')
            || value.split('/').any(|part| part.is_empty() || part == "." || part == "..")
            || value.split('/').next() == Some(".git")
        {
            return Err(Error::InvalidPath(value));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        self == other
            || self.0.strip_prefix(&other.0).is_some_and(|tail| tail.starts_with('/'))
            || other.0.strip_prefix(&self.0).is_some_and(|tail| tail.starts_with('/'))
    }
}
impl TryFrom<String> for ResourcePath {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        Self::parse(value)
    }
}
impl From<ResourcePath> for String {
    fn from(value: ResourcePath) -> String {
        value.0
    }
}

macro_rules! identifier {
    ($name:ident, $min:expr) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
        #[serde(try_from = "u64", into = "u64")]
        pub struct $name(i64);
        impl $name {
            pub fn new(value: u64) -> Result<Self> {
                if !($min..=i64::MAX as u64).contains(&value) {
                    return Err(Error::InvalidId(value));
                }
                Ok(Self(value as i64))
            }
            pub fn get(self) -> u64 {
                self.0 as u64
            }
            pub(crate) fn sql(self) -> i64 {
                self.0
            }
            pub(crate) fn from_sql(value: i64) -> Result<Self> {
                let value = u64::try_from(value).map_err(|_| Error::Schema)?;
                Self::new(value)
            }
        }
        impl TryFrom<u64> for $name {
            type Error = Error;
            fn try_from(value: u64) -> Result<Self> {
                Self::new(value)
            }
        }
        impl From<$name> for u64 {
            fn from(value: $name) -> u64 {
                value.get()
            }
        }
    };
}
identifier!(LeafId, 1);
identifier!(Version, 0);
identifier!(Token, 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Executable,
    Symlink,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Immutable bytes, including symlink text for symlinks.
    pub object: ObjectId,
    /// Git-compatible file kind and executable state.
    pub kind: EntryKind,
}
pub type Snapshot = BTreeMap<ResourcePath, Entry>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    /// Leaf identity never reused after drop.
    pub leaf: LeafId,
    /// Exact reserved path; overlapping reservations exclude other leaves.
    pub path: ResourcePath,
    /// Captured generation required on every authority-changing operation.
    pub token: Token,
    /// Committed value installed during activation.
    pub origin: Option<Entry>,
    /// Activation has been acknowledged by the caller.
    pub activated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestId {
    /// Leaf owning this request namespace.
    pub leaf: LeafId,
    /// Monotone sequence beginning at one; expired sequences never execute again.
    pub sequence: u64,
}
impl RequestId {
    pub(crate) fn seq(self) -> Result<i64> {
        let v = i64::try_from(self.sequence).map_err(|_| Error::InvalidId(self.sequence))?;
        if v < 1 {
            return Err(Error::InvalidId(self.sequence));
        }
        Ok(v)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposalInput {
    /// Leaf and monotone request sequence identifying this capture.
    pub request: RequestId,
    /// Exact paths selected for one atomic capture.
    pub paths: BTreeSet<ResourcePath>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposedChange {
    /// Normalized path within the logical workspace.
    pub path: ResourcePath,
    /// Reservation generation captured for commit-time fencing.
    pub token: Token,
    /// Committed value on which the local change is based.
    pub origin: Option<Entry>,
    /// Current or captured local value; None represents deletion.
    pub value: Option<Entry>,
    /// Monotone local revision, preserved when a captured edit is published.
    pub edit_revision: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Leaf and monotone request sequence identifying this capture.
    pub request: RequestId,
    /// Committed epoch retained while this proposal is pending.
    pub captured_at: Version,
    /// Immutable path changes captured in one transaction.
    pub changes: Vec<ProposedChange>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Leaf and monotone request sequence identifying this capture.
    pub request: RequestId,
    /// Preparation generation; a later attempt supersedes this candidate.
    pub attempt: u64,
    /// Tip version used to build this candidate.
    pub parent: Version,
    /// SHA-256 identity of the immutable snapshot manifest.
    pub root: ObjectId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    /// Leaf and monotone request sequence identifying this capture.
    pub request: RequestId,
    /// Distinct committed publication version, even if content repeats.
    pub version: Version,
    /// SHA-256 identity of the immutable snapshot manifest.
    pub root: ObjectId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeafView {
    /// Normalized path within the logical workspace.
    pub path: ResourcePath,
    /// Committed value on which the local change is based.
    pub origin: Option<Entry>,
    /// Current or captured local value; None represents deletion.
    pub value: Option<Entry>,
    /// Monotone local revision, preserved when a captured edit is published.
    pub edit_revision: u64,
}
impl LeafView {
    pub fn dirty(&self) -> bool {
        self.origin != self.value
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum simultaneously active leaf identities.
    pub max_leaves: u32,
    /// Maximum snapshot entries and, separately, retained views/reservations.
    pub max_paths: u32,
    /// Maximum pending proposals, uploads, and manual pins, separately.
    pub max_pending: u32,
    /// Maximum bytes in one staged payload.
    pub max_object_bytes: u64,
    /// Recent committed epochs kept automatically by maintenance.
    pub retained_epochs: u32,
    /// Recent commit receipts kept automatically by maintenance.
    pub retained_receipts: u32,
    /// Observed WAL admission threshold; a transaction may overshoot.
    pub max_wal_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_leaves: 64,
            max_paths: 100_000,
            max_pending: 256,
            max_object_bytes: 64 * 1024 * 1024,
            retained_epochs: 16,
            retained_receipts: 128,
            max_wal_bytes: 64 * 1024 * 1024,
        }
    }
}
impl Limits {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.max_leaves == 0
            || self.max_paths == 0
            || self.max_pending == 0
            || self.max_object_bytes == 0
            || self.retained_epochs == 0
            || self.retained_receipts == 0
            || self.max_wal_bytes < 4096
        {
            return Err(Error::Limit(crate::LimitKind::Configuration));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Maintenance {
    /// Epoch rows pruned in this maintenance pass.
    pub epochs_removed: usize,
    /// Committed request receipts pruned in this pass.
    pub receipts_removed: usize,
    /// Unreferenced canonical payload or manifest files removed.
    pub objects_removed: usize,
    /// Abandoned temporary publication files removed.
    pub temporary_files_removed: usize,
    /// Observed database file length after space reclamation.
    pub database_bytes: u64,
    /// Observed WAL file length after the truncating checkpoint.
    pub wal_bytes: u64,
}
