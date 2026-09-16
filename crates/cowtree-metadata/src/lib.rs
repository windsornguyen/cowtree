// Copyright (c) 2026 Windsor Nguyen

//! Local fenced publication with SQLite metadata and durable immutable objects.
//!
//! See the crate README for the authority, retention, and failure contracts.

mod batch;
mod batch_types;
mod database;
mod durability;
mod error;
mod fault;
mod imports;
mod install_plan;
mod installation;
mod leases;
mod maintenance;
pub mod objects;
mod publication;
mod resolution;
mod tree_io;
mod types;
mod workspace_init;

pub use batch_types::{BatchCandidate, BatchReceipt, ResolutionInput};
pub use database::Store;
pub use error::{Error, ErrorCode, ErrorDetails, LimitKind, Result, RetryAction, WireError};
pub use installation::Installation;
pub use types::{
    Candidate, Entry, EntryKind, Grant, LeafId, LeafView, Limits, Maintenance, Proposal,
    ProposalInput, ProposedChange, Receipt, RequestId, ResourcePath, Snapshot, Token, Version,
};
