// Copyright (c) 2026 Windsor Nguyen

//! Local fenced publication with SQLite metadata and durable immutable objects.
//!
//! See the crate README for the authority, retention, and failure contracts.

mod batch;
mod batch_types;
mod client_pins;
mod database;
mod durability;
mod error;
mod fault;
mod files;
mod import;
mod leases;
mod maintenance;
pub mod objects;
mod publication;
mod resolution;
mod types;

pub use batch_types::{BatchCandidate, BatchReceipt, ResolutionInput};
pub use database::Store;
pub use error::{Error, ErrorCode, ErrorDetails, LimitKind, Result, RetryAction, WireError};
pub use import::ImportProgress;
pub use types::{
    Candidate, Entry, EntryKind, FileKind, Grant, LeafId, LeafView, Limits, Maintenance, Proposal,
    ProposalInput, ProposedChange, Receipt, RequestId, ResourcePath, Snapshot, Token, Version,
};
