// Copyright (c) 2026 Windsor Nguyen

//! Explicit identities for atomic batches and caller-selected conflict resolution.

use crate::{Candidate, Receipt, RequestId, ResourcePath};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatchCandidate {
    /// Complete sorted membership; every member shares one parent and snapshot root.
    pub members: Vec<Candidate>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BatchReceipt {
    /// One durable result per member, all naming the same committed epoch.
    pub receipts: Vec<Receipt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolutionInput {
    /// New request identity belonging to the sources' common owner.
    pub request: RequestId,
    /// Pending captures explicitly superseded atomically by this resolution.
    pub sources: Vec<RequestId>,
    /// For every source path, the capture whose immutable value the caller selected.
    pub choices: BTreeMap<ResourcePath, RequestId>,
}
