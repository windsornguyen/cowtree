// Copyright (c) 2026 Windsor Nguyen

//! Durable records bind filesystem ownership to typed publication authority values.

use std::{collections::BTreeMap, num::NonZeroU64, path::PathBuf};

use cowtree_metadata::{
    BatchCandidate, Candidate, Entry, Grant, LeafId, Receipt, RequestId, ResourcePath, Snapshot,
};
use serde::{Deserialize, Serialize};

use crate::{CommitId, NodeId};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hardlinks {
    #[default]
    Reject,
    Clone,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmodulePolicy {
    #[default]
    Reject,
    MaterializePinned,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    /// Dependency namespace relative to the parent checkout.
    pub path: String,
    /// Exact commit recorded by the parent's gitlink.
    pub commit: CommitId,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Explicit cache prefixes retained across warm forks.
    pub derived: Vec<String>,
    /// Prefixes deliberately omitted from captures.
    pub ephemeral: Vec<String>,
    /// Git-ignored prefixes bound to the captured checkout.
    pub ignored: Vec<String>,
    /// Whether each derived hard-link alias receives an independent clone.
    pub derived_hardlinks: Hardlinks,
    /// Admission rule for parent Git links.
    pub submodules: SubmodulePolicy,
    /// Validated direct dependencies with immutable source namespaces.
    pub pins: Vec<Pin>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Owned directory name, distinct from the content's Git identity.
    pub id: NodeId,
    /// Previous checkpoint in this private history.
    pub parent: Option<NodeId>,
    /// Exact captured source bytes and access policy.
    pub source: Snapshot,
    /// Published values used to detect private edits.
    pub origins: Snapshot,
    /// Source-only Git projection of this checkpoint.
    pub git_commit: CommitId,
    /// File selection rules applied to the captured tree.
    pub policy: Policy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    /// Exact fenced candidate accepted by this check.
    pub candidate: Candidate,
    /// Argument vector executed in the private check leaf.
    pub command: Vec<String>,
    /// Durable combined stdout and stderr log.
    pub log: PathBuf,
    /// Checkpoint containing the verified source and warmed cache.
    pub node: NodeId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRecord {
    /// Durable authority acknowledgement.
    pub receipt: Receipt,
    /// Check that admitted this exact publication.
    pub validation: Validation,
    /// All acknowledgements when publication was atomic across leaves.
    #[serde(default)]
    pub receipts: Vec<Receipt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    /// Leaf-local idempotency key retained through recovery.
    pub request: RequestId,
    /// Immutable checkpoint supplying the captured bytes.
    pub node: NodeId,
    /// Selected changes, with None representing a deletion.
    pub changes: BTreeMap<ResourcePath, Option<Entry>>,
    /// Authority has acknowledged this capture.
    #[serde(default)]
    pub submitted: bool,
    /// A durable cancellation intent must be completed before other work.
    #[serde(default)]
    pub aborting: bool,
    /// Exact authority candidate, if preparation completed.
    pub candidate: Option<Candidate>,
    /// Atomic batch containing this request, if selected.
    pub batch: Option<BatchCandidate>,
    /// Published warm checkpoint used for candidate validation.
    pub parent_node: Option<NodeId>,
    /// Evidence from a successful check of the exact candidate.
    pub validation: Option<Validation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Leaf {
    /// Monotone authority identity never reused after retirement.
    pub id: LeafId,
    /// Owned checkout's absolute path.
    pub path: PathBuf,
    /// Filesystem device of the exclusively reserved directory.
    pub device: u64,
    /// Inode of that directory, checked before mutation or removal.
    pub inode: u64,
    /// Last retained private checkpoint.
    pub node: NodeId,
    /// Expected detached Git HEAD.
    pub git_head: CommitId,
    /// Per-path comparison origins independent of later private edits.
    pub origins: Snapshot,
    /// Authority reservations installed in this leaf.
    #[serde(default)]
    pub grants: BTreeMap<ResourcePath, Grant>,
    /// Next capture sequence, advanced after commit or abort.
    #[serde(default = "first_sequence")]
    pub sequence: NonZeroU64,
    /// One durable capture or publication being reconciled.
    pub pending: Option<Pending>,
    /// Candidate whose running validation owns this temporary leaf.
    pub check_candidate: Option<Candidate>,
    /// Last acknowledged publication retained for retries.
    pub last_receipt: Option<Receipt>,
}

fn first_sequence() -> NonZeroU64 {
    NonZeroU64::MIN
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Canonical store location, checked when reopening.
    pub location: PathBuf,
    /// Original admitted checkout.
    pub source: PathBuf,
    /// Stable common Git directory shared by private worktrees.
    pub git_directory: PathBuf,
    /// Executable recorded at initialization for provenance. Operations use linked Rust code.
    pub binary: PathBuf,
    /// Workspace-wide file selection and dependency policy.
    pub policy: Policy,
    /// Initial checkpoint and namespace for the published Git tip.
    pub initial: NodeId,
    /// Current checked source and cache checkpoint.
    pub warm_tip: NodeId,
}
